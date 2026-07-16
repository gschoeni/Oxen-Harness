//! Commands driving the live-preview pane: the frontend's placeholder calls
//! these as it mounts/resizes/unmounts, plus toolbar actions (reload, stop,
//! pop-out) and a status query for cold mounts.

use tauri::{AppHandle, State};

use harness_preview::PreviewStatus;

use crate::preview::{self, Bounds};
use crate::state::AppState;

/// Show `session`'s preview webview over the placeholder at `bounds`
/// (CSS pixels). Creates the webview on first call; later calls reposition it.
///
/// Serialized app-wide: the frontend calls this on every layout tick (a
/// splitter drag fires at frame rate), and two concurrent calls at first mount
/// would both find no webview and both try to create one — the loser erroring
/// on the duplicate label, its (newer) bounds lost.
#[tauri::command]
pub(crate) async fn preview_attach(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
    bounds: Bounds,
) -> Result<(), String> {
    let _guard = state.preview_attach.lock().await;
    let server = state
        .dev_servers
        .get(&session)
        .ok_or_else(|| "no dev server for this session".to_string())?;
    let url = server
        .status()
        .url
        .ok_or_else(|| "dev server has no URL yet".to_string())?;
    // The console bridge must exist before the webview is created — its port
    // is baked into the initialization script (which can only be set at
    // creation), so a bridge failure here would permanently blind the console
    // feed and the Fix-it banner for this session. Fail loudly instead.
    let console_port = preview::console_bridge(&app)
        .await
        .map_err(|e| format!("could not start the preview console bridge: {e}"))?
        .port();
    preview::attach(&app, &session, &url, &bounds, Some(console_port))
}

/// Hide all preview webviews (tab switch, overlay opened, pane closed).
#[tauri::command]
pub(crate) fn preview_detach(app: AppHandle) {
    preview::detach_all(&app);
}

/// Reload the session's preview page.
#[tauri::command]
pub(crate) fn preview_reload(app: AppHandle, session: String) {
    preview::reload(&app, &session);
}


/// Stop the session's dev server (toolbar stop button) and drop its webview.
#[tauri::command]
pub(crate) async fn preview_stop(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
) -> Result<(), String> {
    state.dev_servers.stop(&session).await;
    preview::close(&app, &session);
    Ok(())
}

/// Open the session's running app in the system browser (pop-out button).
#[tauri::command]
pub(crate) fn preview_open_external(
    state: State<'_, AppState>,
    session: String,
) -> Result<(), String> {
    let url = state
        .dev_servers
        .get(&session)
        .and_then(|s| s.status().url)
        .ok_or_else(|| "no running dev server".to_string())?;
    preview::open_external(&url)
}

/// The session's dev-server status, if one was started — lets a freshly
/// mounted UI (or a resumed chat) sync without waiting for the next event.
#[tauri::command]
pub(crate) fn preview_status(
    state: State<'_, AppState>,
    session: String,
) -> Option<PreviewStatus> {
    state.dev_servers.get(&session).map(|s| s.status())
}

/// Statuses of every session's dev server (sidebar chips, settings page).
#[tauri::command]
pub(crate) fn preview_statuses(state: State<'_, AppState>) -> Vec<(String, PreviewStatus)> {
    state.dev_servers.statuses()
}

/// The persisted live-preview preferences (Settings → Preview).
#[tauri::command]
pub(crate) fn get_preview_prefs() -> harness_runtime::preview::PreviewPrefs {
    harness_runtime::preview::load()
}

/// Persist the auto-verify flag; applies to newly built (or resumed) agents.
#[tauri::command]
pub(crate) fn set_preview_auto_verify(auto_verify: bool) -> Result<(), String> {
    harness_runtime::preview::set_auto_verify(auto_verify).map_err(|e| e.to_string())
}

/// Restart the session's dev server: reuse the last spec it ran with, falling
/// back to the project's saved preview config (the pane's Restart button after
/// a crash — no agent turn or tokens involved).
#[tauri::command]
pub(crate) async fn preview_restart(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
) -> Result<(), String> {
    let root = state.session_workspace(&session);
    let spec = state
        .dev_servers
        .get(&session)
        .map(|s| s.spec().clone())
        .or_else(|| {
            harness_preview::config::load(&root)
                .servers
                .first()
                .map(|saved| harness_preview::ServerSpec {
                    name: saved.name.clone(),
                    command: saved.command.clone(),
                    port: saved.port,
                    auto_port: saved.auto_port,
                })
        })
        .ok_or_else(|| "no saved dev-server command for this project".to_string())?;
    let sink = std::sync::Arc::new(crate::preview::TauriPreviewSink {
        app: app.clone(),
        session: session.clone(),
    });
    state
        .dev_servers
        .start(
            &session,
            spec,
            &root,
            sink,
            harness_preview::DEFAULT_READY_TIMEOUT,
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
