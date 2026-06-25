//! Atomic, versioned config IO.
//!
//! Two problems this module solves for every JSON config file we keep:
//!
//! 1. **Torn writes.** A naive `fs::write` that's interrupted (crash, full disk,
//!    power loss) can leave a half-written, unparseable file. We always write to
//!    a sibling temp file and `rename` it into place — `rename` is atomic on the
//!    same filesystem, so readers see either the old file or the new one.
//!
//! 2. **Schema evolution.** Each file is wrapped in a [`Versioned`] envelope that
//!    records a `schema_version`. Old files written before versioning existed
//!    simply have no field and read back as version `0`, so a caller can detect
//!    "pre-versioning" data and migrate it forward.

use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// Version assigned to a config file that predates versioning (no
/// `schema_version` field on disk).
pub const UNVERSIONED: u32 = 0;

/// A config payload tagged with the schema version it was written under.
///
/// `#[serde(flatten)]` keeps the JSON flat — the version sits alongside the
/// payload's own fields rather than nesting it under a key — so adding the
/// envelope to a previously bare struct is backward compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Versioned<T> {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(flatten)]
    pub data: T,
}

/// Atomically write `bytes` to `path` via a temp file + rename. Creates parent
/// directories as needed.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    // Temp file in the same directory so the rename stays on one filesystem.
    let tmp = path.with_extension(tmp_extension(path));
    std::fs::write(&tmp, bytes).map_err(|source| ConfigError::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| {
        // Best-effort cleanup; the rename error is what matters.
        let _ = std::fs::remove_file(&tmp);
        ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// A unique-ish temp extension that keeps the original extension visible, e.g.
/// `connection.json` → `connection.json.tmp`. The pid keeps concurrent writers
/// from colliding on the same temp path.
fn tmp_extension(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    if ext.is_empty() {
        format!("tmp.{}", std::process::id())
    } else {
        format!("{ext}.tmp.{}", std::process::id())
    }
}

/// Read a versioned JSON config, returning both the on-disk schema version and
/// the payload. A missing or unparseable file yields `(UNVERSIONED, default)`,
/// so config is never a hard failure — the caller falls back to defaults.
pub fn read_versioned<T: DeserializeOwned + Default>(path: &Path) -> (u32, T) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (UNVERSIONED, T::default());
    };
    match serde_json::from_str::<Versioned<T>>(&text) {
        Ok(v) => (v.schema_version, v.data),
        Err(_) => (UNVERSIONED, T::default()),
    }
}

/// Atomically write a versioned JSON config (pretty-printed, trailing newline).
pub fn write_versioned<T: Serialize>(
    path: &Path,
    schema_version: u32,
    data: &T,
) -> Result<(), ConfigError> {
    let env = Versioned {
        schema_version,
        data,
    };
    let mut json = serde_json::to_string_pretty(&env)?;
    json.push('\n');
    atomic_write(path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Demo {
        host: String,
        #[serde(default)]
        count: u32,
    }

    #[test]
    fn round_trips_with_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.json");
        let demo = Demo {
            host: "hub.oxen.ai".into(),
            count: 3,
        };
        write_versioned(&path, 2, &demo).unwrap();

        let (ver, got) = read_versioned::<Demo>(&path);
        assert_eq!(ver, 2);
        assert_eq!(got, demo);
    }

    #[test]
    fn bare_file_reads_as_unversioned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.json");
        // A pre-versioning file: payload fields, no schema_version.
        std::fs::write(&path, r#"{"host":"old","count":1}"#).unwrap();

        let (ver, got) = read_versioned::<Demo>(&path);
        assert_eq!(ver, UNVERSIONED);
        assert_eq!(got.host, "old");
        assert_eq!(got.count, 1);
    }

    #[test]
    fn missing_or_garbage_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.json");
        assert_eq!(
            read_versioned::<Demo>(&missing),
            (UNVERSIONED, Demo::default())
        );

        let garbage = dir.path().join("bad.json");
        std::fs::write(&garbage, "not json{{").unwrap();
        assert_eq!(
            read_versioned::<Demo>(&garbage),
            (UNVERSIONED, Demo::default())
        );
    }

    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        atomic_write(&path, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        // Only the final file remains in the directory.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("x.json")]);
    }
}
