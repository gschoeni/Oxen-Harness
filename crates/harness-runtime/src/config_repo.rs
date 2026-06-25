//! Versioning the config directory (`~/.oxen-harness`) with Oxen.
//!
//! Opt-in: the directory only becomes an Oxen repo when the user runs
//! [`init`]. Once it is one, [`snapshot`] commits subsequent config changes.
//! Both are best-effort from the callers' perspective — config writes never fail
//! because Oxen isn't installed or versioning isn't enabled.

use harness_oxen::Oxen;

use crate::RuntimeError;

/// Initialize `~/.oxen-harness` as an Oxen repository and commit the current
/// config, so future changes can be versioned and shared.
pub fn init() -> Result<(), RuntimeError> {
    let dir = harness_config::base_dir()?;
    let oxen = Oxen::new();
    if !oxen.is_available() {
        return Err(RuntimeError::OxenUnavailable);
    }
    oxen.snapshot(&dir, "Initialize oxen-harness config")?;
    Ok(())
}

/// Whether the config directory is already an Oxen repository.
pub fn is_versioned() -> bool {
    match harness_config::base_dir_unchecked() {
        Some(dir) => Oxen::new().is_repo(&dir),
        None => false,
    }
}

/// Commit the current config state if the directory is an Oxen repo and the
/// `oxen` binary is available. A no-op otherwise — callers invoke this after a
/// config write and ignore the result.
pub fn snapshot(message: &str) {
    let Some(dir) = harness_config::base_dir_unchecked() else {
        return;
    };
    let oxen = Oxen::new();
    if oxen.is_repo(&dir) && oxen.is_available() {
        let _ = oxen.snapshot(&dir, message);
    }
}
