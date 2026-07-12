//! Driving a chat turn: run, retry, and cancel — plus delivering the user's
//! `ask_user_question` answer back to a turn parked on one.
//!
//! [`execute_turn`] is the single streaming/accounting scaffold both entry
//! points share: rehydrate the agent, register the turn's cancel token,
//! forward every [`AgentEvent`] to the webview as session-tagged events, then
//! persist usage in the shared agent ledger.

use harness_agent::AgentEvent;
use harness_llm::Attachment;
use harness_tools::QuestionAnswer;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::commands::session::bump_total_tokens_saved;
use crate::events::{
    CompactedPayload, CompressionPayload, RetryPayload, SessionPayload, TokenPayload,
    ToolDeltaPayload, ToolEventPayload, UsagePayload,
};
use crate::state::{agent_or_build, evict_idle, AppState};

/// Whether a turn starts fresh (pushing a new user message) or retries the
/// existing transcript's trailing user turn (e.g. after authenticating past a
/// 401). Both drive the identical streaming/accounting scaffold in [`execute_turn`].
enum TurnKind {
    Fresh {
        prompt: String,
        attachments: Vec<Attachment>,
    },
    Retry,
}

const TOKEN_BATCH_BYTES: usize = 512;

#[derive(Clone)]
struct TokenBatch {
    app: AppHandle,
    session: String,
    buffer: Arc<Mutex<String>>,
}

impl TokenBatch {
    fn new(app: AppHandle, session: String) -> Self {
        Self {
            app,
            session,
            buffer: Arc::new(Mutex::new(String::with_capacity(TOKEN_BATCH_BYTES))),
        }
    }

    fn push(&self, token: &str) {
        let ready = {
            let mut buffer = self.buffer.lock().expect("token batch poisoned");
            buffer.push_str(token);
            (buffer.len() >= TOKEN_BATCH_BYTES).then(|| std::mem::take(&mut *buffer))
        };
        if let Some(text) = ready {
            self.emit(text);
        }
    }

    fn flush(&self) {
        let text = std::mem::take(&mut *self.buffer.lock().expect("token batch poisoned"));
        if !text.is_empty() {
            self.emit(text);
        }
    }

    fn emit(&self, token: String) {
        let _ = self.app.emit(
            "agent://token",
            TokenPayload {
                session: self.session.clone(),
                token,
            },
        );
    }
}

/// Run one user turn for a specific chat, streaming session-tagged events to the
/// UI; returns the final text. Holds only that session's lock, so turns in other
/// chats keep running concurrently.
#[tauri::command]
pub(crate) async fn run_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
    prompt: String,
    attachments: Option<Vec<String>>,
) -> Result<String, String> {
    // Read any dropped file paths into attachments, skipping unreadable ones so
    // a bad path never blocks the turn (the agent just sends what loaded).
    let attachments: Vec<Attachment> = attachments
        .unwrap_or_default()
        .iter()
        .filter_map(|p| Attachment::from_path(p).ok())
        .collect();
    execute_turn(
        app,
        &state,
        session,
        TurnKind::Fresh {
            prompt,
            attachments,
        },
    )
    .await
}

/// Retry the current chat's failed turn after its API key was set, continuing the
/// same conversation. The user message from the failed attempt is already in the
/// transcript, so this drives it again without re-appending it (avoiding a
/// duplicate user turn in the history / fine-tuning export).
#[tauri::command]
pub(crate) async fn retry_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
) -> Result<String, String> {
    execute_turn(app, &state, session, TurnKind::Retry).await
}

