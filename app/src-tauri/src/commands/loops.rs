//! Desktop bridge for the same saved, shareable verification loops as `/loop`
//! in the CLI. Management calls are deliberately small; `run_loop` delegates
//! to the shared `harness_host::SessionService`, which owns the session lock
//! and cancellation token for the whole discover/verify cycle and streams the
//! loop's agent activity as `agent://token` / `agent://tool` events.

use harness_loop::{LoopSpec, LoopStore, LoopSummary};
use harness_protocol::LoopResult;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub(crate) fn list_loops() -> Result<Vec<LoopSummary>, String> {
    Ok(LoopStore::open().map_err(|e| e.to_string())?.list())
}

#[tauri::command]
pub(crate) fn loops_path() -> Result<String, String> {
    Ok(LoopStore::open()
        .map_err(|e| e.to_string())?
        .root()
        .display()
        .to_string())
}

#[tauri::command]
pub(crate) fn get_loop(name: String) -> Result<LoopSpec, String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .resolve(&name)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn save_loop(spec: LoopSpec) -> Result<(), String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .save(&spec)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn import_loop(path: String) -> Result<LoopSpec, String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .import(path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn export_loop(name: String, path: String) -> Result<String, String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .export(&name, path)
        .map(|p| p.display().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn remove_loop(name: String) -> Result<(), String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .remove(&name)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn run_loop(
    state: State<'_, AppState>,
    session: String,
    name: Option<String>,
    goal: Option<String>,
) -> Result<LoopResult, String> {
    state.run_loop(&session, name, goal).await
}
