//! Session lifecycle and history: starting/resuming/deleting chats, reading
//! persisted transcripts (the developer inspector), the training-data review
//! flags and fine-tuning export, and the all-time token counters the hero and
//! Compression page read. Everything here goes through the shared history
//! store — the same DB the agents persist to as they run.

use std::sync::Arc;

use harness_llm::{Attachment, ChatMessage};
use harness_store::{HistoryStore, SessionSummary};
use serde::Serialize;
use tauri::{AppHandle, State};
use tokio::sync::Mutex;

use crate::commands::project::remember_project;
use crate::state::{
    active_root, agent_for, build_fresh_agent, build_resumed_agent, current_agent, evict_idle,
    info_for, install_agent, open_history_store, session_workspace, AppState,
};

#[derive(Clone, Serialize)]
pub(crate) struct SessionInfo {
    pub(crate) model: String,
    pub(crate) workspace: String,
    pub(crate) session_id: String,
    /// Cumulative tokens used in this session, so the UI dashboard reflects real
    /// consumption rather than static flavor text.
    pub(crate) tokens_used: usize,
    /// Tokens the current transcript occupies (how full the context window is).
    pub(crate) context_tokens: usize,
    /// The model's effective context window, for a "% of context" readout.
    pub(crate) context_window: usize,
    /// The context-compression mode this session's agent was built with
    /// ("off"/"audit"/"on") — drives the TokenMeter's armed indicator.
    pub(crate) compression_mode: String,
}

/// A resumed session: its info plus the verbatim transcript for the UI to
/// re-render (user/assistant bubbles and tool activity). When `running` is true
/// the chat is mid-turn and couldn't be read; `messages` is empty and the UI
/// keeps the live thread it already streamed.
#[derive(Serialize)]
pub(crate) struct SessionView {
    info: SessionInfo,
    messages: Vec<serde_json::Value>,
    running: bool,
}

/// Report the current session info, initializing the agent if needed.
#[tauri::command]
pub(crate) async fn session_info(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SessionInfo, String> {
    let arc = current_agent(&app, &state).await?;
    let agent = arc.lock().await;
    Ok(info_for(&agent))
}

/// List past chat sessions (those with at least one user message), newest first.
#[tauri::command]
pub(crate) async fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    open_history_store()?
        .list_sessions()
        .map_err(|e| e.to_string())
}

/// Read a session's raw, persisted transcript (every message, verbatim — system
/// prompt, tool calls, and tool results included) straight from the history
/// store, for the developer inspector. Read-only and never touches the live
/// agent, so it works even while a turn is mid-flight.
#[tauri::command]
pub(crate) async fn session_messages(id: String) -> Result<Vec<serde_json::Value>, String> {
    open_history_store()?
        .messages(&id)
        .map_err(|e| e.to_string())
}

/// Set a chat's training-data review status: `""` (unreviewed), `"kept"`, or
/// `"rejected"`. Persisted so the dataset builder's decisions survive restarts.
#[tauri::command]
pub(crate) async fn set_review_status(id: String, status: String) -> Result<(), String> {
    open_history_store()?
        .set_review_status(&id, &status)
        .map_err(|e| e.to_string())
}

/// Bulk-set the review status for many chats at once (bulk keep/reject/clear
/// from the dataset builder). Returns how many rows changed.
#[tauri::command]
pub(crate) async fn set_review_status_many(
    ids: Vec<String>,
    status: String,
) -> Result<usize, String> {
    open_history_store()?
        .set_review_status_many(&ids, &status)
        .map_err(|e| e.to_string())
}

/// Permanently delete a chat session: remove it (and its messages) from history,
/// drop any cached live agent, and clear it as the current chat if it was active.
#[tauri::command]
pub(crate) async fn delete_session(state: State<'_, AppState>, id: String) -> Result<(), String> {
    open_history_store()?
        .delete_session(&id)
        .map_err(|e| e.to_string())?;
    state.agents.lock().await.remove(&id);
    // Drop the session's fleet spawner in lockstep with its agent, so a
    // deleted chat doesn't leave its spawner (a client + tool-registry +
    // config clone) stranded in the map until the next eviction sweep.
    state
        .fleet_spawners
        .lock()
        .expect("fleet spawners poisoned")
        .remove(&id);
    let mut current = state.current.lock().await;
    if current.as_deref() == Some(id.as_str()) {
        *current = None;
    }
    Ok(())
}

