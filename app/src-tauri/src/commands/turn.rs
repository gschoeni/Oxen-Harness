//! Driving a chat turn: run, retry, and cancel — plus delivering the user's
//! `ask_user_question` answers and permission-approval decisions back to a
//! turn parked on them. All of it delegates to the shared
//! `harness_host::SessionService`; the streaming happens there, arriving in
//! the webview through `crate::state::TauriSink`.

use harness_protocol::{ApprovalAnswer, QuestionAnswer};
use tauri::State;

use crate::state::AppState;

/// Run one user turn for a specific chat, streaming session-tagged events to
/// the UI; returns the final text. Holds only that session's lock, so turns
/// in other chats keep running concurrently. `attachments` are dropped/pasted
/// file paths; unreadable ones are skipped so a bad path never blocks the turn.
#[tauri::command]
pub(crate) async fn run_turn(
    state: State<'_, AppState>,
    session: String,
    prompt: String,
    attachments: Option<Vec<String>>,
) -> Result<String, String> {
    state
        .run_turn(&session, prompt, attachments.unwrap_or_default())
        .await
}

/// Retry the chat's failed turn after its API key was set, continuing the same
/// conversation without re-appending the user message.
#[tauri::command]
pub(crate) async fn retry_turn(
    state: State<'_, AppState>,
    session: String,
) -> Result<String, String> {
    state.retry_turn(&session).await
}

/// Deliver background results (a finished generation, fleet, or shell task)
/// that landed while the chat was idle, as a turn of their own; `None` when
/// there was nothing to deliver. The UI calls it on `turn://delivery-ready`.
#[tauri::command]
pub(crate) async fn deliver_pending(
    state: State<'_, AppState>,
    session: String,
) -> Result<Option<String>, String> {
    state.deliver_pending(&session).await
}

/// Stop the in-flight turn for `session`, if any. A no-op when idle.
#[tauri::command]
pub(crate) async fn cancel_turn(state: State<'_, AppState>, session: String) -> Result<(), String> {
    state.cancel_turn(&session).await;
    Ok(())
}

/// Stop one `spawn_agents` fleet (named on its `fleet://started`) without
/// ending the turn; `false` once it has already ended.
#[tauri::command]
pub(crate) fn cancel_fleet(
    state: State<'_, AppState>,
    session: String,
    fleet: String,
) -> Result<bool, String> {
    Ok(state.cancel_fleet(&session, &fleet))
}

/// Stop one lane of a fleet (its id rides on `fleet://agent`); the rest of
/// the fleet carries on. `false` once the lane has already ended.
#[tauri::command]
pub(crate) fn cancel_agent(
    state: State<'_, AppState>,
    session: String,
    lane: String,
) -> Result<bool, String> {
    Ok(state.cancel_lane(&session, &lane))
}

/// Hand a running lane a message for its next round; `false` once it has
/// ended.
#[tauri::command]
pub(crate) fn interject_agent(
    state: State<'_, AppState>,
    session: String,
    lane: String,
    text: String,
) -> Result<bool, String> {
    Ok(state.interject_lane(&session, &lane, text))
}

/// A chat's background shell tasks, running and ended.
#[tauri::command]
pub(crate) async fn list_tasks(
    state: State<'_, AppState>,
    session: String,
) -> Result<Vec<harness_protocol::TaskSummary>, String> {
    Ok(state.list_tasks(&session).await)
}

/// Kill one background task (its whole process group).
#[tauri::command]
pub(crate) async fn kill_background_task(
    state: State<'_, AppState>,
    session: String,
    id: u64,
) -> Result<String, String> {
    state.kill_task(&session, id).await
}

/// Deliver the user's answer to a pending `ask_user_question`, unblocking the
/// agent. Unknown ids are ignored (the question may have been cancelled).
#[tauri::command]
pub(crate) async fn answer_question(
    state: State<'_, AppState>,
    id: String,
    answers: Vec<QuestionAnswer>,
) -> Result<(), String> {
    state.answer_question(&id, answers);
    Ok(())
}

/// Deliver the user's decision on a pending permission approval, unblocking
/// the gated tool call. Unknown ids are ignored.
#[tauri::command]
pub(crate) async fn answer_approval(
    state: State<'_, AppState>,
    id: String,
    answer: ApprovalAnswer,
) -> Result<(), String> {
    state.answer_approval(&id, answer);
    Ok(())
}
