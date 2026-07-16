//! Connection settings — a persisted Oxen API key + host override, editable
//! from the desktop Settings page, plus the context-compression mode toggle.
//! Blank fields fall back to env / CLI login.
//!
//! Connection settings live in `harness_runtime::connection`: the non-secret
//! host in `connection.json`, the API/Brave keys in `~/.oxen-harness/.env`.
//! The commands here are thin wrappers so the CLI and desktop resolve a client
//! the same way (no drift) and secrets stay out of the versioned config.

use harness_protocol::SessionInfo;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub(crate) fn get_connection() -> harness_runtime::connection::ConnectionView {
    harness_runtime::connection::view()
}

/// Save just the Brave Search API key and apply it to the running agent.
///
/// Unlike [`set_connection`], this does **not** rebuild the agent or start a new
/// session — it persists the key (to `.env`) and sets `BRAVE_API_KEY` in the
/// process so the already-registered `web_search` tool picks it up on its next
/// call. Lets the user fix a failed web search inline and retry in the same chat.
#[tauri::command]
pub(crate) fn configure_brave_key(key: String) -> Result<(), String> {
    harness_runtime::connection::set_brave_key(&key).map_err(|e| e.to_string())
}

/// Save the Oxen API key and authenticate a chat's running agent in place.
///
/// Unlike [`set_connection`], this does **not** start a new session — it persists
/// the key (to `.env`) and swaps a freshly-built client (same model, now carrying
/// the key) into the session's agent, keeping the transcript intact. Lets the
/// user paste a key inline after a 401 and retry the turn in the same chat.
#[tauri::command]
pub(crate) async fn configure_oxen_key(
    state: State<'_, AppState>,
    session: String,
    key: String,
) -> Result<(), String> {
    harness_runtime::connection::set_oxen_key(&key).map_err(|e| e.to_string())?;
    state.refresh_client(&session).await
}

/// Save the Oxen API key + host and rebuild the agent against the new endpoint.
///
/// Rebuilding validates that a key resolves (a blank key must be backed by env /
/// CLI login), drops any active local-model server, and — since the endpoint may
/// have changed — starts a fresh session. Returns the new session info.
#[tauri::command]
pub(crate) async fn set_connection(
    state: State<'_, AppState>,
    host: String,
    api_key: String,
    brave_api_key: String,
) -> Result<SessionInfo, String> {
    harness_runtime::connection::save(&host, &api_key, &brave_api_key)
        .map_err(|e| e.to_string())?;

    // A connection change drops any local model and starts fresh on the cloud
    // endpoint using the selected cloud model.
    *state.local_server.lock().await = None;
    *state.local_model.lock().await = None;
    let _ = harness_runtime::models::set_active_local("");
    state.new_session().await
}

/// The persisted context-compression mode: `"off"`, `"audit"`, or `"on"`.
#[tauri::command]
pub(crate) async fn get_compression_mode() -> String {
    harness_runtime::compression::mode().as_str().to_string()
}

/// Set the context-compression mode: persist it for new chats AND apply it to
/// the live conversation in place (mirroring `set_model`), so a meter toggle
/// takes effect on the very next model call. Returns the refreshed session
/// info carrying the now-current mode.
#[tauri::command]
pub(crate) async fn set_compression_mode(
    state: State<'_, AppState>,
    mode: String,
) -> Result<SessionInfo, String> {
    let mode = harness_compress::CompressionMode::from_str_or_off(&mode);
    state.set_compression_mode(mode).await
}
