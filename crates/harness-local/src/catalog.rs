//! The curated catalog of local models, loaded from configuration files.
//!
//! Nothing here is hardcoded in Rust: the built-in list ships as
//! [`assets/catalog.json`](../assets/catalog.json) (embedded at compile time),
//! and users can add their own entries — or override a built-in by reusing its
//! `id` — in `~/.oxen-harness/local-models.json`, which is merged in on every
//! read. Both files share one shape:
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "models": [
//!     {
//!       "id": "qwen3-8b",
//!       "display": "Qwen3 8B",
//!       "params": "8B",
//!       "repo": "bartowski/Qwen_Qwen3-8B-GGUF",
//!       "file": "Qwen_Qwen3-8B-Q4_K_M.gguf",
//!       "quant": "Q4_K_M",
//!       "approx_bytes": 5368709120,
//!       "context": 40960,
//!       "derive_quants": true,
//!       "note": "Strong all-rounder for an 8-12 GB machine."
//!     }
//!   ]
//! }
//! ```
//!
//! Only `id`, `repo`, and `file` are required in a user entry — `display`,
//! `params`, and `quant` are derived from the repo/filename when omitted.
//! `derive_quants` is opt-in: use it only when the repository publishes the
//! standard Q8-to-Q3 filename ladder. This keeps one-off/native formats such as
//! Bonsai's Q1/Q2 packs from advertising files that do not exist.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Schema version for `local-models.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// A model the harness can download and run locally.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelSpec {
    /// Stable id used on the CLI and as the served model alias.
    pub id: String,
    /// Human-friendly name (defaults to the id).
    #[serde(default)]
    pub display: String,
    /// Parameter-count label, e.g. `8B`, `30B-A3B (MoE)` (derived if omitted).
    #[serde(default)]
    pub params: String,
    /// Hugging Face repository hosting the GGUF.
    pub repo: String,
    /// GGUF filename within the repo.
    pub file: String,
    /// Quantization preset (parsed from the filename if omitted).
    #[serde(default)]
    pub quant: String,
    /// Approximate download size in bytes (for pre-download display).
    #[serde(default)]
    pub approx_bytes: u64,
    /// Native context window in tokens (0 = read it from the GGUF once local).
    #[serde(default)]
    pub context: u32,
    /// Derive the standard Q8-to-Q3 sibling filenames from `file`.
    #[serde(default)]
    pub derive_quants: bool,
    /// A short "who is this for" note.
    #[serde(default)]
    pub note: String,
}

/// The payload both catalog files carry under their `schema_version`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct CatalogFile {
    #[serde(default)]
    models: Vec<ModelSpec>,
}

/// The built-in catalog, embedded from `assets/catalog.json` at compile time so
/// it needs no network and no install step.
const BUILTIN_CATALOG_JSON: &str = include_str!("../assets/catalog.json");

/// Fill in the optional fields of a spec: display falls back to the id, and
/// quant/params are parsed from the filename/repo, so a minimal user entry
/// (`id` + `repo` + `file`) still renders and sizes sensibly.
fn normalize(mut spec: ModelSpec) -> ModelSpec {
    if spec.display.trim().is_empty() {
        spec.display = spec.id.clone();
    }
    if spec.quant.trim().is_empty() {
        spec.quant = crate::source::parse_quant(&spec.file).unwrap_or_default();
    }
    if spec.params.trim().is_empty() {
        spec.params = crate::source::parse_params(&spec.repo);
        if spec.params.is_empty() {
            spec.params = crate::source::parse_params(&spec.file);
        }
    }
    spec
}

/// The built-in models (parsed once). An unparseable embedded catalog is a
/// build defect — guarded by a test — so it degrades to empty rather than
/// panicking in a user's session.
fn builtins() -> &'static [ModelSpec] {
    static CACHE: OnceLock<Vec<ModelSpec>> = OnceLock::new();
    CACHE.get_or_init(|| {
        serde_json::from_str::<CatalogFile>(BUILTIN_CATALOG_JSON)
            .map(|f| f.models.into_iter().map(normalize).collect())
            .unwrap_or_default()
    })
}

/// The user's additions/overrides from `~/.oxen-harness/local-models.json`
/// (empty if the file is absent or unreadable — config is never a hard failure).
fn user_models() -> Vec<ModelSpec> {
    let Ok(path) = harness_config::paths::local_models_file() else {
        return Vec::new();
    };
    let (_, file) = harness_config::io::read_versioned::<CatalogFile>(&path);
    file.models.into_iter().map(normalize).collect()
}

/// The full model catalog: built-ins merged with the user's file at read time.
/// A user entry that reuses a built-in `id` replaces it; new ids append.
pub fn catalog() -> Vec<ModelSpec> {
    let mut out: Vec<ModelSpec> = builtins().to_vec();
    for user in user_models() {
        if user.id.trim().is_empty() || user.file.trim().is_empty() {
            continue; // an id-less or file-less entry can't be addressed or run
        }
        match out.iter_mut().find(|m| m.id == user.id) {
            Some(slot) => *slot = user,
            None => out.push(user),
        }
    }
    out
}

