//! Canonical on-disk locations for everything oxen-harness stores.
//!
//! Historically each crate and front end re-derived `~/.oxen-harness` from
//! `home_dir()`. That meant the base directory lived in a dozen places and could
//! drift. This module is the single source of truth: everything else asks here.
//!
//! The base directory can be overridden with the `OXEN_HARNESS_DIR` environment
//! variable — useful for tests, sandboxes, and users who keep state elsewhere.

use std::path::PathBuf;

use crate::ConfigError;

/// Name of the base directory under the user's home (`~/.oxen-harness`).
pub const BASE_DIR_NAME: &str = ".oxen-harness";

/// Environment variable that overrides the base directory entirely.
pub const BASE_DIR_ENV: &str = "OXEN_HARNESS_DIR";

/// The base directory (`~/.oxen-harness`, or `$OXEN_HARNESS_DIR`), without
/// creating it. Returns `None` only if the home directory can't be resolved and
/// no override is set.
pub fn base_dir_unchecked() -> Option<PathBuf> {
    if let Some(over) = std::env::var_os(BASE_DIR_ENV) {
        if !over.is_empty() {
            return Some(PathBuf::from(over));
        }
    }
    dirs::home_dir().map(|h| h.join(BASE_DIR_NAME))
}

/// The base directory, creating it (and its parents) if needed.
pub fn base_dir() -> Result<PathBuf, ConfigError> {
    let dir = base_dir_unchecked().ok_or(ConfigError::NoHome)?;
    std::fs::create_dir_all(&dir).map_err(|source| ConfigError::Io {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// A child path under the (created) base directory.
fn under(child: &str) -> Result<PathBuf, ConfigError> {
    Ok(base_dir()?.join(child))
}

/// `~/.oxen-harness/history.sqlite` — the chat transcript database.
pub fn history_db() -> Result<PathBuf, ConfigError> {
    under("history.sqlite")
}

/// `~/.oxen-harness/connection.json` — provider host + non-secret settings.
pub fn connection_file() -> Result<PathBuf, ConfigError> {
    under("connection.json")
}

/// `~/.oxen-harness/projects.json` — known projects and the active one.
pub fn projects_file() -> Result<PathBuf, ConfigError> {
    under("projects.json")
}

/// `~/.oxen-harness/models.json` — the cloud model catalog (custom models the
/// user added) and the currently selected default.
pub fn models_file() -> Result<PathBuf, ConfigError> {
    under("models.json")
}

/// `~/.oxen-harness/tools.json` — per-tool preferences (disabled tools and
/// description overrides) applied when building an agent's tool registry.
pub fn tools_file() -> Result<PathBuf, ConfigError> {
    under("tools.json")
}

/// `~/.oxen-harness/compression.json` — the context-compression mode
/// (off/audit/on) applied when building an agent.
pub fn compression_file() -> Result<PathBuf, ConfigError> {
    under("compression.json")
}

/// `~/.oxen-harness/preview.json` — live-preview preferences (auto-verify).
/// Distinct from the *per-project* `<workspace>/.oxen-harness/preview.json`
/// that remembers a project's dev-server command.
pub fn preview_file() -> Result<PathBuf, ConfigError> {
    under("preview.json")
}

/// `~/.oxen-harness/config.toml` — the active theme selection.
pub fn config_file() -> Result<PathBuf, ConfigError> {
    under("config.toml")
}

/// `~/.oxen-harness/.env` — API keys and other secrets (never versioned).
pub fn env_file() -> Result<PathBuf, ConfigError> {
    under(".env")
}

/// `~/.oxen-harness/prompt_history.txt` — the CLI readline history.
pub fn prompt_history_file() -> Result<PathBuf, ConfigError> {
    under("prompt_history.txt")
}

/// `~/.oxen-harness/errors.jsonl` — the developer error log: one JSON entry
/// per model-call retry attempt and per failed turn (timestamp, session,
/// model, endpoint, error), appended by every host so failures stay
/// debuggable after the UI moved on.
pub fn errors_log() -> Result<PathBuf, ConfigError> {
    under("errors.jsonl")
}

/// `~/.oxen-harness/requests.jsonl` — the developer request log: one JSON
/// entry per model call (prompt size, cache-prefix classification, latency,
/// retries, and the provider's reported usage including cached tokens),
/// appended by every host so cost and cache behavior stay diagnosable.
pub fn requests_log() -> Result<PathBuf, ConfigError> {
    under("requests.jsonl")
}

/// `~/.oxen-harness/limits.json` — spend limits and cost-routing preferences:
/// the per-session token budget and the cheaper model compaction summaries
/// run on.
pub fn limits_file() -> Result<PathBuf, ConfigError> {
    under("limits.json")
}

/// `~/.oxen-harness/model-limits.json` — a cache of API-reported per-model
/// capabilities (`context_length`, `max_output_tokens`), refreshed on every
/// hosted-catalog fetch and read when an agent is built so context budgeting
/// uses the model's real window rather than a name-derived guess. A cache,
/// not a user setting — distinct from `limits.json` (spend limits).
pub fn model_limits_file() -> Result<PathBuf, ConfigError> {
    under("model-limits.json")
}

/// `~/.oxen-harness/last-crash` — the marker a fatal-signal handler leaves
/// behind when a run dies on SIGSEGV/SIGBUS/etc., read (and consumed) by the
/// next launch so the crash gets reported instead of vanishing (see the
/// `harness-crash` crate). Absent after every clean exit.
pub fn last_crash_file() -> Result<PathBuf, ConfigError> {
    under("last-crash")
}

/// `~/.oxen-harness/themes/` — custom + exported themes.
pub fn themes_dir() -> Result<PathBuf, ConfigError> {
    let dir = under("themes")?;
    std::fs::create_dir_all(&dir).map_err(|source| ConfigError::Io {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// `~/.oxen-harness/code-review.json` — the code-review pipeline: the ordered
/// step prompts `/code-review` runs and its output cap.
pub fn code_review_file() -> Result<PathBuf, ConfigError> {
    under("code-review.json")
}

/// `~/.oxen-harness/permissions.json` — the global permission mode and
/// allow/deny command rules for the tool-call gate. Distinct from the
/// *per-project* `<workspace>/.oxen-harness/permissions.json` where "always
/// allow for this project" grants persist.
pub fn permissions_file() -> Result<PathBuf, ConfigError> {
    under("permissions.json")
}

/// `~/.oxen-harness/permissions.jsonl` — the gate's audit log: one JSON entry
/// per permission decision (what ran or was refused, and which rule/choice
/// decided it).
pub fn permissions_log() -> Result<PathBuf, ConfigError> {
    under("permissions.jsonl")
}

/// `~/.oxen-harness/trash/` — where "move to trash instead" relocates files a
/// command would have deleted (pruned after a TTL by the permission gate).
pub fn trash_dir() -> Result<PathBuf, ConfigError> {
    under("trash")
}

/// `~/.oxen-harness/snapshots/` — shadow git repositories holding pre-command
/// workspace snapshots taken before approved dangerous commands.
pub fn snapshots_dir() -> Result<PathBuf, ConfigError> {
    under("snapshots")
}

/// `~/.oxen-harness/skills.json` — skill preferences (disabled skills).
pub fn skills_file() -> Result<PathBuf, ConfigError> {
    under("skills.json")
}

/// `~/.oxen-harness/skills/` — global skills (one directory per skill, each
/// holding a `SKILL.md`), available in every project.
pub fn skills_dir() -> Result<PathBuf, ConfigError> {
    let dir = under("skills")?;
    std::fs::create_dir_all(&dir).map_err(|source| ConfigError::Io {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// `~/.oxen-harness/loops/` — shareable loop specs + run journals.
pub fn loops_dir() -> Result<PathBuf, ConfigError> {
    let dir = under("loops")?;
    std::fs::create_dir_all(&dir).map_err(|source| ConfigError::Io {
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// `~/.oxen-harness/models/` — downloaded local model weights.
pub fn models_dir() -> Result<PathBuf, ConfigError> {
    under("models")
}

/// `~/.oxen-harness/local-models.json` — user additions and overrides to the
/// curated local (llama.cpp) model catalog.
pub fn local_models_file() -> Result<PathBuf, ConfigError> {
    under("local-models.json")
}

/// `~/.oxen-harness/canvas/` — canvas documents written by the CLI.
pub fn canvas_dir() -> Result<PathBuf, ConfigError> {
    under("canvas")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guard that points the base dir at a temp directory for the duration of
    /// a test and restores the previous value on drop. Tests touching the env
    /// must not run concurrently with each other; cargo runs them in-process on
    /// separate threads, so we serialize via a mutex in the test module.
    struct DirGuard(Option<std::ffi::OsString>);

    impl DirGuard {
        fn set(dir: &std::path::Path) -> Self {
            let prev = std::env::var_os(BASE_DIR_ENV);
            std::env::set_var(BASE_DIR_ENV, dir);
            DirGuard(prev)
        }
    }

    impl Drop for DirGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var(BASE_DIR_ENV, v),
                None => std::env::remove_var(BASE_DIR_ENV),
            }
        }
    }

    #[test]
    fn override_redirects_base_dir_and_creates_children() {
        let _lock = crate::testutil::env_guard();
        let tmp = tempfile::tempdir().unwrap();
        let _g = DirGuard::set(tmp.path());

        assert_eq!(base_dir_unchecked().unwrap(), tmp.path());
        // history_db creates the base dir; themes_dir creates its subdir.
        assert_eq!(history_db().unwrap(), tmp.path().join("history.sqlite"));
        let themes = themes_dir().unwrap();
        assert!(themes.is_dir());
        assert_eq!(themes, tmp.path().join("themes"));
    }
}
