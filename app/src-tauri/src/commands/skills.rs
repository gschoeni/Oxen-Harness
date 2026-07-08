//! Skills — the Settings → Skills page: list, author, delete, and toggle
//! SKILL.md skills (global + project scope, with project shadowing). The
//! discovery/persistence lives in `harness_runtime::skills`; these commands
//! are the webview's thin entry points, scoped to the active project.

use tauri::State;

use crate::state::{active_root, AppState};

/// Every skill visible from the active project (global + project scope, with
/// project shadowing), for the Skills settings page.
#[tauri::command]
pub(crate) async fn list_skills(
    state: State<'_, AppState>,
) -> Result<Vec<harness_runtime::skills::SkillInfo>, String> {
    let root = active_root(&state).await;
    let prefs = harness_runtime::skills::load();
    Ok(harness_runtime::skills::list(&root, &prefs))
}

/// Create or update a skill's SKILL.md. Takes effect for new/resumed chats.
#[tauri::command]
pub(crate) async fn save_skill(
    state: State<'_, AppState>,
    scope: harness_tools::SkillScope,
    name: String,
    description: String,
    instructions: String,
) -> Result<(), String> {
    let root = active_root(&state).await;
    harness_runtime::skills::save_skill(&root, scope, &name, &description, &instructions)
        .map_err(|e| e.to_string())
}

/// Delete a skill's directory (SKILL.md plus any supporting files).
#[tauri::command]
pub(crate) async fn delete_skill(
    state: State<'_, AppState>,
    scope: harness_tools::SkillScope,
    name: String,
) -> Result<(), String> {
    let root = active_root(&state).await;
    harness_runtime::skills::delete_skill(&root, scope, &name).map_err(|e| e.to_string())
}

/// Enable or disable a skill. Takes effect for new/resumed chats.
#[tauri::command]
pub(crate) async fn set_skill_enabled(name: String, enabled: bool) -> Result<(), String> {
    let mut prefs = harness_runtime::skills::load();
    if prefs.set_enabled(&name, enabled) {
        harness_runtime::skills::save(&prefs).map_err(|e| e.to_string())?;
    }
    Ok(())
}
