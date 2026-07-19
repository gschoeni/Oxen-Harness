//! Session lifecycle and history: starting/resuming/deleting chats, reading
//! persisted transcripts (the developer inspector), the training-data review
//! flags and fine-tuning export, and the all-time token counters the hero and
//! Compression page read. Lifecycle goes through the shared
//! `harness_host::SessionService`; the read-only stats go straight to the
//! history store — the same DB the agents persist to as they run.

use std::collections::HashMap;

use harness_llm::{Attachment, ChatMessage};
use harness_protocol::{SessionInfo, SessionView};
use harness_store::{DailyUsage, HistoryStore, ModelUsage, SessionSummary};
use serde::Serialize;
use tauri::State;

use crate::commands::project::remember_project;
use crate::state::{open_history_store, AppState};

/// Report the current session info, initializing the agent if needed.
#[tauri::command]
pub(crate) async fn session_info(state: State<'_, AppState>) -> Result<SessionInfo, String> {
    state.session_info().await
}

/// List past chat sessions (those with at least one user message), newest first.
#[tauri::command]
pub(crate) async fn list_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<SessionSummary>, String> {
    state.list_sessions()
}

/// Read a session's raw, persisted transcript (every message, verbatim — system
/// prompt, tool calls, and tool results included) straight from the history
/// store, for the developer inspector. Read-only and never touches the live
/// agent, so it works even while a turn is mid-flight.
#[tauri::command]
pub(crate) async fn session_messages(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<serde_json::Value>, String> {
    state.session_messages(&id)
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

/// Permanently delete a chat session: remove it (and its messages) from
/// history, drop any cached live agent, stop its dev server (the service),
/// and close its preview webview (the deletion hook).
#[tauri::command]
pub(crate) async fn delete_session(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.delete_session(&id).await
}

/// Load an attachment as a `data:` URI for display in the UI (composer preview
/// and chat history). `path` is either an absolute path (a freshly picked file)
/// or a path relative to a session's workspace (how persisted image attachments
/// are stored, under `.oxen-harness/attachments/`). Returning a data URI keeps
/// rendering CSP-safe — no asset-protocol or file:// access needed.
#[tauri::command]
pub(crate) async fn attachment_data_uri(
    state: State<'_, AppState>,
    path: String,
    session: Option<String>,
) -> Result<String, String> {
    let p = std::path::Path::new(&path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else if let Some(s) = session {
        state.session_workspace(&s).join(p)
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

/// One importable source of external conversations, with how many the tool's
/// local logs hold and how many were already imported — the Training Data
/// page's import panel.
#[derive(Serialize)]
pub(crate) struct ImportSourceStatus {
    /// Source id as stored on sessions: `"claude-code"` or `"cursor"`.
    pub(crate) source: String,
    /// Conversations found in the tool's local logs (drafts included — the
    /// import itself drops conversations without a real exchange).
    pub(crate) available: usize,
    /// Sessions already imported from this source.
    pub(crate) imported: i64,
}

/// Where Claude Code keeps its per-project session transcripts.
fn claude_code_root() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Cursor's user-data directory (`.../Cursor/User`); `config_dir` resolves the
/// platform's app-support location (e.g. `~/Library/Application Support`).
fn cursor_user_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|c| c.join("Cursor").join("User"))
}

/// What the local machine has to import: per source, how many conversations
/// its logs hold and how many are already in the store.
#[tauri::command]
pub(crate) async fn import_sources_scan() -> Result<Vec<ImportSourceStatus>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        use harness_store::import::{claude_code, cursor, SOURCE_CLAUDE_CODE, SOURCE_CURSOR};
        let store = open_history_store()?;
        let mut out = Vec::new();
        for (source, available) in [
            (
                SOURCE_CLAUDE_CODE,
                claude_code_root()
                    .map(|r| claude_code::scan(&r))
                    .unwrap_or(0),
            ),
            (
                SOURCE_CURSOR,
                cursor_user_dir().map(|d| cursor::scan(&d)).unwrap_or(0),
            ),
        ] {
            out.push(ImportSourceStatus {
                source: source.to_string(),
                available,
                imported: store.imported_count(source).map_err(|e| e.to_string())?,
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Import every conversation from one source's local logs. Rescans dedup by
/// the source's own conversation id: new conversations are added, ones that
/// grew since the last import are refreshed (keeping their review status), and
/// unchanged ones are skipped. Runs on a blocking thread — parsing a large
/// history can take a while.
#[tauri::command]
pub(crate) async fn import_external(source: String) -> Result<harness_store::ImportReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use harness_store::import::{claude_code, cursor, SOURCE_CLAUDE_CODE, SOURCE_CURSOR};
        let conversations = match source.as_str() {
            SOURCE_CLAUDE_CODE => claude_code_root()
                .map(|r| claude_code::load(&r))
                .unwrap_or_default(),
            SOURCE_CURSOR => cursor_user_dir()
                .map(|d| cursor::load(&d))
                .unwrap_or_default(),
            other => return Err(format!("unknown import source: {other}")),
        };
        open_history_store()?
            .import_conversations(&source, &conversations)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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

/// The all-time tokens compression saved (mode `on`) or would have saved
/// (mode `audit`) across every session — the Compression settings page's stat.
#[tauri::command]
pub(crate) async fn total_tokens_saved(state: State<'_, AppState>) -> Result<usize, String> {
    state.total_tokens_saved()
}

/// Estimated all-time Oxen cloud spend for the hero, using observed per-model
/// input/output tokens and current catalog rates. `None` when catalog pricing
/// is unavailable; local/custom endpoint usage is kept unpriced.
#[tauri::command]
pub(crate) async fn total_cost_usd() -> Result<Option<f64>, String> {
    let store = open_history_store()?;
    Ok(
        price_usage(store.model_usage_breakdown().map_err(|e| e.to_string())?)
            .await
            .total_cost_usd,
    )
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
pub(crate) async fn model_usage_breakdown(date: Option<String>) -> Result<UsageBreakdown, String> {
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
/// transcript scan runs at most once (the first time); afterwards each turn
/// just increments the counter, so reads and updates stay O(1).
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

/// Start a fresh chat session as its own agent. Any in-flight chats keep
/// running in the background — this never disturbs them. Returns the new
/// session's info.
#[tauri::command]
pub(crate) async fn new_session(state: State<'_, AppState>) -> Result<SessionInfo, String> {
    state.new_session().await
}

/// Switch to an existing session, returning its info and full transcript so
/// the UI can re-render the conversation. Reuses the session's live agent if
/// one exists; otherwise loads it cold from history. A chat still mid-turn
/// can't be locked, so its transcript comes back empty with `running: true`.
#[tauri::command]
pub(crate) async fn resume_session(
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionView, String> {
    let view = state.resume_session(&id).await?;
    // Opening a chat enters its project; remember it as the active one so new
    // chats land in the same directory across restarts.
    let _ = remember_project(&view.info.workspace);
    Ok(view)
}
