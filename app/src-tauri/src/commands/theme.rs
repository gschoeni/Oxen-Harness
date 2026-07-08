//! Themes — list, switch, import/export, and vibe-code a new one via the model.
//! Theme storage/resolution lives in `harness_theme`; generation reuses the
//! session's model + endpoint through a one-shot completion that never touches
//! (or waits on) a chat's agent.

use harness_llm::{ChatMessage, ChatRequest};
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::state::{client_for, AppState};

fn theme_store() -> Result<harness_theme::Store, String> {
    harness_theme::Store::open().map_err(|e| e.to_string())
}

/// A one-shot, agent-free model completion using the active model + endpoint.
/// Used for side tasks (theme generation) so they never block — or wait on — a
/// chat's agent, which may be mid-turn.
async fn complete_oneshot(
    app: &AppHandle,
    state: &AppState,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let (client, model, _) = client_for(app, state).await?;
    let request = ChatRequest::new(
        &model,
        vec![
            ChatMessage::system(system.to_string()),
            ChatMessage::user(user.to_string()),
        ],
    )
    .streaming(true);
    let assembled = client
        .stream_chat(&request, &CancellationToken::new(), |_| {})
        .await
        .map_err(|e| e.to_string())?;
    Ok(assembled.content)
}

/// All available themes (built-in + installed), with the active one marked.
#[tauri::command]
pub(crate) async fn list_themes() -> Result<Vec<harness_theme::store::ThemeSummary>, String> {
    Ok(theme_store()?.list())
}

/// The full active theme (palette + voice) for the UI to apply.
#[tauri::command]
pub(crate) async fn active_theme() -> Result<harness_theme::Theme, String> {
    Ok(theme_store()?.load_active())
}

/// Switch the active theme; returns the resolved theme so the UI can re-skin.
#[tauri::command]
pub(crate) async fn use_theme(name: String) -> Result<harness_theme::Theme, String> {
    theme_store()?.set_active(&name).map_err(|e| e.to_string())
}

/// Install a theme from pasted/loaded TOML or JSON, then activate it.
#[tauri::command]
pub(crate) async fn import_theme(contents: String) -> Result<harness_theme::Theme, String> {
    let store = theme_store()?;
    let theme = store
        .install_from_str(&contents)
        .map_err(|e| e.to_string())?;
    store
        .set_active(&theme.meta.name)
        .map_err(|e| e.to_string())
}

/// Export a theme as a shareable TOML document.
#[tauri::command]
pub(crate) async fn export_theme(name: String) -> Result<String, String> {
    let theme = theme_store()?.resolve(&name).map_err(|e| e.to_string())?;
    theme.to_toml().map_err(|e| e.to_string())
}

/// Remove an installed theme (built-ins always remain).
#[tauri::command]
pub(crate) async fn remove_theme(name: String) -> Result<(), String> {
    theme_store()?.remove(&name).map_err(|e| e.to_string())
}

/// Vibe-code a new theme: send the brief to the model, parse its output, save
/// and activate it. Reuses the session's model + endpoint.
#[tauri::command]
pub(crate) async fn new_theme(
    app: AppHandle,
    state: State<'_, AppState>,
    brief: String,
) -> Result<harness_theme::Theme, String> {
    let raw = complete_oneshot(
        &app,
        &state,
        &harness_theme::Theme::generation_system_prompt(),
        &brief,
    )
    .await?;
    let theme = harness_theme::Theme::from_model_output(&raw).map_err(|e| e.to_string())?;
    let store = theme_store()?;
    store.save(&theme).map_err(|e| e.to_string())?;
    store
        .set_active(&theme.meta.name)
        .map_err(|e| e.to_string())
}
