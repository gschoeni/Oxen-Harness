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
//! - [`models`] — the cloud-model catalog (built-ins + user additions).
//! - [`tools`] — per-tool preferences (enable/disable, description overrides)
//!   and user-defined custom HTTP tools.
//! - [`skills`] — skill discovery (global + per-project `SKILL.md` dirs),
//!   preferences, and authoring.

mod config;
pub mod config_repo;
pub mod connection;
pub mod models;
pub mod skills;
pub mod tools;

/// A process-wide lock serializing tests that mutate global env vars / the shared
/// config dir (`connection` and `models` both redirect `OXEN_HARNESS_DIR`). Cargo
/// runs a crate's tests on multiple threads in one process, so without this they
/// race on that env var.
#[cfg(test)]
pub(crate) static TEST_ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` against a fresh, isolated `~/.oxen-harness` (a tempdir) with the
/// secret env vars cleared, holding [`TEST_ENV_GUARD`] for the duration so the
/// process-global env doesn't race with other tests. Shared by the `connection`
/// and `models` test suites, which both persist to that directory.
#[cfg(test)]
pub(crate) fn with_temp_home<T>(f: impl FnOnce() -> T) -> T {
    use harness_config::paths::BASE_DIR_ENV;
    use harness_llm::auth::API_KEY_ENV;
    use harness_tools::web::BRAVE_API_KEY_ENV;

    let _lock = TEST_ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var(BASE_DIR_ENV, tmp.path());
    std::env::remove_var(API_KEY_ENV);
    std::env::remove_var(BRAVE_API_KEY_ENV);
    let out = f();
    std::env::remove_var(BASE_DIR_ENV);
    std::env::remove_var(API_KEY_ENV);
    std::env::remove_var(BRAVE_API_KEY_ENV);
    out
}

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
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
