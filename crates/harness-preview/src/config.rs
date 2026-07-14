//! Remembering what worked: `<workspace>/.oxen-harness/preview.json`.
//!
//! After a server starts successfully its spec is saved here (VS Code
//! launch.json spirit), so a later session — or a "start server" button in the
//! UI — can bring the app back up without re-discovering the command.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Per-project harness directory (shared with skills).
const PROJECT_DIR: &str = ".oxen-harness";
const CONFIG_FILE: &str = "preview.json";

/// A server spec as persisted per project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedServer {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default = "default_true")]
    pub auto_port: bool,
}

fn default_true() -> bool {
    true
}

/// The persisted preview config for one project.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewConfig {
    #[serde(default)]
    pub servers: Vec<SavedServer>,
}

fn config_path(root: &Path) -> PathBuf {
    root.join(PROJECT_DIR).join(CONFIG_FILE)
}

/// Load the project's preview config (empty when absent or unreadable —
/// a corrupt file should never block starting a server).
pub fn load(root: &Path) -> PreviewConfig {
    std::fs::read_to_string(config_path(root))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Upsert `server` (by name) into the project's preview config.
///
/// Written via a temp file + rename so a crash (or two sessions saving at
/// once) can never leave a torn file behind — a half-written config would
/// silently lose the project's remembered start command.
pub fn remember(root: &Path, server: SavedServer) -> std::io::Result<()> {
    let mut config = load(root);
    match config.servers.iter_mut().find(|s| s.name == server.name) {
        Some(existing) => *existing = server,
        None => config.servers.push(server),
    }
    let path = config_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(&config).expect("config serializes");
    let temp = path.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_round_trips_and_upserts() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), PreviewConfig::default());

        let dev = SavedServer {
            name: "dev".into(),
            command: "npm run dev".into(),
            port: None,
            auto_port: true,
        };
        remember(dir.path(), dev.clone()).unwrap();
        assert_eq!(load(dir.path()).servers, vec![dev.clone()]);

        let pinned = SavedServer {
            command: "npm run dev -- --port 4321".into(),
            port: Some(4321),
            auto_port: false,
            ..dev
        };
        remember(dir.path(), pinned.clone()).unwrap();
        assert_eq!(load(dir.path()).servers, vec![pinned]);
    }

    #[test]
    fn corrupt_config_loads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROJECT_DIR);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join(CONFIG_FILE), "{nope").unwrap();
        assert_eq!(load(dir.path()), PreviewConfig::default());
    }
}
