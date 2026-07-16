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

/// Stop the in-flight turn for `session`, if any. A no-op when idle.
#[tauri::command]
pub(crate) async fn cancel_turn(state: State<'_, AppState>, session: String) -> Result<(), String> {
    state.cancel_turn(&session).await;
    Ok(())
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
