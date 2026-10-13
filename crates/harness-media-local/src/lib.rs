//! On-device image generation for `harness-media`.
//!
//! `harness-media` runs any model whose id starts with `local/` through the
//! [`harness_media::LocalMedia`] provider a host installs; this crate is
//! that provider. It has three layers, each replaceable on its own:
//!
//! - [`catalog`] — which models exist, from which weight files, with which
//!   default settings and request schema (built-ins + a user overlay).
//! - [`store`] — the weight files on disk, shared between models, pulled
//!   from Hugging Face and checksummed.
//! - [`engine`] — the [`engine::MediaEngine`] trait a runtime implements.
//!   [`sdcpp`] (stable-diffusion.cpp's `sd-server`, Metal, no Python) is the
//!   one shipped today; an in-house engine or a video engine registers the
//!   same way and catalog entries pick it by id.
//!
//! [`service::LocalMediaService`] ties them together and queues jobs for the
//! GPU. Hosts call [`install`] once at boot.
//!
//! Built-in models are the commercially licensed ones benchmarked on a
//! 36 GB M4 Max (1024², warm): Iris-3B Turbo ~21 s, FLUX.2 klein 4B ~37 s
//! (also edits references), Z-Image-Turbo ~110 s.

pub mod catalog;
pub mod engine;
pub mod sdcpp;
pub mod service;
pub mod store;

use std::sync::{Arc, OnceLock};

pub use catalog::{ModelSpec, Settings, WeightFile};
pub use engine::{EngineRequest, EngineStatus, MediaEngine};
pub use service::LocalMediaService;
pub use store::{PullProgress, WeightStore};

/// What can go wrong outside a generation (catalog, store, runtime).
#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    #[error("local media catalog: {0}")]
    Catalog(String),
    #[error("{0}")]
    Store(String),
    #[error("stable-diffusion.cpp runtime: {0}")]
    Runtime(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

static SERVICE: OnceLock<Arc<LocalMediaService>> = OnceLock::new();

/// The process's service, built on first use from the catalog and store
/// under `~/.oxen-harness`.
pub fn service() -> Result<Arc<LocalMediaService>, LocalError> {
    if let Some(s) = SERVICE.get() {
        return Ok(s.clone());
    }
    let built = Arc::new(
        LocalMediaService::new(catalog::load()?, WeightStore::open()?).with_default_engines(),
    );
    Ok(SERVICE.get_or_init(|| built).clone())
}

/// Boot-time setup for a host: reap image servers a crashed host left
/// running, then make the service the process's local media provider.
/// Nothing is loaded — a server starts on the first local generation.
pub fn install() -> Result<Arc<LocalMediaService>, LocalError> {
    let reaped = sdcpp::reap_stale();
    if !reaped.is_empty() {
        tracing::info!("stopped {} orphaned sd-server(s)", reaped.len());
    }
    let service = service()?;
    if !harness_media::local::install(service.clone()) {
        tracing::warn!("a local media provider was already installed; keeping it");
    }
    Ok(service)
}
