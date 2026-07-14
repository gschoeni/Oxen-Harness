//! Live-preview preferences, shared by the CLI and desktop app.
//!
//! One knob for now: `auto_verify` (default **on**) — whether the model is
//! nudged to look at the running app (screenshot + console) after each batch
//! of edits before reporting done. Persisted to `~/.oxen-harness/preview.json`
//! (versioned, no secrets). Read when an agent is built, so a change takes
//! effect for new (and resumed) chats rather than the live one.

use harness_config::paths;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

/// Schema version for `preview.json`.
pub const SCHEMA_VERSION: u32 = 1;

fn default_true() -> bool {
    true
}

/// Persisted preview preferences. Auto-verify defaults to on — it's the
/// behavior that makes the preview trustworthy for people who can't read the
/// code, at the cost of some tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewPrefs {
    #[serde(default = "default_true")]
    pub auto_verify: bool,
}

impl Default for PreviewPrefs {
    fn default() -> Self {
        Self { auto_verify: true }
    }
}

/// Read the saved preferences (defaults on a fresh install / unreadable file).
pub fn load() -> PreviewPrefs {
    crate::config::load_or_default(paths::preview_file())
}

/// Whether the agent should be nudged to verify its work in the preview.
pub fn auto_verify() -> bool {
    load().auto_verify
}

/// Atomically persist the preferences and snapshot the config repo.
pub fn save(prefs: &PreviewPrefs) -> Result<(), RuntimeError> {
    crate::config::write_and_snapshot(
        &paths::preview_file()?,
        SCHEMA_VERSION,
        prefs,
        "Update preview preference",
    )
}

/// Set and persist the auto-verify flag.
pub fn set_auto_verify(auto_verify: bool) -> Result<(), RuntimeError> {
    let mut prefs = load();
    prefs.auto_verify = auto_verify;
    save(&prefs)
}
