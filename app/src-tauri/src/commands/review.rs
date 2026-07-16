//! The code-review commands: run the configurable review pipeline against a
//! chat's workspace, and read/save/reset the pipeline configuration for the
//! Settings page. The pipeline itself lives in `harness_review`, the event
//! streaming in `harness_host::SessionService` — progress arrives in the
//! webview as `review://*` and `fleet://*` events through the shared sink.

use harness_protocol::ReviewResult;
use tauri::State;

use crate::state::AppState;

/// Run the configurable code-review pipeline for a chat's workspace:
/// uncommitted changes by default, or PR-style against `base_branch`. Streams
/// progress via `review://progress` / `review://token` / `review://tool`,
/// then injects the findings into the session (as a settled user/assistant
/// exchange) so follow-up turns can act on them ("fix 1 and 3"). Holds the
/// session's agent lock for the duration, so it can't interleave with a
/// running turn; `cancel_turn` stops it.
#[tauri::command]
pub(crate) async fn run_code_review(
    state: State<'_, AppState>,
    session: String,
    base_branch: Option<String>,
) -> Result<ReviewResult, String> {
    state.run_code_review(&session, base_branch).await
}

/// The saved code-review pipeline (steps + findings cap) for the Settings page.
#[tauri::command]
pub(crate) fn get_code_review_config() -> harness_review::ReviewConfig {
    harness_review::ReviewConfig::load()
}

/// Persist the code-review pipeline and snapshot the versioned config repo.
/// Applies to the next review — a running one keeps the steps it started with.
#[tauri::command]
pub(crate) fn save_code_review_config(config: harness_review::ReviewConfig) -> Result<(), String> {
    config.save().map_err(|e| e.to_string())?;
    harness_runtime::config_repo::snapshot("Update code review settings");
    Ok(())
}

/// The built-in default pipeline, for the Settings page's "reset to defaults".
#[tauri::command]
pub(crate) fn default_code_review_config() -> harness_review::ReviewConfig {
    harness_review::ReviewConfig::default()
}
