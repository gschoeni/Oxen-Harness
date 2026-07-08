//! Tools — manage which built-in tools the agent may call and the descriptions
//! it sees for them. Preferences persist to `tools.json` and are applied when an
//! agent's registry is built (see [`crate::state::agent_parts`]), so changes
//! take effect for new and resumed chats. The full manageable set comes from
//! [`crate::state::settings_registry`], which mirrors what a real agent gets.

use tauri::{AppHandle, State};

use crate::state::{current_agent, settings_registry, AppState};

/// The tool definitions (JSON schemas) the current session's agent advertises to
/// the model on every call — surfaced in the developer view so the full request
/// (transcript + tools) is inspectable. These aren't persisted per-message, so
/// we read them from the live agent.
#[tauri::command]
pub(crate) async fn tool_definitions(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let arc = current_agent(&app, &state).await?;
    let agent = arc.lock().await;
    Ok(agent.tool_definitions())
}

/// Every manageable tool with its current enabled/override state, for the Tools
/// settings page. Built from a fresh full registry (so disabled tools still
/// appear, toggled off) overlaid with the saved preferences.
#[tauri::command]
pub(crate) async fn list_tools(
    state: State<'_, AppState>,
) -> Result<Vec<harness_runtime::tools::ToolInfo>, String> {
    let registry = settings_registry(&state).await?;
    let prefs = harness_runtime::tools::load();
    Ok(harness_runtime::tools::list(&registry, &prefs))
}

/// Add or update a custom HTTP POST tool. Takes effect for new/resumed chats.
#[tauri::command]
pub(crate) async fn add_custom_tool(
    state: State<'_, AppState>,
    spec: harness_tools::CustomToolSpec,
) -> Result<(), String> {
    let registry = settings_registry(&state).await?;
    harness_runtime::tools::add_custom(spec, &registry).map_err(|e| e.to_string())
}

/// Remove a custom tool. Built-ins cannot be removed, only disabled.
#[tauri::command]
pub(crate) async fn remove_custom_tool(name: String) -> Result<(), String> {
    harness_runtime::tools::remove_custom(&name).map_err(|e| e.to_string())
}

/// Enable or disable a built-in tool. Takes effect for new/resumed chats.
#[tauri::command]
pub(crate) async fn set_tool_enabled(name: String, enabled: bool) -> Result<(), String> {
    let mut prefs = harness_runtime::tools::load();
    if prefs.set_enabled(&name, enabled) {
        harness_runtime::tools::save(&prefs).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Override (or clear, with `None`/blank) the description the model sees for a
/// tool. Takes effect for new/resumed chats.
#[tauri::command]
pub(crate) async fn set_tool_description(
    name: String,
    description: Option<String>,
) -> Result<(), String> {
    let mut prefs = harness_runtime::tools::load();
    if prefs.set_description(&name, description.as_deref()) {
        harness_runtime::tools::save(&prefs).map_err(|e| e.to_string())?;
    }
    Ok(())
}
