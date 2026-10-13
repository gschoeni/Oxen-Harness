//! On-device generation: the seam between the tools and whatever runs
//! models on this machine.
//!
//! The tools only know [`LocalMedia`]: a list of models (catalog rows with
//! `local/` ids) and a way to run one job. Which runtime does the work —
//! stable-diffusion.cpp today, an in-house engine or a video engine later —
//! is the implementor's business (`harness-media-local`), so this crate stays
//! free of model weights, processes, and GPUs.
//!
//! A host installs one provider per process with [`install`]: a GPU holds
//! one model at a time, so the provider is a machine-wide resource every
//! session shares rather than something each chat builds.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::catalog::MediaModel;
use crate::MediaKind;

/// Every local model id starts with this, so a glance (or the agent) can
/// tell an on-device model from a hosted one, and the two catalogs never
/// collide.
pub const LOCAL_PREFIX: &str = "local/";

/// Whether `id` names an on-device model.
pub fn is_local_id(id: &str) -> bool {
    id.trim().starts_with(LOCAL_PREFIX)
}

/// Every local generation id (a library row, and what `cancel` takes)
/// starts with this.
pub const LOCAL_JOB_PREFIX: &str = "local-";

/// Whether `id` names a local generation rather than a hub one.
pub fn is_local_job_id(id: &str) -> bool {
    id.trim().starts_with(LOCAL_JOB_PREFIX)
}

/// The local half of a catalog row: what the tools need to say about a
/// model that runs here instead of a price.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalInfo {
    /// The weights are on disk; a model that isn't can be listed but not run.
    pub installed: bool,
    /// The engine that runs it (`sd.cpp`).
    pub engine: String,
    /// SPDX id of the weights' license.
    pub license: String,
    /// Bytes still to download before it can run (0 once installed).
    #[serde(default)]
    pub download_bytes: u64,
    /// Typical seconds per output at the default settings on the machine
    /// it was measured on, for the agent's planning.
    #[serde(default)]
    pub typical_secs: Option<f64>,
}

/// One output to produce.
#[derive(Debug, Clone)]
pub struct LocalJob {
    /// The generation id the library row carries (`local-…`), which is also
    /// what `cancel` takes.
    pub id: String,
    pub model: String,
    pub kind: MediaKind,
    pub prompt: String,
    /// Validated parameters from the model's request schema (seed, steps,
    /// width, …), reference fields excluded.
    pub params: Map<String, Value>,
    /// Reference images, in the order the prompt numbers them.
    pub refs: Vec<PathBuf>,
}

/// Where a running job is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalProgress {
    /// Waiting behind another job for the GPU.
    Queued,
    /// Starting the runtime or loading weights into memory.
    Loading,
    /// Sampling step `step` of `total`.
    Step { step: u32, total: u32 },
    /// Sampling is done; decoding and encoding the output.
    Finishing,
    /// Something the user should know while it runs (memory is tight).
    Warning(String),
}

/// A finished output.
#[derive(Debug, Clone)]
pub struct LocalOutput {
    /// The encoded file (PNG for images).
    pub bytes: Vec<u8>,
    /// The seed actually used, when the engine reports it.
    pub seed: Option<Value>,
    /// What the engine wants on the manifest row as `provider`: engine,
    /// version, weights, settings, timings.
    pub record: Value,
}

/// Why a local job didn't produce an output.
#[derive(Debug, thiserror::Error)]
pub enum LocalMediaError {
    #[error("{0} is not installed: {1}")]
    NotInstalled(String, String),
    #[error("not enough free memory to load {model}: {detail}")]
    Memory { model: String, detail: String },
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

/// The on-device generation provider.
#[async_trait]
pub trait LocalMedia: Send + Sync {
    /// Every local model, installed or not, as catalog rows (`local/` ids,
    /// `local` filled in).
    fn models(&self) -> Vec<MediaModel>;

    /// Produce one output. Jobs queue for the GPU in arrival order;
    /// `progress` hears where this one is.
    async fn generate(
        &self,
        job: LocalJob,
        progress: Arc<dyn Fn(LocalProgress) + Send + Sync>,
    ) -> Result<LocalOutput, LocalMediaError>;

    /// Stop a queued or running job. `false` when no job has that id.
    async fn cancel(&self, id: &str) -> bool;
}

static PROVIDER: OnceLock<Arc<dyn LocalMedia>> = OnceLock::new();

/// Make `provider` the process's local generator. Later calls are ignored
/// (the first host to boot owns it); returns whether this call installed it.
pub fn install(provider: Arc<dyn LocalMedia>) -> bool {
    PROVIDER.set(provider).is_ok()
}

/// The installed provider, if the host set one up.
pub fn installed() -> Option<Arc<dyn LocalMedia>> {
    PROVIDER.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ids_are_prefixed() {
        assert!(is_local_id("local/flux-2-klein-4b"));
        assert!(is_local_id("  local/iris-3b-turbo"));
        assert!(!is_local_id("black-forest-labs-flux-2-klein-4b"));
    }
}
