//! Session lifecycle and history: starting/resuming/deleting chats, reading
//! persisted transcripts (the developer inspector), the training-data review
//! flags and fine-tuning export, and the all-time token counters the hero and
//! Compression page read. Everything here goes through the shared history
//! store — the same DB the agents persist to as they run.

use std::collections::HashMap;
use std::sync::Arc;

use harness_llm::{Attachment, ChatMessage};
use harness_store::{DailyUsage, HistoryStore, ModelUsage, SessionSummary};
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

/// Estimated all-time Oxen cloud spend for the hero, using observed per-model
/// input/output tokens and current catalog rates. `None` when catalog pricing
/// is unavailable; local/custom endpoint usage is kept unpriced.
#[tauri::command]
pub(crate) async fn total_cost_usd() -> Result<Option<f64>, String> {
    let store = open_history_store()?;
    Ok(price_usage(store.model_usage_breakdown().map_err(|e| e.to_string())?)
        .await
        .total_cost_usd)
}

/// One model's accumulated usage, for the Usage settings breakdown: the model
/// id, its prompt/completion token totals, and the dollars spent on it.
#[derive(Serialize)]
pub(crate) struct ModelUsageRow {
    pub(crate) model: String,
    pub(crate) source: String,
    pub(crate) prompt_tokens: i64,
    pub(crate) completion_tokens: i64,
    pub(crate) cost_usd: Option<f64>,
}

/// The per-model usage breakdown (most-spent first) plus the grand total, for
/// the Usage settings page. Empty rows and a `0.0` total when nothing's been
/// recorded yet.
#[derive(Serialize)]
pub(crate) struct UsageBreakdown {
    pub(crate) rows: Vec<ModelUsageRow>,
    pub(crate) total_cost_usd: Option<f64>,
    pub(crate) prompt_tokens: i64,
    pub(crate) completion_tokens: i64,
    pub(crate) has_unpriced_usage: bool,
}

/// The per-model usage breakdown behind the Usage settings page: every model
/// with recorded usage, its tokens and dollars, and the grand total.
#[tauri::command]
pub(crate) async fn model_usage_breakdown(
    date: Option<String>,
) -> Result<UsageBreakdown, String> {
    let store = open_history_store()?;
    let usage = match date {
        Some(date) => store.model_usage_for_day(&date),
        None => store.model_usage_breakdown(),
    }
    .map_err(|e| e.to_string())?;
    Ok(price_usage(usage).await)
}

#[tauri::command]
pub(crate) async fn session_cost(
    model: String,
    prompt_tokens: usize,
    completion_tokens: usize,
) -> Result<Option<f64>, String> {
    let connection = harness_runtime::connection::load();
    let base_url = harness_runtime::connection::effective_base_url(&connection);
    let token = harness_runtime::connection::effective_api_key(&base_url);
    let pricing = harness_local::source::oxen_model_pricing_catalog_at(
        &base_url,
        (!token.trim().is_empty()).then_some(token.as_str()),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(pricing
        .get(&model)
        .map(|p| p.cost_of(prompt_tokens, completion_tokens)))
}

/// One day in the yearly token-activity grid.
#[derive(Serialize)]
pub(crate) struct DailyUsageRow {
    pub(crate) date: String,
    pub(crate) prompt_tokens: i64,
    pub(crate) completion_tokens: i64,
}

/// Daily token totals for a local-calendar year. Zero-usage days are omitted;
/// the frontend fills the empty grid cells.
#[tauri::command]
pub(crate) async fn daily_usage(year: i32) -> Result<Vec<DailyUsageRow>, String> {
    open_history_store()?
        .daily_usage(year)
        .map_err(|e| e.to_string())
        .map(|days| {
            days.into_iter()
                .map(|d: DailyUsage| DailyUsageRow {
                    date: d.date,
                    prompt_tokens: d.prompt_tokens,
                    completion_tokens: d.completion_tokens,
                })
                .collect()
        })
}

async fn price_usage(usage: Vec<ModelUsage>) -> UsageBreakdown {
    let connection = harness_runtime::connection::load();
    let base_url = harness_runtime::connection::effective_base_url(&connection);
    let token = harness_runtime::connection::effective_api_key(&base_url);
    let pricing = harness_local::source::oxen_model_pricing_catalog_at(
        &base_url,
        (!token.trim().is_empty()).then_some(token.as_str()),
    )
        .await
        .ok();
    price_usage_with_catalog(usage, pricing.as_ref())
}

fn price_usage_with_catalog(
    usage: Vec<ModelUsage>,
    pricing: Option<&HashMap<String, harness_local::source::ModelPricing>>,
) -> UsageBreakdown {
    let mut rows = Vec::with_capacity(usage.len());
    let mut total_cost = 0.0;
    let mut priced_any = usage.is_empty();
    let mut prompt_tokens = 0;
    let mut completion_tokens = 0;
    let mut has_unpriced_usage = false;
    for u in usage {
        prompt_tokens += u.prompt_tokens;
        completion_tokens += u.completion_tokens;
        let cost_usd = pricing
            .and_then(|catalog| catalog.get(&u.model))
            .map(|pricing| {
                priced_any = true;
                pricing.cost_of(
                    u.prompt_tokens.max(0) as usize,
                    u.completion_tokens.max(0) as usize,
                )
            });
        if cost_usd.is_none() {
            has_unpriced_usage = true;
        }
        if let Some(cost) = cost_usd {
            total_cost += cost;
        }
        rows.push(ModelUsageRow {
            model: u.model,
            source: u.source,
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            cost_usd,
        });
    }
    rows.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.model.cmp(&b.model))
    });
    UsageBreakdown {
        rows,
        total_cost_usd: priced_any.then_some(total_cost),
        prompt_tokens,
        completion_tokens,
        has_unpriced_usage,
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn catalog_prices_a_model_even_when_its_endpoint_is_custom() {
        let mut catalog = HashMap::new();
        catalog.insert(
            "priced".to_string(),
            harness_local::source::ModelPricing {
                input_cost_per_token: 0.01,
                output_cost_per_token: 0.02,
            },
        );
        let report = price_usage_with_catalog(
            vec![
                ModelUsage {
                    model: "priced".into(),
                    source: "unpriced".into(),
                    prompt_tokens: 10,
                    completion_tokens: 5,
                },
                ModelUsage {
                    model: "missing".into(),
                    source: "oxen_cloud".into(),
                    prompt_tokens: 20,
                    completion_tokens: 5,
                },
            ],
            Some(&catalog),
        );

        assert_eq!(report.total_cost_usd, Some(0.2));
        assert!(report.has_unpriced_usage);
        assert!(report
            .rows
            .iter()
            .any(|row| row.model == "missing" && row.cost_usd.is_none()));
    }
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
