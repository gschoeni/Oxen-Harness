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

/// System memory, this app's local model servers, and the last memory
/// notice — the memory meter polls it while a local model is in use.
// The probes and disk walks below are blocking work: off the main thread
// (sync commands run there) and off the async workers.
#[tauri::command]
pub(crate) async fn memory_status() -> Result<harness_protocol::MemoryStatus, String> {
    tauri::async_runtime::spawn_blocking(harness_host::local_media::memory_status)
        .await
        .map_err(|e| e.to_string())
}

/// The on-device image models, their engine, and free disk.
#[tauri::command]
pub(crate) async fn local_media_status() -> Result<harness_protocol::LocalMediaStatus, String> {
    tauri::async_runtime::spawn_blocking(harness_host::local_media::local_media_status)
        .await
        .map_err(|e| e.to_string())
}

/// Download an on-device image model, streaming `local-media://progress`.
#[tauri::command]
pub(crate) async fn pull_local_media(state: State<'_, AppState>, id: String) -> Result<(), String> {
    harness_host::local_media::pull_local_media(state.sink.clone(), &id).await
}

/// Stop an image model download; the partial file is removed.
#[tauri::command]
pub(crate) fn cancel_local_media_pull(id: String) {
    harness_host::local_media::cancel_local_media_pull(&id);
}

/// Quit an app a memory notice named, on the user's say-so; returns the
/// memory it held.
#[tauri::command]
pub(crate) async fn quit_memory_hog(name: String) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || harness_host::local_media::quit_memory_hog(&name))
        .await
        .map_err(|e| e.to_string())?
}

/// Delete an image model's weights; returns the bytes freed.
#[tauri::command]
pub(crate) async fn remove_local_media(id: String) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || harness_host::local_media::remove_local_media(&id))
        .await
        .map_err(|e| e.to_string())?
}
