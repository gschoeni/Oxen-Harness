//! Commands for image/video generation: the project's media library (the
//! gallery's cold load; live updates arrive on `media://changed`), cancel,
//! the media preferences page, and the hub's model catalog for pickers.

use tauri::State;

use crate::state::AppState;

/// The project's generations and in-flight jobs, newest first.
#[tauri::command]
pub(crate) fn list_media(
    state: State<'_, AppState>,
    root: String,
) -> Result<Vec<harness_protocol::MediaItem>, String> {
    Ok(state.list_media(std::path::Path::new(&root)))
}

/// Cancel an in-flight generation by its hub id.
#[tauri::command]
pub(crate) async fn cancel_media(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.cancel_media(&id).await
}

/// The saved media preferences (default models, output folder, budgets).
#[tauri::command]
pub(crate) fn get_media_prefs() -> harness_runtime::media::MediaPrefs {
    harness_runtime::media::load()
}

/// Persist the media preferences; applies to newly built (or resumed) agents.
#[tauri::command]
pub(crate) fn set_media_prefs(prefs: harness_runtime::media::MediaPrefs) -> Result<(), String> {
    harness_runtime::media::save(&prefs).map_err(|e| e.to_string())
}

/// The hub's image/video models (`kind` = `image` | `video` | omitted).
/// `refresh` refetches from the hub instead of the day-long cache.
#[tauri::command]
pub(crate) async fn list_media_models(
    state: State<'_, AppState>,
    kind: Option<String>,
    refresh: Option<bool>,
) -> Result<Vec<harness_protocol::MediaModelSummary>, String> {
    state
        .media_models(kind.as_deref(), refresh.unwrap_or(false))
        .await
}
