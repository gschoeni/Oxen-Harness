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
pub mod server;
pub mod store;

pub use catalog::{catalog, download_url, find, ModelSpec};
pub use server::{
    can_auto_install, install_hint, install_llama_server, llama_server_path, LocalServer,
};
pub use store::{DownloadProgress, ModelStatus, ModelStore};

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
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_byte_counts() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
        assert_eq!(format_bytes(20_400_000_000), "19.0 GB");
    }
}