/// Load an attachment as a `data:` URI for display in the UI (composer preview
/// and chat history). `path` is either an absolute path (a freshly picked file)
/// or a path relative to a session's workspace (how persisted image attachments
/// are stored, under `.oxen-harness/attachments/`). Returning a data URI keeps
/// rendering CSP-safe — no asset-protocol or file:// access needed.
#[tauri::command]
pub(crate) async fn attachment_data_uri(
    path: String,
    session: Option<String>,
) -> Result<String, String> {
    let p = std::path::Path::new(&path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else if let Some(s) = session {
        session_workspace(&s).join(p)
    } else {
        p.to_path_buf()
    };
    let attachment = Attachment::from_path(&abs).map_err(|e| e.to_string())?;
    Ok(attachment.data_uri())
}

/// Export the given sessions as chat-completions fine-tuning JSONL (Oxen.ai
/// format: one `{"messages":[…]}` conversation per line) to `path`. Returns the
/// number of conversations written. `include_tools` keeps tool calls + results.
#[tauri::command]
pub(crate) async fn export_finetuning(
    path: String,
    session_ids: Vec<String>,
    include_tools: bool,
) -> Result<usize, String> {
    let jsonl = open_history_store()?
        .export_chat_completions(&session_ids, include_tools)
        .map_err(|e| e.to_string())?;
    let count = jsonl.lines().filter(|l| !l.is_empty()).count();
    std::fs::write(&path, jsonl).map_err(|e| format!("could not write {path}: {e}"))?;
    Ok(count)
}

/// The `app_meta` key holding the all-time running total of tokens used.
const TOTAL_TOKENS_KEY: &str = "total_tokens_used";

/// The all-time total tokens used across every session — a running grand total
/// for the hero's "Total tokens used" stat. Read from a cheap persisted counter
/// (backfilled once from history), not by rescanning transcripts each call.
#[tauri::command]
pub(crate) async fn total_tokens_used() -> Result<usize, String> {
    let store = open_history_store()?;
    Ok(ensure_total_tokens(&store)?.max(0) as usize)
}

/// The estimated all-time dollars spent, priced at the currently-selected cloud
/// model's per-token rates from the Oxen models API. `None` when no cost can be
/// computed — a local model is active, the model isn't listed with token
/// pricing, or the catalog can't be reached. Best-effort: cost is informational,
/// so we never surface a hard error to the UI (we return `Ok(None)`).
#[tauri::command]
pub(crate) async fn total_cost_usd() -> Result<Option<f64>, String> {
    let model = harness_runtime::models::selected();
    // A local model has no cloud price; report "unavailable" rather than $0.
    if model.trim().is_empty() {
        return Ok(None);
    }
    let token = harness_config::secrets::get("OXEN_API_KEY").filter(|t| !t.trim().is_empty());
    let pricing = match harness_local::source::oxen_model_pricing(&model, token.as_deref()).await {
        Ok(Some(p)) => p,
        // No pricing listed, or the catalog was unreachable: treat as unavailable.
        Ok(None) | Err(_) => return Ok(None),
    };
    let (input_tokens, output_tokens) = total_io_tokens();
    Ok(Some(pricing.cost_of(input_tokens, output_tokens)))
}

/// Ensure the running token counter exists, seeding it once from existing
/// history if it was never set, and return the current total. The expensive
/// transcript scan runs at most once (the first time); afterwards each turn just
/// increments the counter, so reads and updates stay O(1).
fn ensure_total_tokens(store: &HistoryStore) -> Result<i64, String> {
    if let Some(v) = store
        .meta_get_i64(TOTAL_TOKENS_KEY)
        .map_err(|e| e.to_string())?
    {
        return Ok(v);
    }
    let seeded = estimate_all_tokens(store) as i64;
    store
        .meta_set_i64(TOTAL_TOKENS_KEY, seeded)
        .map_err(|e| e.to_string())?;
    Ok(seeded)
}

/// One-time backfill: estimate tokens across every stored transcript. We don't
/// keep exact historical per-turn counts, so this is a best-effort seed for the
/// running counter; new turns add their real throughput on top.
fn estimate_all_tokens(store: &HistoryStore) -> usize {
    let Ok(sessions) = store.list_sessions() else {
        return 0;
    };
    let mut total = 0usize;
    for s in sessions {
        let Ok(raw) = store.messages(&s.id) else {
            continue;
        };
        let messages: Vec<ChatMessage> = raw
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        total += harness_agent::budget::estimate_prompt_tokens(&messages, &[]);
    }
    total
}

/// Add a turn's token throughput to the all-time counter (backfilling once if
/// needed) and return the new grand total. Best-effort: never fails a turn.
pub(crate) fn bump_total_tokens(delta: usize) -> usize {
    let Ok(store) = open_history_store() else {
        return 0;
    };
    let _ = ensure_total_tokens(&store);
    store
        .meta_add_i64(TOTAL_TOKENS_KEY, delta as i64)
        .map(|v| v.max(0) as usize)
        .unwrap_or(0)
}

/// `app_meta` keys holding the all-time input (prompt) and output (completion)
/// token totals, tracked separately so cost can be priced at a model's distinct
/// input/output rates. Unlike [`TOTAL_TOKENS_KEY`], these aren't backfilled from
/// history (we can't split an old transcript into prompt vs completion), so they
/// count only tokens observed since this feature shipped.
const TOTAL_INPUT_TOKENS_KEY: &str = "total_input_tokens";
const TOTAL_OUTPUT_TOKENS_KEY: &str = "total_output_tokens";

/// The all-time input/output token totals (prompt, completion). Best-effort:
/// missing counters read as 0.
pub(crate) fn total_io_tokens() -> (usize, usize) {
    let Ok(store) = open_history_store() else {
        return (0, 0);
    };
    let read = |key: &str| {
        store
            .meta_get_i64(key)
            .ok()
            .flatten()
            .unwrap_or(0)
            .max(0) as usize
    };
    (read(TOTAL_INPUT_TOKENS_KEY), read(TOTAL_OUTPUT_TOKENS_KEY))
}

/// Add a turn's input/output token throughput to the all-time split counters.
/// Best-effort: never fails a turn.
pub(crate) fn bump_io_tokens(input_delta: usize, output_delta: usize) {
    let Ok(store) = open_history_store() else {
        return;
    };
    if input_delta > 0 {
        let _ = store.meta_add_i64(TOTAL_INPUT_TOKENS_KEY, input_delta as i64);
    }
    if output_delta > 0 {
        let _ = store.meta_add_i64(TOTAL_OUTPUT_TOKENS_KEY, output_delta as i64);
    }
}

/// The `app_meta` key holding the all-time tokens saved by context compression.
const TOTAL_TOKENS_SAVED_KEY: &str = "total_tokens_saved";

/// The all-time tokens compression saved (mode `on`) or would have saved
/// (mode `audit`) across every session — the Compression settings page's stat.
/// No backfill: savings only exist from the moment the feature ships, so the
/// counter simply starts at 0.
#[tauri::command]
pub(crate) async fn total_tokens_saved() -> Result<usize, String> {
    let store = open_history_store()?;
    Ok(store
        .meta_get_i64(TOTAL_TOKENS_SAVED_KEY)
        .map_err(|e| e.to_string())?
        .unwrap_or(0)
        .max(0) as usize)
}

/// Add a turn's compression savings to the all-time counter and return the new
/// grand total. Best-effort: never fails a turn.
pub(crate) fn bump_total_tokens_saved(delta: usize) -> usize {
    if delta == 0 {
        return 0; // the common case (compression off) — skip the DB round-trip
    }
    let Ok(store) = open_history_store() else {
        return 0;
    };
    store
        .meta_add_i64(TOTAL_TOKENS_SAVED_KEY, delta as i64)
        .map(|v| v.max(0) as usize)
        .unwrap_or(0)
}

/// Start a fresh chat session as its own agent. Any in-flight chats keep running
/// in the background — this never disturbs them. Returns the new session's info.
#[tauri::command]
pub(crate) async fn new_session(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SessionInfo, String> {
    let root = active_root(&state).await;
    let agent = build_fresh_agent(&app, &state, &root).await?;
    Ok(install_agent(&state, agent).await)
}

/// Switch to an existing session, returning its info and full transcript so the
/// UI can re-render the conversation. Reuses the session's live agent if one
/// exists (e.g. a chat that finished in the background); otherwise loads it cold
/// from history. A chat still mid-turn can't be locked, so its transcript comes
/// back empty — the UI keeps the live thread it already streamed.
#[tauri::command]
pub(crate) async fn resume_session(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionView, String> {
    // The session belongs to its own project; opening it enters that project so
    // new chats land in the same directory.
    let workspace = session_workspace(&id);
    *state.active_project.lock().await = workspace.clone();
    let _ = remember_project(&workspace.display().to_string());

    let arc = match agent_for(&state, &id).await {
        Some(a) => a,
        None => {
            // Cold resume: build an agent bound to the existing session (no
            // throwaway row), rooted at the session's own workspace, then insert
            // via the map entry so a concurrent resume can't leave two behind.
            let agent = build_resumed_agent(&app, &state, id.clone(), &workspace).await?;
            let arc = Arc::new(Mutex::new(agent));
            let winner = state
                .agents
                .lock()
                .await
                .entry(id.clone())
                .or_insert(arc)
                .clone();
            winner
        }
    };
    *state.current.lock().await = Some(id.clone());
    evict_idle(&state).await;

    // Bind to a local so the try_lock guard drops before `arc` at block end.
    let view = match arc.try_lock() {
        Ok(agent) => {
            let messages = agent
                .messages()
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            SessionView {
                info: info_for(&agent),
                messages,
                running: false,
            }
        }
        // Mid-turn: can't read it. The UI keeps its live in-memory thread; the
        // explicit `running` flag tells it not to touch the transcript.
        Err(_) => SessionView {
            info: SessionInfo {
                model: String::new(),
                workspace: workspace.display().to_string(),
                session_id: id,
                tokens_used: 0,
                context_tokens: 0,
                context_window: 0,
                // Mid-turn placeholder: the live agent is locked, so report the
                // saved preference (what any rebuilt agent would get).
                compression_mode: harness_runtime::compression::mode().as_str().to_string(),
            },
            messages: vec![],
            running: true,
        },
    };
    Ok(view)
}
