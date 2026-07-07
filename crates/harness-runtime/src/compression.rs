//! The context-compression preference, shared by the CLI and desktop app.
//!
//! One knob (see [`harness_compress::CompressionMode`]): `off` (default),
//! `audit` (measure would-be savings, send requests untouched), or `on`
//! (compress stale tool output before each model call). Persisted to
//! `~/.oxen-harness/compression.json` (versioned, no secrets).
//!
//! Like tool prefs, the mode is read when an agent is built, so a change
//! takes effect for new (and resumed) chats rather than the live one.

use harness_compress::CompressionMode;
use harness_config::paths;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

/// Schema version for `compression.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// Persisted compression preference. Defaults to `off` so a fresh install
/// behaves exactly as before the feature existed.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CompressionPrefs {
    #[serde(default)]
    pub mode: CompressionMode,
}

/// Read the saved preference (defaults to `off` on a fresh install or an
/// unreadable file).
pub fn load() -> CompressionPrefs {
    crate::config::load_or_default(paths::compression_file())
}

/// The saved mode, ready to drop into an `AgentConfig`.
pub fn mode() -> CompressionMode {
    load().mode
}

/// Atomically persist the preference and snapshot the config repo.
pub fn save(prefs: &CompressionPrefs) -> Result<(), RuntimeError> {
    crate::config::write_and_snapshot(
        &paths::compression_file()?,
        SCHEMA_VERSION,
        prefs,
        "Update compression preference",
    )
}

/// Set and persist the mode.
pub fn set_mode(mode: CompressionMode) -> Result<(), RuntimeError> {
    let mut prefs = load();
    prefs.mode = mode;
    save(&prefs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_temp_home;

    #[test]
    fn defaults_to_off_and_round_trips() {
        with_temp_home(|| {
            assert_eq!(mode(), CompressionMode::Off);
            set_mode(CompressionMode::Audit).unwrap();
            assert_eq!(mode(), CompressionMode::Audit);
            set_mode(CompressionMode::On).unwrap();
            assert_eq!(load().mode, CompressionMode::On);
        });
    }
}
