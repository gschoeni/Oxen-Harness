//! Offline-first resolution of "run local model `<id>`".
//!
//! A local model can be named two ways: by the id of a model already in the
//! [`ModelStore`] (anything ever downloaded — curated, Hugging Face, or a GGUF
//! dropped in by hand), or by a catalog id from the config-file catalog
//! ([`crate::catalog`]). Resolution always prefers weights that are already on
//! disk, so starting a downloaded model never touches the network — a session
//! on a plane runs exactly like one at a desk. Only a catalog model with no
//! quant installed resolves to a download.

use std::path::PathBuf;

use crate::catalog;
use crate::source::ModelRef;
use crate::store::ModelStore;
use crate::{LocalError, ModelSpec};

/// What running `<id>` requires.
#[derive(Debug, Clone)]
pub enum Runnable {
    /// The weights are on disk — serve them directly, no network needed.
    Installed { model: ModelRef, path: PathBuf },
    /// A catalog model with nothing on disk yet: download `model` first.
    Downloadable { spec: ModelSpec, model: ModelRef },
}

/// Resolve `<id>` against what's installed and what the catalog offers:
///
/// 1. An exact installed store id (any downloaded model) wins outright.
/// 2. A catalog id whose weights are on disk at *any* quant (the published
///    quant preferred, then best quality first) also runs offline.
/// 3. A catalog id with nothing installed resolves to its default download.
/// 4. Anything else is [`LocalError::UnknownModel`].
pub fn resolve_runnable(store: &ModelStore, id: &str) -> Result<Runnable, LocalError> {
    if let Some(model) = store.installed_ref(id) {
        return Ok(Runnable::Installed {
            path: store.path_for(id),
            model,
        });
    }

    let Some(spec) = catalog::find(id) else {
        return Err(LocalError::UnknownModel(id.to_string()));
    };
    let refs = catalog::quant_refs(&spec);

    // Any installed quant of this catalog model runs offline; prefer the
    // published quant, then the ladder's best-quality-first order.
    let installed = refs
        .iter()
        .find(|r| r.quant == spec.quant && store.is_installed(&r.id))
        .or_else(|| refs.iter().find(|r| store.is_installed(&r.id)));
    if let Some(r) = installed {
        return Ok(Runnable::Installed {
            path: store.path_for(&r.id),
            model: r.clone(),
        });
    }

    // Nothing on disk: offer the published quant (or the best available).
    let model = refs
        .iter()
        .find(|r| r.quant == spec.quant)
        .or_else(|| refs.first())
        .cloned()
        .ok_or_else(|| LocalError::UnknownModel(id.to_string()))?;
    Ok(Runnable::Downloadable { spec, model })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_temp_harness_dir;

    fn store() -> ModelStore {
        ModelStore::open().unwrap()
    }

    /// Fake a completed download: the GGUF plus its sidecar.
    fn install(store: &ModelStore, model: &ModelRef) {
        std::fs::write(store.path_for(&model.id), b"gguf-bytes").unwrap();
        std::fs::write(
            store.dir().join(format!("{}.json", model.id)),
            serde_json::to_string(model).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn an_installed_store_id_resolves_without_the_catalog() {
        with_temp_harness_dir(|| {
            let store = store();
            // A model that exists nowhere in the catalog (e.g. installed via
            // the desktop's Hugging Face search) — the plane scenario.
            let model = ModelRef {
                id: "qwythos-9b-q4-k-m".into(),
                display: "Qwythos 9B".into(),
                params: "9B".into(),
                quant: "Q4_K_M".into(),
                context: 0,
                size_bytes: 0,
                origin: crate::source::Origin::HuggingFace {
                    repo: "ox/Qwythos-9B-GGUF".into(),
                    file: "Qwythos-9B-Q4_K_M.gguf".into(),
                    revision: "main".into(),
                },
            };
            install(&store, &model);

            match resolve_runnable(&store, "qwythos-9b-q4-k-m").unwrap() {
                Runnable::Installed { model: m, path } => {
                    assert_eq!(m.display, "Qwythos 9B");
                    assert!(path.is_file());
                }
                other => panic!("expected Installed, got {other:?}"),
            }
        });
    }

    #[test]
    fn a_bare_gguf_without_sidecar_still_resolves() {
        with_temp_harness_dir(|| {
            let store = store();
            std::fs::write(store.path_for("hand-placed-Q5_K_M"), b"x").unwrap();
            match resolve_runnable(&store, "hand-placed-Q5_K_M").unwrap() {
                Runnable::Installed { model, .. } => assert_eq!(model.quant, "Q5_K_M"),
                other => panic!("expected Installed, got {other:?}"),
            }
        });
    }

    #[test]
    fn a_catalog_id_prefers_an_installed_quant_over_a_download() {
        with_temp_harness_dir(|| {
            let store = store();
            let spec = catalog::find("qwen3-8b").unwrap();
            // Install a *non-default* quant: the catalog id must still resolve
            // to it rather than asking to download the published quant.
            let q6 = catalog::quant_refs(&spec)
                .into_iter()
                .find(|r| r.quant == "Q6_K")
                .unwrap();
            install(&store, &q6);

            match resolve_runnable(&store, "qwen3-8b").unwrap() {
                Runnable::Installed { model, .. } => assert_eq!(model.quant, "Q6_K"),
                other => panic!("expected Installed, got {other:?}"),
            }
        });
    }

    #[test]
    fn a_catalog_id_with_nothing_on_disk_offers_the_published_quant() {
        with_temp_harness_dir(|| {
            let store = store();
            match resolve_runnable(&store, "qwen3-8b").unwrap() {
                Runnable::Downloadable { spec, model } => {
                    assert_eq!(spec.id, "qwen3-8b");
                    assert_eq!(model.quant, spec.quant);
                }
                other => panic!("expected Downloadable, got {other:?}"),
            }
        });
    }

    #[test]
    fn an_unknown_id_errors() {
        with_temp_harness_dir(|| {
            let err = resolve_runnable(&store(), "nope").unwrap_err();
            assert!(matches!(err, LocalError::UnknownModel(_)));
        });
    }
}
