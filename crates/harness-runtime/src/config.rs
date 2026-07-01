//! Shared lifecycle for the small JSON config files this crate owns
//! (`connection.json`, `models.json`, `tools.json`).
//!
//! Every one of them follows the same shape: read a versioned payload — falling
//! back to defaults when it's missing or unreadable — and, after any change,
//! write it atomically and snapshot the config repo. These two helpers capture
//! that shape so each config module is left with just its schema and its own
//! domain logic.

use std::path::{Path, PathBuf};

use harness_config::io::{read_versioned, write_versioned};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::{config_repo, RuntimeError};

/// Load a versioned config payload, falling back to `T::default()` when the path
/// can't be resolved (no home dir) or the file is absent/unreadable. Config is
/// never a hard failure — a fresh install just reads as defaults.
pub(crate) fn load_or_default<T, E>(path: Result<PathBuf, E>) -> T
where
    T: DeserializeOwned + Default,
{
    path.map(|p| read_versioned::<T>(&p).1).unwrap_or_default()
}

/// Atomically persist a config payload and snapshot the config repo with
/// `message`, so every settings change is one revision in the versioned config.
pub(crate) fn write_and_snapshot<T: Serialize>(
    path: &Path,
    schema_version: u32,
    value: &T,
    message: &str,
) -> Result<(), RuntimeError> {
    write_versioned(path, schema_version, value)?;
    config_repo::snapshot(message);
    Ok(())
}
