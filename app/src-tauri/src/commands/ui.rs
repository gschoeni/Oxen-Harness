//! Desktop UI preferences (`~/.oxen-harness/ui.json`): color mode, dock
//! layout, home view, and similar webview-only state. Persisted in the harness
//! base dir — not the webview's localStorage — so `OXEN_HARNESS_DIR` relocates
//! or resets the whole app in one move. The backend treats the contents as an
//! opaque JSON object; the frontend (`lib/uiState.ts`) owns the keys.

use harness_config::paths;

/// The saved UI preferences, or `None` on first run (no `ui.json` yet, or one
/// that doesn't parse — either way the frontend starts from defaults).
#[tauri::command]
pub(crate) fn load_ui_state() -> Result<Option<serde_json::Value>, String> {
    let path = paths::ui_state_file().map_err(|e| e.to_string())?;
    match std::fs::read_to_string(&path) {
        Ok(raw) => Ok(serde_json::from_str(&raw).ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Persist the full UI preferences object (the frontend always saves the
/// whole state, never a patch).
#[tauri::command]
pub(crate) fn save_ui_state(state: serde_json::Value) -> Result<(), String> {
    let path = paths::ui_state_file().map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    std::fs::write(&path, raw).map_err(|e| e.to_string())
}
