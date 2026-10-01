//! The codebase study game ("Trail of Understanding"): the profile, question
//! batches, and answers for the project a chat is rooted in, plus the model
//! roles (`study`, `smol`, `summary`) the Settings page assigns. The quiz
//! logic lives in `harness_study`; the host surface in
//! `harness_host::SessionService` — none of it waits on the chat's turn, so
//! the game plays while the agent works.

use harness_protocol::{
    StudyAnswerRequest, StudyAnswerResult, StudyBatch, StudyBatchRequest, StudyProfile,
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

/// How well the user understands the project `session` is rooted in. Reading
/// it walks the workspace tree, so it runs off the main thread.
#[tauri::command]
pub(crate) async fn study_profile(
    state: State<'_, AppState>,
    session: String,
) -> Result<StudyProfile, String> {
    let service = state.service.clone();
    tauri::async_runtime::spawn_blocking(move || service.study_profile(&session))
        .await
        .map_err(|e| format!("reading the study profile: {e}"))?
}

/// Questions for one stretch of a run. Rejects with the reason when the mode
/// has nothing to ask about (a clean tree, an agent that touched no files).
#[tauri::command]
pub(crate) async fn study_batch(
    state: State<'_, AppState>,
    session: String,
    request: StudyBatchRequest,
) -> Result<StudyBatch, String> {
    state.study_batch(&session, request).await
}

/// Grade an answer and record it against the project's progress.
#[tauri::command]
pub(crate) async fn study_answer(
    state: State<'_, AppState>,
    session: String,
    request: StudyAnswerRequest,
) -> Result<StudyAnswerResult, String> {
    state.study_answer(&session, request).await
}

/// The per-role model overrides from `limits.json`. `None` means the role
/// falls back (study → smol → the session model; the others → the session
/// model).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ModelRolesView {
    pub study: Option<String>,
    pub smol: Option<String>,
    pub summary: Option<String>,
}

#[tauri::command]
pub(crate) fn get_model_roles() -> ModelRolesView {
    let limits = harness_runtime::limits::load();
    ModelRolesView {
        study: limits.study_model,
        smol: limits.smol_model,
        summary: limits.summary_model,
    }
}

/// Assign (or, with `model: null`, clear) one role. Applies to new and
/// resumed chats; the study game reads it on its next question batch.
#[tauri::command]
pub(crate) fn set_model_role(
    role: String,
    model: Option<String>,
) -> Result<ModelRolesView, String> {
    let model = model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let mut limits = harness_runtime::limits::load();
    match role.as_str() {
        "study" => limits.study_model = model,
        "smol" => limits.smol_model = model,
        "summary" => limits.summary_model = model,
        other => return Err(format!("unknown model role `{other}`")),
    }
    harness_runtime::limits::save(&limits)
        .map_err(|e| format!("saving the {role} model role: {e}"))?;
    Ok(get_model_roles())
}
