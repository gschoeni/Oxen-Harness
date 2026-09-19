//! Thin desktop transport for the session-scoped workbench service.

#[tauri::command]
pub(crate) async fn workbench_request(
    state: tauri::State<'_, crate::state::AppState>,
    session: String,
    action: String,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    state.workbench_request(&session, &action, payload).await
}