/// The shared body of a turn: rehydrate the agent, register a cancel token, run
/// the turn (fresh or retried) while forwarding streamed events to the UI, then
/// account for tokens and release idle background agents.
async fn execute_turn(
    app: AppHandle,
    state: &State<'_, AppState>,
    session: String,
    kind: TurnKind,
) -> Result<String, String> {
    // Get the live agent or rehydrate it from the database. The agents map is a
    // cache, not the source of truth, so an evicted chat simply rebuilds here.
    let arc = agent_or_build(&app, state, &session).await?;

    let sid = session.clone();
    // The context window is fixed for the turn; capture it once so the live usage
    // events emitted from inside the turn can report "% of context".
    let context_window = arc.lock().await.context_window();
    // A fresh stop signal for this turn, registered so `cancel_turn` can fire it
    // (a clone) without waiting on the agent lock the turn holds.
    let cancel = CancellationToken::new();
    state
        .cancels
        .lock()
        .await
        .insert(session.clone(), cancel.clone());
    // Hand the turn's stop signal to the session's fleet spawner too, so
    // cancelling the turn also stops any fleet the model launched inside it.
    if let Some(spawner) = state
        .fleet_spawners
        .lock()
        .expect("fleet spawners poisoned")
        .get(&session)
        .cloned()
    {
        spawner.set_cancel(cancel.clone());
    }
    // Track compression savings around the turn. Model usage itself is recorded
    // per call inside `Agent`, where provider-reported input/output counts and
    // the exact model are available (including review/fleet side agents).
    let saved_delta;
    let token_batch = TokenBatch::new(app.clone(), sid.clone());
    let result = {
        let mut agent = arc.lock().await;
        agent.set_cancel_token(cancel.clone());
        let saved_before = agent.tokens_saved();
        let event_tokens = token_batch.clone();
        let on_event = move |event: &AgentEvent| {
            if !matches!(event, AgentEvent::Token(_)) {
                event_tokens.flush();
            }
            match event {
            AgentEvent::Token(t) => event_tokens.push(t),
            // The model started writing a canvas; open the panel in a
            // "writing" state while its content streams in as tool args.
            AgentEvent::ToolPending { name } if name == harness_tools::CANVAS_TOOL => {
                let _ = app.emit(
                    "agent://canvas-writing",
                    SessionPayload {
                        session: sid.clone(),
                    },
                );
            }
            AgentEvent::ToolPending { .. } => {}
            // Stream the tool call's arguments as they arrive so the UI can
            // show the file/canvas content being written in real time.
            AgentEvent::ToolDelta { name, delta } => {
                let _ = app.emit(
                    "agent://tool-delta",
                    ToolDeltaPayload {
                        session: sid.clone(),
                        name: name.clone(),
                        delta: delta.clone(),
                    },
                );
            }
            AgentEvent::ToolStart { name, arguments } => {
                let _ = app.emit(
                    "agent://tool",
                    ToolEventPayload {
                        session: sid.clone(),
                        phase: "start",
                        name: name.clone(),
                        detail: arguments.clone(),
                    },
                );
            }
            AgentEvent::ToolEnd { name, result } => {
                let _ = app.emit(
                    "agent://tool",
                    ToolEventPayload {
                        session: sid.clone(),
                        phase: "end",
                        name: name.clone(),
                        detail: result.clone(),
                    },
                );
            }
            // Live token usage, surfaced around each model call within the
            // turn so the meter tracks real consumption as it accrues rather
            // than jumping only at the end.
            AgentEvent::Usage {
                tokens_used,
                context_tokens,
                prompt_tokens_used,
                completion_tokens_used,
            } => {
                let _ = app.emit(
                    "agent://usage",
                    UsagePayload {
                        session: sid.clone(),
                        tokens_used: *tokens_used,
                        context_tokens: *context_tokens,
                        context_window,
                        prompt_tokens_used: *prompt_tokens_used,
                        completion_tokens_used: *completion_tokens_used,
                    },
                );
            }
            // The context filled and was compacted; surface it as a visible
            // notice in the thread so the trimming isn't silent.
            AgentEvent::Compacted { detail } => {
                let _ = app.emit(
                    "agent://compacted",
                    CompactedPayload {
                        session: sid.clone(),
                        detail: detail.clone(),
                    },
                );
            }
            // A transient provider/network failure being retried with backoff;
            // surface it so the turn reads as alive (and debuggable), not hung.
            AgentEvent::Retrying {
                attempt,
                max_attempts,
                delay_ms,
                error,
            } => {
                let _ = app.emit(
                    "agent://retry",
                    RetryPayload {
                        session: sid.clone(),
                        attempt: *attempt,
                        max_attempts: *max_attempts,
                        delay_ms: *delay_ms,
                        error: error.clone(),
                    },
                );
            }
            // Compression shrank (or, in audit mode, measured) this model
            // call's request; surface the savings so the UI can track them.
            AgentEvent::Compression {
                mode,
                saved_tokens,
                total_saved_tokens,
                results_compressed,
            } => {
                let _ = app.emit(
                    "agent://compression",
                    CompressionPayload {
                        session: sid.clone(),
                        mode: mode.clone(),
                        saved_tokens: *saved_tokens,
                        total_saved_tokens: *total_saved_tokens,
                        results_compressed: *results_compressed,
                    },
                );
            }
            }
        };
        let r = match kind {
            TurnKind::Fresh {
                prompt,
                attachments,
            } => {
                agent
                    .run_turn_with_attachments(prompt, attachments, on_event)
                    .await
            }
            TurnKind::Retry => agent.continue_turn(on_event).await,
        };
        saved_delta = agent.tokens_saved().saturating_sub(saved_before);
        r
    };
    token_batch.flush();
    // The turn is over (finished, stopped, or errored): drop its stop signal so a
    // later `cancel_turn` can't fire against a stale token.
    state.cancels.lock().await.remove(&session);
    // Same for what compression saved (or would have saved, in audit mode).
    let _ = bump_total_tokens_saved(saved_delta);
    // The turn is persisted message-by-message, so once it's done the agent is
    // just a cache. Release idle background agents (keeping the current chat and
    // any still-running turns) so memory tracks concurrency, not chat count.
    evict_idle(state).await;
    result.map_err(|e| e.to_string())
}

/// Stop the in-flight turn for `session`, if any. Fires that turn's cancellation
/// token, which breaks the streaming read and drops the HTTP connection — so a
/// local `llama-server` stuck chewing through a long prompt is released too. The
/// turn returns its partial reply (often empty) and settles normally; a no-op if
/// the session isn't currently running.
#[tauri::command]
pub(crate) async fn cancel_turn(state: State<'_, AppState>, session: String) -> Result<(), String> {
    if let Some(token) = state.cancels.lock().await.get(&session) {
        token.cancel();
    }
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
    let sender = state
        .pending
        .lock()
        .expect("pending mutex poisoned")
        .remove(&id);
    if let Some(tx) = sender {
        let _ = tx.send(answers);
    }
    Ok(())
}
