//! API keys and other secrets, kept in `~/.oxen-harness/.env`.
//!
//! Secrets are deliberately *not* stored in the versioned JSON config files —
//! those are meant to be safe to commit to an Oxen repo and share. Instead they
//! live in a `.env` file that is loaded into the process environment at startup
//! (via [`dotenvy`]) and excluded from version control.
//!
//! Loading never overrides a variable already present in the real environment,
//! so an explicit `BRAVE_API_KEY=… oxen-harness` on the command line still wins
//! over the saved file.

use std::io::Write;

use crate::{paths, ConfigError};

/// Load `~/.oxen-harness/.env` into the process environment, if it exists.
///
/// Idempotent and best-effort: a missing file, or a process that already has a
/// given variable set, is fine. Call this once at startup, before anything reads
/// `BRAVE_API_KEY` / `OXEN_API_KEY` / friends.
pub fn load() {
    if let Ok(path) = paths::env_file() {
        // `from_path` does not override variables already set in the environment.
        let _ = dotenvy::from_path(&path);
    }
}

/// Read a secret: the live process environment (which [`load`] has populated
/// from `.env`) is the source of truth.
pub fn get(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Persist `key=value` to `.env` (creating it `0600`) and set it in the current
/// process so it takes effect immediately. An empty `value` removes the key.
pub fn set(key: &str, value: &str) -> Result<(), ConfigError> {
    let path = paths::env_file()?;
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        // Preserve comments/blank lines; match `KEY=` (optionally `export KEY=`).
        let trimmed = line.trim_start();
        let body = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let is_key = body
            .split_once('=')
            .map(|(k, _)| k.trim() == key)
            .unwrap_or(false);
        if is_key {
            if !value.is_empty() && !replaced {
                lines.push(format!("{key}={value}"));
                replaced = true;
            }
            // An empty value (or an already-replaced duplicate) drops the line.
        } else {
            lines.push(line.to_string());
        }
    }
    if !value.is_empty() && !replaced {
        lines.push(format!("{key}={value}"));
    }

    let mut body = lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    write_private(&path, body.as_bytes())?;

    // Reflect the change in the running process.
    if value.is_empty() {
        std::env::remove_var(key);
    } else {
        std::env::set_var(key, value);
    }
    Ok(())
}

/// Atomically write `bytes` to `path` with owner-only permissions (`0600` on
/// Unix). Like [`crate::io::atomic_write`] but tightens the mode afterwards so a
/// secrets file is never group/world-readable.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let tmp = path.with_extension(format!("env.tmp.{}", std::process::id()));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|source| ConfigError::Io {
            path: tmp.clone(),
            source,
        })?;
        f.write_all(bytes).map_err(|source| ConfigError::Io {
            path: tmp.clone(),
            source,
        })?;
    }
    std::fs::rename(&tmp, path).map_err(|source| {
        let _ = std::fs::remove_file(&tmp);
        ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_upserts_and_remove_clears() {
        let _lock = crate::testutil::env_guard();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(paths::BASE_DIR_ENV, tmp.path());
        std::env::remove_var("DEMO_KEY");

        set("DEMO_KEY", "first").unwrap();
        assert_eq!(std::env::var("DEMO_KEY").unwrap(), "first");
        let body = std::fs::read_to_string(paths::env_file().unwrap()).unwrap();
        assert_eq!(body, "DEMO_KEY=first\n");

        // Upsert replaces in place rather than appending a duplicate.
        set("DEMO_KEY", "second").unwrap();
        let body = std::fs::read_to_string(paths::env_file().unwrap()).unwrap();
        assert_eq!(body, "DEMO_KEY=second\n");

        // Empty value removes the key and clears it from the process.
        set("DEMO_KEY", "").unwrap();
        assert!(std::env::var("DEMO_KEY").is_err());
        let body = std::fs::read_to_string(paths::env_file().unwrap()).unwrap();
        assert_eq!(body, "");

        std::env::remove_var(paths::BASE_DIR_ENV);
    }

    #[test]
    fn preserves_other_lines_and_comments() {
        let _lock = crate::testutil::env_guard();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(paths::BASE_DIR_ENV, tmp.path());
        let path = paths::env_file().unwrap();
        std::fs::write(&path, "# a comment\nOTHER=keep\nDEMO=old\n").unwrap();

        set("DEMO", "new").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, "# a comment\nOTHER=keep\nDEMO=new\n");

        std::env::remove_var("DEMO");
        std::env::remove_var(paths::BASE_DIR_ENV);
    }
}
