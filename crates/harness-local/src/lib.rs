//! Local LLM management for oxen-harness.
//!
//! Runs open-weight GGUF models (including Qwen3 and Bonsai) on your own machine via
//! [llama.cpp](https://github.com/ggml-org/llama.cpp)'s `llama-server`, which
//! speaks the same OpenAI-compatible API the rest of the harness already uses.
//! The flow is:
//!
//! 1. Pick a model from the [`catalog`](mod@catalog) — loaded from config files
//!    (an embedded default list plus `~/.oxen-harness/local-models.json`), never
//!    hardcoded — or name anything already in the store.
//! 2. [`ModelStore`] downloads the GGUF into `~/.oxen-harness/models/`,
//!    reporting progress and tracking on-disk usage.
//! 3. [`resolve_runnable`] maps an id to weights, preferring what's already on
//!    disk so a downloaded model starts with **no network at all**.
//! 4. [`server::LocalServer`] launches `llama-server` against that file and
//!    exposes a local `http://127.0.0.1:<port>/v1` endpoint the agent connects
//!    to — no API key, no cloud.
//!
//! Downloads are managed here (rather than delegated to `llama-server --hf`) so
//! the CLI and desktop app can show a real download indicator and report the
//! disk space each model occupies.

pub mod catalog;
mod download;
pub mod fit;
pub mod gguf;
pub mod hardware;
pub mod limits;
pub mod resolve;
pub mod runtime;
pub mod server;
pub mod source;
pub mod store;

pub use catalog::{catalog, find, quant_refs, ModelSpec};
pub use fit::{Fit, Quant, QuantCandidate};
pub use hardware::{detect as detect_hardware, Accelerator, HardwareProfile};
pub use resolve::{resolve_runnable, Runnable};
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

/// Point `OXEN_HARNESS_DIR` at a fresh temp directory until the returned guard
/// drops, restoring the previous value after. Env vars are process-wide, so
/// every test in this crate that touches harness paths serializes on the one
/// internal lock here — hold the guard for the whole test (it works across
/// `.await`s, which a closure helper can't).
#[cfg(test)]
pub(crate) fn temp_harness_dir() -> TempHarnessDir {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os(harness_config::paths::BASE_DIR_ENV);
    std::env::set_var(harness_config::paths::BASE_DIR_ENV, tmp.path());
    TempHarnessDir {
        _tmp: tmp,
        prev,
        _lock: lock,
    }
}

#[cfg(test)]
pub(crate) struct TempHarnessDir {
    _tmp: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TempHarnessDir {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(harness_config::paths::BASE_DIR_ENV, v),
            None => std::env::remove_var(harness_config::paths::BASE_DIR_ENV),
        }
    }
}

/// Run `f` under [`temp_harness_dir`] — the closure form for sync tests.
#[cfg(test)]
pub(crate) fn with_temp_harness_dir(f: impl FnOnce()) {
    let _guard = temp_harness_dir();
    f();
}
