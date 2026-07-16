//! Commands driving the link-browser pane (a chat link clicked open in the
//! side panel): the frontend's placeholder calls these as it mounts, resizes,
//! and unmounts, plus its toolbar actions (reload, pop-out, close).

use tauri::{AppHandle, State};

use crate::browser;
use crate::preview::Bounds;
use crate::state::AppState;

/// Show the browser webview over the placeholder at `bounds` (CSS pixels),
/// pointed at `url`. Creates the webview on first call; later calls navigate
/// and/or reposition it.
///
/// Serialized app-wide for the same reason as `preview_attach`: the frontend
/// calls this on every layout tick, and two concurrent first-mount calls would
/// both try to create the webview.
#[tauri::command]
pub(crate) async fn browser_attach(
    app: AppHandle,
    state: State<'_, AppState>,
    url: String,
    bounds: Bounds,
) -> Result<(), String> {
    let _guard = state.preview_attach.lock().await;
    browser::attach(&app, &url, &bounds)
}

/// Hide the browser webview (pane unmounted, overlay opened, tab switched).
#[tauri::command]
pub(crate) fn browser_detach(app: AppHandle) {
    browser::detach(&app);
}

/// Destroy the browser webview (the user closed the pane).
#[tauri::command]
pub(crate) fn browser_close(app: AppHandle) {
    browser::close(&app);
}

/// Reload the pane's current page.
#[tauri::command]
pub(crate) fn browser_reload(app: AppHandle) {
    browser::reload(&app);
}

/// Open `url` in the system browser — the pane's pop-out button (which sends
/// the page the pane is showing) and non-web links (mailto:) alike.
#[tauri::command]
pub(crate) fn open_external(app: AppHandle, url: Option<String>) -> Result<(), String> {
    let url = url
        .or_else(|| browser::current_url(&app))
        .ok_or_else(|| "nothing to open".to_string())?;
    crate::preview::open_external(&url)
}
