//! Local LLM management for oxen-harness.
//!
//! Runs open-weight models (the Qwen3 family) on your own machine via
//! [llama.cpp](https://github.com/ggml-org/llama.cpp)'s `llama-server`, which
//! speaks the same OpenAI-compatible API the rest of the harness already uses.
//! The flow is:
//!
//! 1. Pick a model from the [`catalog`] (curated Qwen3 GGUFs with sizes).
//! 2. [`ModelStore`] downloads the GGUF into `~/.oxen-harness/models/`,
//!    reporting progress and tracking on-disk usage.
//! 3. [`server::LocalServer`] launches `llama-server` against that file and
//!    exposes a local `http://127.0.0.1:<port>/v1` endpoint the agent connects
//!    to — no API key, no cloud.
//!
//! Downloads are managed here (rather than delegated to `llama-server --hf`) so
//! the CLI and desktop app can show a real download indicator and report the
//! disk space each model occupies.

pub mod catalog;
pub mod fit;
pub mod gguf;
pub mod hardware;
pub mod runtime;
pub mod server;
pub mod source;
pub mod store;

pub use catalog::{catalog, find, quant_refs, ModelSpec};
pub use fit::{Fit, Quant, QuantCandidate};
pub use hardware::{detect as detect_hardware, Accelerator, HardwareProfile};
pub use runtime::{RuntimeInstallEvent, RuntimeSource, RuntimeStatus};
pub use server::{
    can_auto_install, install_hint, install_llama_server, llama_server_path, LoadPhase, LocalServer,
};
pub use source::{HfHit, ModelRef, Origin};
pub use store::{disk_space, DownloadProgress, ModelStore};

/// Errors from local model management.
#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    #[error("unknown local model `{0}` (run `oxen-harness models list`)")]
    UnknownModel(String),
    #[error("model `{0}` is not downloaded yet (run `oxen-harness models pull {0}`)")]
    NotDownloaded(String),
    #[error("llama-server not found on PATH. {0}")]
    LlamaServerMissing(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error("llama-server failed to start: {0}")]
    Server(String),
    #[error("install failed: {0}")]
    Install(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Format a byte count as a short human-readable string (e.g. `5.0 GB`).
///
/// Re-exported from [`harness_core::fmt`] so download/disk sizing throughout
/// this crate and the CLI share one implementation.
pub use harness_core::fmt::format_bytes;