/// Look up a catalog model by id.
pub fn find(id: &str) -> Option<ModelSpec> {
    catalog().into_iter().find(|m| m.id == id)
}

/// Standard sibling quants for entries that opt into filename derivation,
/// largest (best quality) first. The built-in bartowski Qwen3 repos publish all
/// of these; native and one-off formats leave `derive_quants` disabled.
const DERIVED_QUANTS: &[&str] = &["Q8_0", "Q6_K", "Q5_K_M", "Q4_K_M", "Q3_K_M"];

/// The installable [`ModelRef`](crate::source::ModelRef)s for a catalog model —
/// one per quant, with filenames derived from the spec's published file and
/// sizes scaled by bits-per-weight. Largest-first so quant auto-pick takes the
/// best that fits.
///
/// Exact-file mode is the safe default. Siblings are derived only when the spec
/// opts in, its filename carries the quant token, and that quant has sizing
/// metadata.
pub fn quant_refs(spec: &ModelSpec) -> Vec<crate::source::ModelRef> {
    use crate::fit;
    use crate::source::{slug, ModelRef, Origin};

    let make_ref = |quant: &str, file: String, size_bytes: u64| ModelRef {
        id: slug(&spec.repo, quant),
        display: if quant.is_empty() {
            spec.display.clone()
        } else {
            format!("{} · {quant}", spec.display)
        },
        params: spec.params.clone(),
        quant: quant.to_string(),
        context: spec.context,
        size_bytes,
        origin: Origin::HuggingFace {
            repo: spec.repo.to_string(),
            file,
            revision: "main".to_string(),
        },
    };

    // Offer only the published file unless sibling derivation is explicitly
    // enabled and the filename has a safely replaceable quant token.
    if !spec.derive_quants || spec.quant.is_empty() || !spec.file.contains(&spec.quant) {
        return vec![make_ref(&spec.quant, spec.file.clone(), spec.approx_bytes)];
    }

    let Some(ref_bpw) = fit::QUANTS
        .iter()
        .find(|q| q.name == spec.quant)
        .map(|q| q.bits_per_weight)
    else {
        return vec![make_ref(&spec.quant, spec.file.clone(), spec.approx_bytes)];
    };

    DERIVED_QUANTS
        .iter()
        .filter_map(|&name| {
            let bpw = fit::QUANTS.iter().find(|q| q.name == name)?.bits_per_weight;
            let file = spec.file.replace(&spec.quant, name);
            let size_bytes = if name == spec.quant {
                spec.approx_bytes
            } else {
                fit::scale_weight_bytes(spec.approx_bytes, ref_bpw, bpw)
            };
            Some(make_ref(name, file, size_bytes))
        })
        .collect()
}

