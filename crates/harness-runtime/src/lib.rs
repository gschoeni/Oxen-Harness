//! Front-end-agnostic runtime services shared by the CLI and desktop app.
//!
//! The review flagged that the Tauri bridge was accreting application logic that
//! should be shared, so session behavior wouldn't drift between front ends. The
//! agent *loop* is already shared (`harness_agent::Agent`); what was duplicated
//! was the surrounding configuration. This crate owns that:
//!
//! - [`connection`] — Oxen host + API/Brave keys, with secrets kept in `.env`
//!   (migrated out of the old plaintext `connection.json`) and one resolution
//!   path both front ends use to build a client.
//! - [`config_repo`] — opt-in Oxen versioning of `~/.oxen-harness`, snapshotted
//!   after config changes.

pub mod config_repo;
pub mod connection;
pub mod models;
pub mod tools;

/// A process-wide lock serializing tests that mutate global env vars / the shared
/// config dir (`connection` and `models` both redirect `OXEN_HARNESS_DIR`). Cargo
/// runs a crate's tests on multiple threads in one process, so without this they
/// race on that env var.
#[cfg(test)]
pub(crate) static TEST_ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Errors from the runtime services.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Config(#[from] harness_config::ConfigError),
    #[error(transparent)]
    Oxen(#[from] harness_oxen::OxenError),
    #[error("the `oxen` CLI is not installed; install it to version your config")]
    OxenUnavailable,
    #[error("could not build client: {0}")]
    Client(String),
    #[error("{0}")]
    Invalid(String),
}
