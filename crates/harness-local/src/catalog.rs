//! Curated catalog of local models.
//!
//! Each entry points at a public Hugging Face GGUF (the `bartowski` Qwen3
//! quants, the de-facto standard) at the `Q4_K_M` quantization — the consumer
//! sweet spot (~4.5 bits/weight, near-FP16 quality at ~28% of the size). The
//! list spans the Qwen3 family from the tiny 0.6B up to the 32B dense model and
//! the 30B-A3B mixture-of-experts, so there is a fit for laptops through
//! workstations. `approx_bytes` is the published download size; the real size
//! is measured once a model is on disk.

/// A model the harness can download and run locally.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    /// Stable id used on the CLI and as the served model alias.
    pub id: &'static str,
    /// Human-friendly name.
    pub display: &'static str,
    /// Parameter-count label (e.g. `8B`, `30B-A3B (MoE)`).
    pub params: &'static str,
    /// Hugging Face repository hosting the GGUF.
    pub repo: &'static str,
    /// GGUF filename within the repo.
    pub file: &'static str,
    /// Quantization preset.
    pub quant: &'static str,
    /// Approximate download size in bytes (for pre-download display).
    pub approx_bytes: u64,
    /// Native context window in tokens.
    pub context: u32,
    /// A short "who is this for" note.
    pub note: &'static str,
}

const GB: u64 = 1024 * 1024 * 1024;
const MB: u64 = 1024 * 1024;

/// The built-in model catalog (Qwen3 family, Q4_K_M GGUFs).
pub fn catalog() -> &'static [ModelSpec] {
    &[
        ModelSpec {
            id: "qwen3-0.6b",
            display: "Qwen3 0.6B",
            params: "0.6B",
            repo: "bartowski/Qwen_Qwen3-0.6B-GGUF",
            file: "Qwen_Qwen3-0.6B-Q4_K_M.gguf",
            quant: "Q4_K_M",
            approx_bytes: 490 * MB,
            context: 40_960,
            note: "Tiny; runs anywhere. Good for quick edits and testing the loop.",
        },
        ModelSpec {
            id: "qwen3-1.7b",
            display: "Qwen3 1.7B",
            params: "1.7B",
            repo: "bartowski/Qwen_Qwen3-1.7B-GGUF",
            file: "Qwen_Qwen3-1.7B-Q4_K_M.gguf",
            quant: "Q4_K_M",
            approx_bytes: 1400 * MB,
            context: 40_960,
            note: "Small and snappy on a laptop CPU.",
        },
        ModelSpec {
            id: "qwen3-4b",
            display: "Qwen3 4B",
            params: "4B",
            repo: "bartowski/Qwen_Qwen3-4B-GGUF",
            file: "Qwen_Qwen3-4B-Q4_K_M.gguf",
            quant: "Q4_K_M",
            approx_bytes: 2500 * MB,
            context: 40_960,
            note: "Capable lightweight coder; fits in ~4 GB of RAM/VRAM.",
        },
        ModelSpec {
            id: "qwen3-8b",
            display: "Qwen3 8B",
            params: "8B",
            repo: "bartowski/Qwen_Qwen3-8B-GGUF",
            file: "Qwen_Qwen3-8B-Q4_K_M.gguf",
            quant: "Q4_K_M",
            approx_bytes: 5 * GB,
            context: 40_960,
            note: "Strong all-rounder for an 8-12 GB machine.",
        },
        ModelSpec {
            id: "qwen3-14b",
            display: "Qwen3 14B",
            params: "14B",
            repo: "bartowski/Qwen_Qwen3-14B-GGUF",
            file: "Qwen_Qwen3-14B-Q4_K_M.gguf",
            quant: "Q4_K_M",
            approx_bytes: 9 * GB,
            context: 40_960,
            note: "Noticeably sharper reasoning; wants ~12 GB.",
        },
        ModelSpec {
            id: "qwen3-30b-a3b",
            display: "Qwen3 30B-A3B (MoE)",
            params: "30B-A3B (MoE)",
            repo: "bartowski/Qwen_Qwen3-30B-A3B-GGUF",
            file: "Qwen_Qwen3-30B-A3B-Q4_K_M.gguf",
            quant: "Q4_K_M",
            approx_bytes: 18 * GB,
            context: 40_960,
            note: "Big-model quality at ~3B speed; great on a 24 GB card.",
        },
        ModelSpec {
            id: "qwen3-32b",
            display: "Qwen3 32B",
            params: "32B",
            repo: "bartowski/Qwen_Qwen3-32B-GGUF",
            file: "Qwen_Qwen3-32B-Q4_K_M.gguf",
            quant: "Q4_K_M",
            approx_bytes: 20 * GB,
            context: 40_960,
            note: "Heaviest dense option; needs ~24 GB and patience on CPU.",
        },
    ]
}

/// Look up a model by id.
pub fn find(id: &str) -> Option<&'static ModelSpec> {
    catalog().iter().find(|m| m.id == id)
}

/// Quants offered for a curated model, largest (best quality) first. These all
/// exist in the bartowski Qwen3 GGUF repos this catalog draws from.
const CURATED_QUANTS: &[&str] = &["Q8_0", "Q6_K", "Q5_K_M", "Q4_K_M", "Q3_K_M"];

/// The installable [`ModelRef`]s for a curated model — one per quant, with
/// filenames derived from the published `Q4_K_M` file and sizes scaled by
/// bits-per-weight. Largest-first so quant auto-pick takes the best that fits.
pub fn quant_refs(spec: &ModelSpec) -> Vec<crate::source::ModelRef> {
    use crate::fit;
    use crate::source::{slug, ModelRef, Origin};

    let ref_bpw = fit::QUANTS
        .iter()
        .find(|q| q.name == spec.quant)
        .map(|q| q.bits_per_weight)
        .unwrap_or(4.9);

    CURATED_QUANTS
        .iter()
        .filter_map(|&name| {
            let bpw = fit::QUANTS.iter().find(|q| q.name == name)?.bits_per_weight;
            let file = spec.file.replace(spec.quant, name);
            let size_bytes = if name == spec.quant {
                spec.approx_bytes
            } else {
                fit::scale_weight_bytes(spec.approx_bytes, ref_bpw, bpw)
            };
            Some(ModelRef {
                id: slug(spec.repo, name),
                display: format!("{} · {name}", spec.display),
                params: spec.params.to_string(),
                quant: name.to_string(),
                context: spec.context,
                size_bytes,
                origin: Origin::HuggingFace {
                    repo: spec.repo.to_string(),
                    file,
                    revision: "main".to_string(),
                },
            })
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

    #[test]
    fn ids_are_unique_and_nonempty() {
        let mut seen = std::collections::HashSet::new();
        for m in catalog() {
            assert!(!m.id.is_empty());
            assert!(seen.insert(m.id), "duplicate id: {}", m.id);
            assert!(m.file.ends_with(".gguf"));
            assert!(m.approx_bytes > 0);
        }
    }

    #[test]
    fn find_resolves_known_ids() {
        assert_eq!(find("qwen3-8b").unwrap().params, "8B");
        assert!(find("nope").is_none());
    }

    #[test]
    fn download_url_points_at_hugging_face() {
        let spec = find("qwen3-0.6b").unwrap();
        let url = download_url(spec);
        assert!(
            url.starts_with("https://huggingface.co/bartowski/Qwen_Qwen3-0.6B-GGUF/resolve/main/")
        );
        assert!(url.contains("Qwen_Qwen3-0.6B-Q4_K_M.gguf"));
    }
}
