//! Spend limits and cost-routing preferences, shared by the CLI and desktop app.
//!
//! Persisted to `~/.oxen-harness/limits.json` (versioned, no secrets). Like
//! tool prefs, values are read when an agent is built, so a change takes
//! effect for new (and resumed) chats rather than the live one. Everything
//! defaults to "off" — a fresh install enforces nothing and routes nothing.

use harness_config::paths;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

/// Schema version for `limits.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// Persisted limits. All optional: `None` means no limit / no override.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Limits {
    /// Hard per-session spend ceiling in tokens (prompt + completion). A
    /// session that would exceed it stops gracefully with its state saved.
    #[serde(default)]
    pub max_session_tokens: Option<usize>,
    /// The model compaction summaries run on (e.g. a cheap small model).
    /// `None` summarizes with the session model.
    #[serde(default)]
    pub summary_model: Option<String>,
}

/// Read the saved limits (defaults to no limits on a fresh install or an
/// unreadable file).
pub fn load() -> Limits {
    crate::config::load_or_default(paths::limits_file())
}

/// Atomically persist the limits and snapshot the config repo.
pub fn save(limits: &Limits) -> Result<(), RuntimeError> {
    crate::config::write_and_snapshot(
        &paths::limits_file()?,
        SCHEMA_VERSION,
        limits,
        "Update spend limits",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_temp_home;

    #[test]
    fn defaults_enforce_nothing_and_round_trip() {
        with_temp_home(|| {
            let fresh = load();
            assert!(fresh.max_session_tokens.is_none());
            assert!(fresh.summary_model.is_none());

            save(&Limits {
                max_session_tokens: Some(2_000_000),
                summary_model: Some("gemini-2-5-flash".into()),
            })
            .unwrap();
            let loaded = load();
            assert_eq!(loaded.max_session_tokens, Some(2_000_000));
            assert_eq!(loaded.summary_model.as_deref(), Some("gemini-2-5-flash"));
        });
    }
}