/// The Hugging Face direct-download URL for a model's GGUF.
pub fn download_url(spec: &ModelSpec) -> String {
    format!(
        "https://huggingface.co/{}/resolve/main/{}?download=true",
        spec.repo, spec.file
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_temp_harness_dir;

    #[test]
    fn builtin_catalog_parses_with_unique_nonempty_ids() {
        let models = builtins();
        assert!(!models.is_empty(), "embedded catalog.json failed to parse");
        let mut seen = std::collections::HashSet::new();
        for m in models {
            assert!(!m.id.is_empty());
            assert!(seen.insert(m.id.clone()), "duplicate id: {}", m.id);
            assert!(m.file.ends_with(".gguf"));
            assert!(m.approx_bytes > 0);
            assert!(!m.display.is_empty());
            assert!(!m.quant.is_empty());
            if m.derive_quants {
                assert!(
                    m.file.contains(&m.quant),
                    "{} opts into quant derivation, but its file lacks {}",
                    m.id,
                    m.quant
                );
                assert!(
                    crate::fit::QUANTS.iter().any(|q| q.name == m.quant),
                    "{} derives siblings from unsized quant {}",
                    m.id,
                    m.quant
                );
            }
        }
    }

    #[test]
    fn find_resolves_known_ids() {
        with_temp_harness_dir(|| {
            assert_eq!(find("qwen3-8b").unwrap().params, "8B");
            assert!(find("nope").is_none());
        });
    }

    #[test]
    fn user_file_adds_and_overrides_models() {
        with_temp_harness_dir(|| {
            let path = harness_config::paths::local_models_file().unwrap();
            std::fs::write(
                &path,
                r#"{
                  "schema_version": 1,
                  "models": [
                    { "id": "my-model", "repo": "me/My-Model-GGUF",
                      "file": "My-Model-7B-Q5_K_M.gguf" },
                    { "id": "qwen3-8b", "display": "Qwen3 8B (patched)",
                      "repo": "someone/else", "file": "other-Q4_K_M.gguf",
                      "quant": "Q4_K_M" }
                  ]
                }"#,
            )
            .unwrap();

            // The addition appears, with display/quant/params derived.
            let added = find("my-model").expect("user model should be listed");
            assert_eq!(added.display, "my-model");
            assert_eq!(added.quant, "Q5_K_M");
            assert_eq!(added.params, "7B");
            assert_eq!(
                quant_refs(&added).len(),
                1,
                "custom entries must not invent sibling filenames"
            );

            // The override replaces the built-in (no duplicate id).
            let cat = catalog();
            assert_eq!(cat.iter().filter(|m| m.id == "qwen3-8b").count(), 1);
            assert_eq!(find("qwen3-8b").unwrap().display, "Qwen3 8B (patched)");

            // Built-ins stay present alongside user entries.
            assert!(find("qwen3-0.6b").is_some());
        });
    }

    #[test]
    fn absent_or_garbage_user_file_reads_as_builtins() {
        with_temp_harness_dir(|| {
            assert_eq!(catalog().len(), builtins().len());
            let path = harness_config::paths::local_models_file().unwrap();
            std::fs::write(&path, "not json{{").unwrap();
            assert_eq!(catalog().len(), builtins().len());
        });
    }

    #[test]
    fn derived_quants_all_have_a_bits_per_weight_in_fit() {
        // quant_refs() sizes each offered quant by looking it up in fit::QUANTS;
        // a derived quant missing there would be silently dropped. Guard the
        // invariant so renaming a quant fails loudly here instead.
        for name in DERIVED_QUANTS {
            assert!(
                crate::fit::QUANTS.iter().any(|q| &q.name == name),
                "DERIVED_QUANTS has `{name}`, absent from fit::QUANTS"
            );
        }
    }

    #[test]
    fn quant_refs_derive_a_ladder_when_the_filename_carries_the_quant() {
        with_temp_harness_dir(|| {
            let spec = find("qwen3-8b").unwrap();
            let refs = quant_refs(&spec);
            assert_eq!(refs.len(), DERIVED_QUANTS.len());
            // Largest-first, and the published quant keeps its exact size.
            assert_eq!(refs[0].quant, "Q8_0");
            let published = refs.iter().find(|r| r.quant == spec.quant).unwrap();
            assert_eq!(published.size_bytes, spec.approx_bytes);
        });
    }

    #[test]
    fn bonsai_27b_entries_match_the_published_mainline_ggufs() {
        with_temp_harness_dir(|| {
            let binary = find("bonsai-27b").expect("1-bit Bonsai 27B should be built in");
            assert_eq!(binary.repo, "prism-ml/Bonsai-27B-gguf");
            assert_eq!(binary.file, "Bonsai-27B-Q1_0.gguf");
            assert_eq!(binary.quant, "Q1_0");
            assert_eq!(binary.approx_bytes, 3_803_452_480);
            assert_eq!(binary.context, 262_144);
            let binary_refs = quant_refs(&binary);
            assert_eq!(binary_refs.len(), 1);
            assert_eq!(binary_refs[0].size_bytes, binary.approx_bytes);

            let ternary =
                find("ternary-bonsai-27b").expect("ternary Bonsai 27B should be built in");
            assert_eq!(ternary.repo, "prism-ml/Ternary-Bonsai-27B-gguf");
            assert_eq!(ternary.file, "Ternary-Bonsai-27B-Q2_g64.gguf");
            assert_eq!(ternary.quant, "Q2_G64");
            assert_eq!(ternary.approx_bytes, 7_585_330_240);
            assert_eq!(ternary.context, 262_144);
            let ternary_refs = quant_refs(&ternary);
            assert_eq!(ternary_refs.len(), 1);
            assert_eq!(ternary_refs[0].size_bytes, ternary.approx_bytes);
        });
    }

    #[test]
    fn quant_refs_fall_back_to_a_single_file_without_a_quant_token() {
        let spec = normalize(ModelSpec {
            id: "custom".into(),
            display: String::new(),
            params: String::new(),
            repo: "me/custom".into(),
            file: "custom-model.gguf".into(),
            quant: String::new(),
            approx_bytes: 42,
            context: 0,
            derive_quants: false,
            note: String::new(),
        });
        let refs = quant_refs(&spec);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].size_bytes, 42);
        match &refs[0].origin {
            crate::source::Origin::HuggingFace { file, .. } => {
                assert_eq!(file, "custom-model.gguf")
            }
            other => panic!("unexpected origin {other:?}"),
        }
    }

    #[test]
    fn quant_refs_do_not_derive_from_an_unsized_quant() {
        let spec = ModelSpec {
            id: "experimental".into(),
            display: "Experimental".into(),
            params: "7B".into(),
            repo: "me/experimental".into(),
            file: "experimental-NATIVE_1.gguf".into(),
            quant: "NATIVE_1".into(),
            approx_bytes: 42,
            context: 0,
            derive_quants: true,
            note: String::new(),
        };

        let refs = quant_refs(&spec);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].quant, "NATIVE_1");
        assert_eq!(refs[0].size_bytes, 42);
    }

    #[test]
    fn download_url_points_at_hugging_face() {
        with_temp_harness_dir(|| {
            let spec = find("qwen3-0.6b").unwrap();
            let url = download_url(&spec);
            assert!(url.starts_with(
                "https://huggingface.co/bartowski/Qwen_Qwen3-0.6B-GGUF/resolve/main/"
            ));
            assert!(url.contains("Qwen_Qwen3-0.6B-Q4_K_M.gguf"));
        });
    }
}
