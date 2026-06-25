//! Shared configuration plumbing for oxen-harness.
//!
//! Three concerns the CLI, desktop app, and library crates all share:
//!
//! - [`paths`] — the one place that knows where `~/.oxen-harness` and everything
//!   under it lives.
//! - [`io`] — atomic, schema-versioned reads/writes for JSON config files.
//! - [`secrets`] — API keys kept in a `.env` file, loaded into the environment
//!   and never written into the versioned config.

pub mod io;
pub mod paths;
pub mod secrets;

use std::path::PathBuf;

/// Errors from resolving paths or reading/writing config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine the home directory")]
    NoHome,
    #[error("config IO failed at `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

// Re-export the most-used items so callers can write `harness_config::base_dir()`
// and `harness_config::Versioned`.
pub use io::{atomic_write, read_versioned, write_versioned, Versioned, UNVERSIONED};
pub use paths::{base_dir, base_dir_unchecked};

#[cfg(test)]
pub(crate) mod testutil {
    use std::sync::{Mutex, MutexGuard};

    /// Tests that mutate process-global env vars (`OXEN_HARNESS_DIR` and the
    /// secret vars) must hold this lock so they don't race each other across the
    /// test runner's threads. Poisoning is ignored — a panicking test still
    /// releases the serialization guarantee for the next one.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}
