//! Skills — the Settings → Skills page: list, author, delete, and toggle
//! SKILL.md skills (global + project scope, with project shadowing). The
//! discovery/persistence lives in `harness_runtime::skills`; these commands
//! are the webview's thin entry points, scoped to the active project.

use std::path::Path;

use tauri::State;

use crate::state::AppState;

/// Every skill visible from the active project (global + project scope, with
/// project shadowing), for the Skills settings page.
#[tauri::command]
pub(crate) async fn list_skills(
    state: State<'_, AppState>,
) -> Result<Vec<harness_runtime::skills::SkillInfo>, String> {
    let root = state.active_root().await;
    let prefs = harness_runtime::skills::load();
    Ok(harness_runtime::skills::list(&root, &prefs))
}

/// Run skill file work off the async runtime (a rename can copy a whole
/// folder across volumes; an import unpacks an archive).
async fn blocking<T: Send + 'static>(
    what: &str,
    work: impl FnOnce() -> Result<T, harness_runtime::RuntimeError> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| format!("{what}: {e}"))?
        .map_err(|e| e.to_string())
}

/// Start a new (draft) skill. Fails rather than overwrite an existing one.
#[tauri::command]
pub(crate) async fn create_skill(
    state: State<'_, AppState>,
    scope: harness_tools::SkillScope,
    name: String,
) -> Result<(), String> {
    let root = state.active_root().await;
    blocking("creating skill", move || {
        harness_runtime::skills::create_skill(&root, scope, &name)
    })
    .await
}

/// Rename a skill and/or move it between scopes, supporting files included.
#[tauri::command]
pub(crate) async fn rename_skill(
    state: State<'_, AppState>,
    from_scope: harness_tools::SkillScope,
    from_name: String,
    to_scope: harness_tools::SkillScope,
    to_name: String,
) -> Result<(), String> {
    let root = state.active_root().await;
    blocking("renaming skill", move || {
        harness_runtime::skills::rename_skill(&root, (from_scope, &from_name), (to_scope, &to_name))
    })
    .await
}

/// Autosave an existing skill's SKILL.md (refused once the skill is gone; new
/// skills start with `create_skill`). Takes effect for new/resumed chats.
#[tauri::command]
pub(crate) async fn save_skill(
    state: State<'_, AppState>,
    scope: harness_tools::SkillScope,
    name: String,
    description: String,
    instructions: String,
) -> Result<(), String> {
    let root = state.active_root().await;
    blocking("saving skill", move || {
        harness_runtime::skills::update_skill(&root, scope, &name, &description, &instructions)
    })
    .await
}

/// Export a skill's folder to `path` as a zip (the layout claude.ai imports).
#[tauri::command]
pub(crate) async fn export_skill(
    state: State<'_, AppState>,
    scope: harness_tools::SkillScope,
    name: String,
    path: String,
) -> Result<(), String> {
    let root = state.active_root().await;
    blocking("exporting skill", move || {
        harness_runtime::skills::archive::export_skill(&root, scope, &name, Path::new(&path))
    })
    .await
}

/// Import a skill from a `.zip`, `.skill`, or `SKILL.md` at `path` into
/// `scope`, returning its name.
#[tauri::command]
pub(crate) async fn import_skill(
    state: State<'_, AppState>,
    scope: harness_tools::SkillScope,
    path: String,
) -> Result<String, String> {
    let root = state.active_root().await;
    blocking("importing skill", move || {
        harness_runtime::skills::archive::import_skill(&root, scope, Path::new(&path))
    })
    .await
}

/// Delete a skill's directory (SKILL.md plus any supporting files).
#[tauri::command]
pub(crate) async fn delete_skill(
    state: State<'_, AppState>,
    scope: harness_tools::SkillScope,
    name: String,
) -> Result<(), String> {
    let root = state.active_root().await;
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
