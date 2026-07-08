//! Tauri desktop bridge for oxen-harness.
//!
//! Exposes the agent loop to the web UI: the `run_turn` command drives
//! [`harness_agent::Agent`], emitting `agent://token` and `agent://tool` events
//! as the turn streams, and returning the assistant's final text. The agent is
//! initialized lazily on first use so the window always opens, even without an
//! API key configured.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use harness_agent::{Agent, AgentConfig, AgentEvent};
use harness_llm::{Attachment, ChatMessage, ChatRequest, OxenClient};
use harness_local::{
    fit, install_hint, install_llama_server, llama_server_path, LocalServer, ModelRef, ModelStore,
};
use harness_store::{HistoryStore, SessionMeta, SessionSummary};
use harness_tools::{
    AskUserTool, CanvasDoc, CanvasSink, CanvasTool, Question, QuestionAnswer, QuestionAsker,
    ToolError, ToolRegistry, Workspace,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

/// Outstanding `ask_user_question` prompts awaiting a UI answer, keyed by id.
type Pending = Arc<StdMutex<HashMap<String, oneshot::Sender<Vec<QuestionAnswer>>>>>;

/// Per-session agents, each behind its own lock so turns in different chats run
/// concurrently — a background chat keeps streaming while you start or read
/// another. The map lock is held only briefly to look an agent up; the turn
/// itself holds just that session's lock.
#[derive(Default)]
pub struct AppState {
    agents: Mutex<HashMap<String, Arc<Mutex<Agent>>>>,
    /// The session the UI currently shows. Commands that act on "this" chat
    /// (session_info, new_theme, model/connection switches) use it.
    current: Mutex<Option<String>>,
    /// A local `llama-server` kept alive while a local model is selected.
    local_server: Mutex<Option<LocalServer>>,
    /// The local model id in use, so new sessions reuse it instead of the cloud.
    local_model: Mutex<Option<String>>,
    /// The selected cloud model id, used for new sessions (and live swaps) when
    /// no local model is active. Seeded from the persisted selection at startup.
    cloud_model: Mutex<String>,
    /// The active project's directory — new chats are rooted here (the agent's
    /// workspace), so each project's chats run against its own codebase. Empty
    /// means "the launch directory" (resolved lazily by [`active_root`]).
    active_project: Mutex<PathBuf>,
    /// Questions the agent is currently waiting on the user to answer.
    pending: Pending,
    /// Stop signals for in-flight turns, keyed by session. Held here (not on the
    /// agent) so `cancel_turn` can fire one without taking the agent's lock,
    /// which the running turn holds for its whole duration.
    cancels: Mutex<HashMap<String, CancellationToken>>,
    /// Each session agent's `spawn_agents` spawner, so `execute_turn` can hand
    /// it the turn's stop signal — cancelling the turn then cancels any fleet
    /// the model launched inside it. Std mutex: touched briefly, from sync
    /// builders too. Evicted alongside the agents map.
    fleet_spawners: StdMutex<HashMap<String, Arc<harness_agent::FleetSpawner>>>,
}

/// Bridges the agent's `ask_user_question` tool to the web UI: emits an
/// `agent://question` event and parks on a channel until `answer_question`
/// delivers the user's selection (or the channel is dropped → no answer).
struct TauriAsker {
    app: AppHandle,
    pending: Pending,
}

#[derive(Clone, Serialize)]
struct QuestionPayload {
    id: String,
    questions: Vec<Question>,
}

#[async_trait]
impl QuestionAsker for TauriAsker {
    async fn ask(&self, questions: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = format!("q{}", COUNTER.fetch_add(1, Ordering::Relaxed));

        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .insert(id.clone(), tx);

        self.app
            .emit(
                "agent://question",
                QuestionPayload {
                    id: id.clone(),
                    questions: questions.to_vec(),
                },
            )
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        match rx.await {
            Ok(answers) => Ok(Some(answers)),
            Err(_) => {
                self.pending
                    .lock()
                    .expect("pending mutex poisoned")
                    .remove(&id);
                Ok(None)
            }
        }
    }
}

/// A session-only event payload (e.g. `agent://canvas-writing`).
#[derive(Clone, Serialize)]
struct SessionPayload {
    session: String,
}

/// The `agent://canvas` payload: a document for the UI's side panel, tagged with
/// the session it belongs to so a background chat's canvas doesn't pop into view.
#[derive(Clone, Serialize)]
struct CanvasPayload {
    session: String,
    id: String,
    title: String,
    format: String,
    language: Option<String>,
    content: String,
}

/// Bridges the agent's `canvas` tool to the desktop side panel: emits an
/// `agent://canvas` event with the document. One sink per agent, so it carries
/// that agent's session id.
struct TauriCanvasSink {
    app: AppHandle,
    session: String,
}

#[async_trait]
impl CanvasSink for TauriCanvasSink {
    async fn show(&self, doc: &CanvasDoc) -> Result<Option<String>, ToolError> {
        self.app
            .emit(
                "agent://canvas",
                CanvasPayload {
                    session: self.session.clone(),
                    id: doc.id.clone(),
                    title: doc.title.clone(),
                    format: doc.format.clone(),
                    language: doc.language.clone(),
                    content: doc.content.clone(),
                },
            )
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        // The panel itself is the user-visible result; no extra note needed.
        Ok(None)
    }
}

/// The installed local models plus disk + runtime context (for the manage view).
#[derive(Clone, Serialize)]
struct InstalledView {
    models: Vec<ModelRef>,
    /// Bytes used by downloaded models in the store directory.
    total_disk_bytes: u64,
    dir: String,
    runtime: harness_local::RuntimeStatus,
    /// Total bytes on the volume holding the model store (null if unknown).
    disk_total: Option<u64>,
    /// Free bytes on that volume — used to warn before a download won't fit.
    disk_free: Option<u64>,
}

/// One quant of a catalog model, annotated with how well it fits this machine
/// and the exact [`ModelRef`] to download it.
#[derive(Clone, Serialize)]
struct QuantOption {
    quant: String,
    size_bytes: u64,
    fit: harness_local::Fit,
    installed: bool,
    /// The concrete download reference the UI passes back to `download_model`.
    model: ModelRef,
}

/// A model offered in the setup wizard: a family with one or more quants, the
/// quant we recommend for this machine, and its source.
#[derive(Clone, Serialize)]
struct CatalogModel {
    id: String,
    display: String,
    params: String,
    context: u32,
    note: String,
    /// `"curated"`, `"huggingface"`, or `"oxen"`.
    source: String,
    quants: Vec<QuantOption>,
    /// The quant auto-picked for this machine (best quality that fits), if any.
    recommended_quant: Option<String>,
    /// The best fit achievable across this model's quants (for badges/sorting).
    best_fit: harness_local::Fit,
}

/// A `local://status` payload: a phase of bringing a local model online, so the
/// UI can show meaningful progress while switching to it.
#[derive(Clone, Serialize)]
struct LocalStatusPayload {
    model: String,
    /// `"starting"` (runtime/GPU init), `"loading"` (reading weights),
    /// `"ready"`, or `"error"` (the load ended without a server).
    phase: &'static str,
}

/// Report a local-model load phase to the UI (`local://status`).
fn emit_local_status(app: &AppHandle, model: &str, phase: &'static str) {
    let _ = app.emit(
        "local://status",
        LocalStatusPayload {
            model: model.to_string(),
            phase,
        },
    );
}

/// The `start_with_context` progress callback that streams load phases to the
/// UI — shared by the explicit model switch (`use_local_model`) and the lazy
/// restore of a persisted local selection on first use after launch
/// (`ensure_local_server`), so both render the same loading state.
fn local_status_emitter(app: &AppHandle, model: &str) -> impl FnMut(harness_local::LoadPhase) {
    let app = app.clone();
    let model = model.to_string();
    move |phase| {
        emit_local_status(
            &app,
            &model,
            match phase {
                harness_local::LoadPhase::Starting => "starting",
                harness_local::LoadPhase::LoadingModel => "loading",
                harness_local::LoadPhase::Ready => "ready",
            },
        )
    }
}

#[derive(Clone, Serialize)]
struct DownloadEvent {
    id: String,
    downloaded: u64,
    total: Option<u64>,
    fraction: Option<f64>,
}

/// A streamed assistant token, tagged with the session it belongs to so the UI
/// can route it to the right chat thread (even one running in the background).
#[derive(Clone, Serialize)]
struct TokenPayload {
    session: String,
    token: String,
}

#[derive(Clone, Serialize)]
struct ToolEventPayload {
    session: String,
    phase: &'static str,
    name: String,
    detail: String,
}

/// An `agent://tool-delta` payload: an incremental fragment of a tool call's
/// JSON arguments, so the UI can stream the in-progress content (a file being
/// written, a canvas document being authored).
#[derive(Clone, Serialize)]
struct ToolDeltaPayload {
    session: String,
    name: String,
    delta: String,
}

#[derive(Clone, Serialize)]
struct SessionInfo {
    model: String,
    workspace: String,
    session_id: String,
    /// Cumulative tokens used in this session, so the UI dashboard reflects real
    /// consumption rather than static flavor text.
    tokens_used: usize,
    /// Tokens the current transcript occupies (how full the context window is).
    context_tokens: usize,
    /// The model's effective context window, for a "% of context" readout.
    context_window: usize,
    /// The context-compression mode this session's agent was built with
    /// ("off"/"audit"/"on") — drives the TokenMeter's armed indicator.
    compression_mode: String,
}

/// An `agent://usage` payload: the session's cumulative token count plus current
/// context fill, emitted around each model call within a turn so the UI tracks
/// usage live. (The all-time grand total is a separate, turn-end concern.)
#[derive(Clone, Serialize)]
struct UsagePayload {
    session: String,
    tokens_used: usize,
    context_tokens: usize,
    context_window: usize,
}

/// `agent://compacted` payload — the transcript was trimmed to fit the window,
/// with a short human-readable note for the thread.
#[derive(Clone, Serialize)]
struct CompactedPayload {
    session: String,
    detail: String,
}

/// `agent://retry` payload — a model call hit a transient provider/network
/// error and is being retried with backoff. Surfaced as a thread notice so the
/// pause reads as a hiccup (with the error for debugging), not a hang.
#[derive(Clone, Serialize)]
struct RetryPayload {
    session: String,
    attempt: u32,
    max_attempts: u32,
    delay_ms: u64,
    error: String,
}

/// `agent://compression` payload — stale tool output was compressed before a
/// model call (`mode: "on"`), or its would-be savings were measured without
/// changing the request (`mode: "audit"`). Emitted per model call within a
/// turn, so the UI updates counters rather than appending thread notices.
#[derive(Clone, Serialize)]
struct CompressionPayload {
    session: String,
    mode: String,
    saved_tokens: usize,
    total_saved_tokens: usize,
    results_compressed: usize,
}

/// A resumed session: its info plus the verbatim transcript for the UI to
/// re-render (user/assistant bubbles and tool activity). When `running` is true
/// the chat is mid-turn and couldn't be read; `messages` is empty and the UI
/// keeps the live thread it already streamed.
#[derive(Serialize)]
struct SessionView {
    info: SessionInfo,
    messages: Vec<serde_json::Value>,
    running: bool,
}

/// The client, model label, and context window for a new agent: the selected
/// local model + server if one is active, otherwise the configured cloud client.
async fn client_for(
    app: &AppHandle,
    state: &AppState,
) -> Result<(OxenClient, String, Option<usize>), String> {
    // If a local model is selected (including one restored from a previous run),
    // make sure its server is running and use it.
    let local_id = state.local_model.lock().await.clone();
    if let Some(id) = local_id {
        match ensure_local_server(app, state, &id).await {
            Ok((base_url, ctx)) => {
                return Ok((OxenClient::new(base_url, "local", &id), id, Some(ctx)));
            }
            // The runtime or weights aren't available right now — fall back to the
            // cloud model for this session rather than failing to open a chat. The
            // persisted choice is kept, so it retries on the next launch. Tell the
            // UI the load ended (it may have shown loading phases already).
            Err(_) => {
                emit_local_status(app, &id, "error");
                *state.local_model.lock().await = None;
                *state.local_server.lock().await = None;
            }
        }
    }
    let model = state.cloud_model.lock().await.clone();
    Ok((build_client(&model)?, model, None))
}

/// Ensure a `llama-server` is running for local model `id`, returning its base
/// URL and context size. Reuses the running server if there is one; otherwise
/// validates the runtime + weights and starts it (sized to this machine). This
/// lets a restored local selection start lazily on first use — streaming the
/// same `local://status` load phases an explicit switch shows, so the composer
/// isn't silent while the model comes online after a restart.
async fn ensure_local_server(
    app: &AppHandle,
    state: &AppState,
    id: &str,
) -> Result<(String, usize), String> {
    let mut guard = state.local_server.lock().await;
    if let Some(s) = guard.as_ref() {
        return Ok((s.base_url().to_string(), s.context_size() as usize));
    }
    if llama_server_path().is_none() {
        return Err("the local runtime isn't installed".to_string());
    }
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    if !store.is_installed(id) {
        return Err(format!("{id} isn't downloaded"));
    }
    let profile = harness_local::detect_hardware();
    let weight_bytes = store.installed_size(id).unwrap_or(0);
    let native = store.native_context(id);
    let context = fit::plan_context(profile.usable_budget, weight_bytes, native);
    let server = LocalServer::start_with_context(
        &store.path_for(id),
        id,
        context,
        local_status_emitter(app, id),
    )
    .await
    .map_err(|e| e.to_string())?;
    let result = (
        server.base_url().to_string(),
        server.context_size() as usize,
    );
    *guard = Some(server);
    Ok(result)
}

/// Build an Oxen client honoring the user's saved connection settings, falling
/// back to env / the `oxen` CLI login / the default endpoint. Delegates to the
/// shared runtime so the CLI and desktop resolve the client identically.
fn build_client(model: &str) -> Result<OxenClient, String> {
    harness_runtime::connection::build_client(model).map_err(|e| e.to_string())
}

/// Shared agent dependencies — the tool registry (defaults + the question
/// bridge, *before* user preferences) and the history store. Fresh and resumed
/// agents build these the same way; only how they bind a session differs.
/// [`finish_tools`] completes the registry once the session id is known.
fn agent_parts(
    app: &AppHandle,
    pending: Pending,
    workspace_root: &Path,
) -> Result<(ToolRegistry, Arc<HistoryStore>), String> {
    let workspace = Workspace::new(workspace_root).map_err(|e| e.to_string())?;
    let brave_key = harness_runtime::connection::brave_key_override();
    let mut tools = ToolRegistry::default_for_workspace_with_web_key(workspace, brave_key);
    tools.register_typed(AskUserTool::new(Arc::new(TauriAsker {
        app: app.clone(),
        pending,
    })));

    let history_path = harness_config::paths::history_db().map_err(|e| e.to_string())?;
    let store = Arc::new(HistoryStore::open(history_path).map_err(|e| e.to_string())?);
    Ok((tools, store))
}

/// Complete a session's tool registry and derive its run config. Registers the
/// session-scoped `canvas` tool, applies the user's saved tool preferences to
/// the *complete* tool set (so disabling any tool — including ask/canvas —
/// sticks), adds the `skill` tool when enabled skills exist, and gates the
/// system prompt on what actually survived so the model is never told about a
/// tool the registry would reject.
fn finish_tools(
    tools: &mut ToolRegistry,
    app: &AppHandle,
    session: &str,
    model_label: &str,
    context_window: Option<usize>,
    workspace_root: &Path,
) -> AgentConfig {
    tools.register_typed(CanvasTool::new(Arc::new(TauriCanvasSink {
        app: app.clone(),
        session: session.to_string(),
    })));
    harness_runtime::tools::load().apply(tools);
    // Skills load on demand through the `skill` tool; it's only registered when
    // the user has enabled skills, so an empty set costs no prompt tokens.
    // Registered after prefs: skills have their own enable/disable in
    // skills.json, managed on the Skills settings page.
    if let Some(skill_tool) = harness_runtime::skills::enabled_tool(workspace_root) {
        tools.register_typed(skill_tool);
    }

    let web_search = tools.get(harness_tools::WEB_SEARCH_TOOL).is_some();
    let canvas = tools.get(harness_tools::CANVAS_TOOL).is_some();
    AgentConfig {
        model: model_label.to_string(),
        system_prompt: Some(harness_agent::system_prompt_with_env(
            web_search,
            canvas,
            workspace_root,
        )),
        context_window,
        attachment_root: Some(workspace_root.to_path_buf()),
        compression: harness_runtime::compression::mode(),
        ..AgentConfig::default()
    }
}

/// Register the `spawn_agents` tool on a session's registry: the spawner
/// snapshots the registry *before* the tool registers (subagents get every
/// tool except the fleet itself — one level deep, no fork bombs) and is kept
/// in [`AppState`] so `execute_turn` can wire each turn's stop signal to it.
/// Skipped entirely when the user disabled the tool in Settings → Tools.
fn register_fleet(
    app: &AppHandle,
    session: &str,
    tools: &mut ToolRegistry,
    client: &OxenClient,
    config: &AgentConfig,
) {
    if !harness_runtime::tools::load().is_enabled(harness_agent::FLEET_TOOL) {
        return;
    }
    let spawner = Arc::new(harness_agent::FleetSpawner::new(
        client.clone(),
        tools.clone(),
        config.clone(),
    ));
    tools.register_typed(harness_agent::FleetTool::new(
        spawner.clone(),
        Arc::new(TauriFleetSink {
            app: app.clone(),
            session: session.to_string(),
        }),
    ));
    app.state::<AppState>()
        .fleet_spawners
        .lock()
        .expect("fleet spawners poisoned")
        .insert(session.to_string(), spawner);
}

/// Build an agent for a brand-new session (creates the session row).
fn new_agent(
    app: &AppHandle,
    pending: Pending,
    client: OxenClient,
    model_label: &str,
    context_window: Option<usize>,
    workspace_root: &Path,
) -> Result<Agent, String> {
    let (mut tools, store) = agent_parts(app, pending, workspace_root)?;
    let session = store
        .create_session(&SessionMeta {
            workspace: workspace_root.display().to_string(),
            model: model_label.to_string(),
            provider: "oxen".into(),
            base_url: client.base_url().to_string(),
            context_window: context_window.map(|w| w as i64),
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
    let config = finish_tools(
        &mut tools,
        app,
        &session,
        model_label,
        context_window,
        workspace_root,
    );
    register_fleet(app, &session, &mut tools, &client, &config);
    Agent::new(client, tools, store, session, config).map_err(|e| e.to_string())
}

/// Build an agent bound to an *existing* session, loading its transcript — used
/// to resume a cold history session without leaking a throwaway session row.
/// Rooted at `workspace_root` (the session's own recorded directory).
fn resume_agent(
    app: &AppHandle,
    pending: Pending,
    client: OxenClient,
    model_label: &str,
    context_window: Option<usize>,
    session_id: String,
    workspace_root: &Path,
) -> Result<Agent, String> {
    let (mut tools, store) = agent_parts(app, pending, workspace_root)?;
    let config = finish_tools(
        &mut tools,
        app,
        &session_id,
        model_label,
        context_window,
        workspace_root,
    );
    register_fleet(app, &session_id, &mut tools, &client, &config);
    Agent::resume_from_store(client, tools, store, session_id, config).map_err(|e| e.to_string())
}

// ===========================================================================
// Projects — chats are grouped by their working directory. A "project" is a
// directory the agent runs in; entering one roots new chats there. The set of
// known projects (plus the active one) is persisted to `projects.json`, and
// merged with the distinct workspaces found across existing chats so directories
// that already have history always show up.
// ===========================================================================

/// The directory the app was launched from — the default/initial project.
fn launch_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The active project's directory, falling back to the launch directory.
async fn active_root(state: &AppState) -> PathBuf {
    let p = state.active_project.lock().await.clone();
    if p.as_os_str().is_empty() {
        launch_dir()
    } else {
        p
    }
}

/// A friendly display name for a project directory (its last path segment).
fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}

#[derive(Default, Serialize, Deserialize)]
struct ProjectsConfig {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    active: Option<String>,
}

/// A project shown in the UI: its directory, display name, chat count, whether
/// it's the active one.
#[derive(Clone, Serialize)]
struct ProjectView {
    path: String,
    name: String,
    session_count: usize,
    active: bool,
}

/// Schema version for `projects.json` (bump when the shape changes).
const PROJECTS_SCHEMA_VERSION: u32 = 1;

fn projects_config_path() -> Result<PathBuf, String> {
    harness_config::paths::projects_file().map_err(|e| e.to_string())
}

fn read_projects_config() -> ProjectsConfig {
    match projects_config_path() {
        Ok(path) => harness_config::read_versioned::<ProjectsConfig>(&path).1,
        Err(_) => ProjectsConfig::default(),
    }
}

fn write_projects_config(cfg: &ProjectsConfig) -> Result<(), String> {
    let path = projects_config_path()?;
    harness_config::write_versioned(&path, PROJECTS_SCHEMA_VERSION, cfg)
        .map_err(|e| e.to_string())?;
    harness_runtime::config_repo::snapshot("Update projects");
    Ok(())
}

/// Record `path` as a known project and make it active (persisted).
fn remember_project(path: &str) -> Result<(), String> {
    let mut cfg = read_projects_config();
    if !cfg.paths.iter().any(|p| p == path) {
        cfg.paths.push(path.to_string());
    }
    cfg.active = Some(path.to_string());
    write_projects_config(&cfg)
}

// ===========================================================================
// Connection settings — a persisted Oxen API key + host override, editable
// from the desktop Settings page. Blank fields fall back to env / CLI login.
// ===========================================================================

// Connection settings live in `harness_runtime::connection`: the non-secret host
// in `connection.json`, the API/Brave keys in `~/.oxen-harness/.env`. The
// commands below are thin wrappers so the CLI and desktop resolve a client the
// same way (no drift) and secrets stay out of the versioned config.

#[tauri::command]
fn get_connection() -> harness_runtime::connection::ConnectionView {
    harness_runtime::connection::view()
}

/// Save just the Brave Search API key and apply it to the running agent.
///
/// Unlike [`set_connection`], this does **not** rebuild the agent or start a new
/// session — it persists the key (to `.env`) and sets `BRAVE_API_KEY` in the
/// process so the already-registered `web_search` tool picks it up on its next
/// call. Lets the user fix a failed web search inline and retry in the same chat.
#[tauri::command]
fn configure_brave_key(key: String) -> Result<(), String> {
    harness_runtime::connection::set_brave_key(&key).map_err(|e| e.to_string())
}

/// Save the Oxen API key and authenticate a chat's running agent in place.
///
/// Unlike [`set_connection`], this does **not** start a new session — it persists
/// the key (to `.env`) and swaps a freshly-built client (same model, now carrying
/// the key) into the session's agent, keeping the transcript intact. Lets the
/// user paste a key inline after a 401 and retry the turn in the same chat.
#[tauri::command]
async fn configure_oxen_key(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
    key: String,
) -> Result<(), String> {
    harness_runtime::connection::set_oxen_key(&key).map_err(|e| e.to_string())?;
    let arc = agent_or_build(&app, &state, &session).await?;
    let mut agent = arc.lock().await;
    // Rebuild the client for the agent's own model so the key applies without
    // disturbing the model choice or the conversation.
    let client = build_client(agent.model())?;
    agent.set_client(client);
    Ok(())
}

/// Save the Oxen API key + host and rebuild the agent against the new endpoint.
///
/// Rebuilding validates that a key resolves (a blank key must be backed by env /
/// CLI login), drops any active local-model server, and — since the endpoint may
/// have changed — starts a fresh session. Returns the new session info.
#[tauri::command]
async fn set_connection(
    app: AppHandle,
    state: State<'_, AppState>,
    host: String,
    api_key: String,
    brave_api_key: String,
) -> Result<SessionInfo, String> {
    harness_runtime::connection::save(&host, &api_key, &brave_api_key)
        .map_err(|e| e.to_string())?;

    // A connection change drops any local model and starts fresh on the cloud
    // endpoint using the selected cloud model.
    *state.local_server.lock().await = None;
    *state.local_model.lock().await = None;
    let _ = harness_runtime::models::set_active_local("");
    let model = state.cloud_model.lock().await.clone();
    let root = active_root(&state).await;
    let agent = new_agent(
        &app,
        state.pending.clone(),
        build_client(&model)?,
        &model,
        None,
        &root,
    )?;
    Ok(install_agent(&state, agent).await)
}

/// Build a fresh agent for a new session rooted at `root`, reusing the active
/// local model if any.
async fn build_fresh_agent(
    app: &AppHandle,
    state: &AppState,
    root: &Path,
) -> Result<Agent, String> {
    let (client, label, ctx) = client_for(app, state).await?;
    new_agent(app, state.pending.clone(), client, &label, ctx, root)
}

/// Build an agent bound to an existing session id, rooted at `root` (its own
/// recorded workspace), without leaking a throwaway session row.
async fn build_resumed_agent(
    app: &AppHandle,
    state: &AppState,
    session_id: String,
    root: &Path,
) -> Result<Agent, String> {
    let (client, label, ctx) = client_for(app, state).await?;
    resume_agent(
        app,
        state.pending.clone(),
        client,
        &label,
        ctx,
        session_id,
        root,
    )
}

/// The agent handle for a session id, if one is live in memory.
async fn agent_for(state: &AppState, id: &str) -> Option<Arc<Mutex<Agent>>> {
    state.agents.lock().await.get(id).cloned()
}

/// A session's recorded working directory (its project), read from the store;
/// falls back to the launch directory when unknown.
fn session_workspace(id: &str) -> PathBuf {
    open_history_store()
        .ok()
        .and_then(|s| s.session_meta(id).ok())
        .map(|m| PathBuf::from(m.workspace))
        .unwrap_or_else(launch_dir)
}

/// The live agent for a session, rehydrating it from the database if it isn't
/// cached (e.g. it was evicted, or this is the first turn after a cold resume).
/// The DB is the source of truth — every message was persisted as it was made —
/// so a rebuilt agent continues the exact conversation, in the session's own
/// workspace. Inserts via the map entry so a concurrent build can't duplicate.
async fn agent_or_build(
    app: &AppHandle,
    state: &AppState,
    session: &str,
) -> Result<Arc<Mutex<Agent>>, String> {
    if let Some(a) = agent_for(state, session).await {
        return Ok(a);
    }
    let root = session_workspace(session);
    let agent = build_resumed_agent(app, state, session.to_string(), &root).await?;
    let arc = Arc::new(Mutex::new(agent));
    Ok(state
        .agents
        .lock()
        .await
        .entry(session.to_string())
        .or_insert(arc)
        .clone())
}

fn info_for(agent: &Agent) -> SessionInfo {
    SessionInfo {
        model: agent.model().to_string(),
        workspace: session_workspace(agent.session_id()).display().to_string(),
        session_id: agent.session_id().to_string(),
        tokens_used: agent.tokens_used(),
        context_tokens: agent.context_tokens(),
        context_window: agent.context_window(),
        compression_mode: agent.compression_mode().as_str().to_string(),
    }
}

/// Release cached agents we don't need in memory: everything except the current
/// chat and any whose turn is still running (whose per-session lock is held).
/// The dropped chats live on in SQLite and rehydrate via [`agent_or_build`], so
/// resident memory tracks concurrency (running turns + the open chat), never the
/// number of chats in history.
async fn evict_idle(state: &AppState) {
    let current = { state.current.lock().await.clone() };
    let kept: std::collections::HashSet<String> = {
        let mut agents = state.agents.lock().await;
        agents
            .retain(|id, arc| Some(id.as_str()) == current.as_deref() || arc.try_lock().is_err());
        agents.keys().cloned().collect()
    };
    // The fleet-spawner map mirrors the agents map (an evicted chat's spawner
    // rebuilds with its agent), so evict in lockstep.
    state
        .fleet_spawners
        .lock()
        .expect("fleet spawners poisoned")
        .retain(|id, _| kept.contains(id));
}

/// Register an agent under its session id, make it the current chat, then evict
/// any now-idle background agents.
async fn install_agent(state: &AppState, agent: Agent) -> SessionInfo {
    let info = info_for(&agent);
    state
        .agents
        .lock()
        .await
        .insert(info.session_id.clone(), Arc::new(Mutex::new(agent)));
    *state.current.lock().await = Some(info.session_id.clone());
    evict_idle(state).await;
    info
}

/// The current chat's agent, lazily building one on first use so the window
/// always opens even without an API key configured.
async fn current_agent(app: &AppHandle, state: &AppState) -> Result<Arc<Mutex<Agent>>, String> {
    // Read + drop the `current` guard before locking `agents` — never hold both,
    // so the two maps can't form a lock-ordering cycle.
    let current = { state.current.lock().await.clone() };
    if let Some(id) = current {
        if let Some(a) = agent_for(state, &id).await {
            return Ok(a);
        }
    }
    let root = active_root(state).await;
    let agent = build_fresh_agent(app, state, &root).await?;
    let arc = Arc::new(Mutex::new(agent));
    let id = arc.lock().await.session_id().to_string();
    state.agents.lock().await.insert(id.clone(), arc.clone());
    *state.current.lock().await = Some(id);
    Ok(arc)
}

/// A `review://progress` payload: which pipeline step a running code review is
/// on — and, for a fan-out step, the parallel lanes it runs — so the chat can
/// show a live progress card.
#[derive(Clone, Serialize)]
struct ReviewProgressPayload {
    session: String,
    step: String,
    index: usize,
    total: usize,
    /// Lane labels for this step, in order. More than one = a fan-out.
    agents: Vec<String>,
}

/// A `fleet://started` payload: a fleet of parallel subagents is spinning up
/// in `session` — from a review fan-out step or the model's `spawn_agents`
/// call alike. `agents` is the lane labels, in order.
#[derive(Clone, Serialize)]
struct FleetStartedPayload {
    session: String,
    agents: Vec<String>,
    /// `"review"` (a pipeline step) or `"turn"` (the model's spawn_agents).
    source: &'static str,
}

/// A `fleet://agent` payload: one lane changed state.
#[derive(Clone, Serialize)]
struct FleetAgentPayload {
    session: String,
    agent: usize,
    name: String,
    /// `"started"`, `"done"`, or `"failed"`.
    phase: &'static str,
    tokens: usize,
    /// The lane's truncated reply or error (set on done/failed).
    summary: String,
}

/// A `fleet://agent-activity` payload: what one lane is doing right now —
/// streamed text, a tool invocation, or a token-count update.
#[derive(Clone, Serialize)]
struct FleetActivityPayload {
    session: String,
    agent: usize,
    /// `"token"` (append text), `"tool"` (replace with a tool line), or
    /// `"tokens"` (update the counter).
    kind: &'static str,
    text: String,
    tokens: Option<usize>,
}

/// Bridges a `spawn_agents` fleet (run by the model from inside a turn) to the
/// UI's lanes panel: one sink per agent, tagged with its session, emitting the
/// same `fleet://` events review fan-out steps use.
struct TauriFleetSink {
    app: AppHandle,
    session: String,
}

impl harness_agent::fleet::FleetSink for TauriFleetSink {
    fn started(&self, labels: &[String], _cancel: CancellationToken) {
        let _ = self.app.emit(
            "fleet://started",
            FleetStartedPayload {
                session: self.session.clone(),
                agents: labels.to_vec(),
                source: "turn",
            },
        );
    }

    fn event(&self, event: &harness_agent::fleet::FleetEvent) {
        use harness_agent::fleet::FleetEvent;
        match event {
            FleetEvent::TaskStarted { index, label } => {
                let _ = self.app.emit(
                    "fleet://agent",
                    FleetAgentPayload {
                        session: self.session.clone(),
                        agent: *index,
                        name: label.clone(),
                        phase: "started",
                        tokens: 0,
                        summary: String::new(),
                    },
                );
            }
            FleetEvent::Agent { index, event } => {
                if let Some((kind, text, tokens)) = activity_payload(event) {
                    let _ = self.app.emit(
                        "fleet://agent-activity",
                        FleetActivityPayload {
                            session: self.session.clone(),
                            agent: *index,
                            kind,
                            text,
                            tokens,
                        },
                    );
                }
            }
            FleetEvent::TaskCompleted {
                index,
                label,
                ok,
                tokens_used,
                summary,
            } => {
                let _ = self.app.emit(
                    "fleet://agent",
                    FleetAgentPayload {
                        session: self.session.clone(),
                        agent: *index,
                        name: label.clone(),
                        phase: if *ok { "done" } else { "failed" },
                        tokens: *tokens_used,
                        summary: summary.clone(),
                    },
                );
            }
        }
    }

    fn finished(&self) {
        let _ = self.app.emit(
            "fleet://completed",
            SessionPayload {
                session: self.session.clone(),
            },
        );
    }
}

/// The lane-activity slice of one subagent event, if it has one.
fn activity_payload(event: &AgentEvent) -> Option<(&'static str, String, Option<usize>)> {
    match event {
        AgentEvent::Token(t) => Some(("token", t.clone(), None)),
        AgentEvent::ToolStart { name, .. } => Some(("tool", name.clone(), None)),
        AgentEvent::Usage { tokens_used, .. } => Some(("tokens", String::new(), Some(*tokens_used))),
        _ => None,
    }
}

/// A `review://token` payload: streamed text from the current review step's
/// agent (the card's live activity feed).
#[derive(Clone, Serialize)]
struct ReviewTokenPayload {
    session: String,
    token: String,
}

/// A `review://tool` payload: a tool the current review step's agent invoked.
#[derive(Clone, Serialize)]
struct ReviewToolPayload {
    session: String,
    name: String,
}

/// What `run_code_review` resolves with. `status` is `"ok"`, `"nothing"` (the
/// target had no changes), or `"cancelled"`; on `"ok"` the user/assistant pair
/// is already persisted to the session, so the UI appends it to the thread.
#[derive(Clone, Serialize)]
struct CodeReviewResult {
    status: &'static str,
    user: String,
    assistant: String,
    findings: usize,
    /// Estimated tokens spent across every reviewer agent in the pipeline.
    tokens_used: usize,
}

/// Run the configurable code-review pipeline for a chat's workspace: uncommitted
/// changes by default, or PR-style against `base_branch`. Streams progress via
/// `review://progress` / `review://token` / `review://tool`, then injects the
/// findings into the session (as a settled user/assistant exchange) so follow-up
/// turns can act on them ("fix 1 and 3"). Holds the session's agent lock for the
/// duration, so it can't interleave with a running turn; `cancel_turn` stops it.
#[tauri::command]
async fn run_code_review(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
    base_branch: Option<String>,
) -> Result<CodeReviewResult, String> {
    use harness_review::{ReviewError, ReviewEvent};

    let arc = agent_or_build(&app, &state, &session).await?;
    let root = session_workspace(&session);
    let target = match base_branch.filter(|b| !b.trim().is_empty()) {
        Some(branch) => harness_review::ReviewTarget::BaseBranch(branch.trim().to_string()),
        None => harness_review::ReviewTarget::Uncommitted,
    };

    let cancel = CancellationToken::new();
    {
        // Registering under the session's key is also the mutual-exclusion
        // check: an existing entry means a turn (or another review) is already
        // in flight, and overwriting its token would orphan its stop button.
        let mut cancels = state.cancels.lock().await;
        if cancels.contains_key(&session) {
            return Err("a turn is already running in this chat".to_string());
        }
        cancels.insert(session.clone(), cancel.clone());
    }

    let result = {
        let mut agent = arc.lock().await;
        let runner = harness_review::ReviewRunner::new(
            harness_review::ReviewConfig::load(),
            target.clone(),
            &root,
        )
        .with_cancel(cancel);
        let sid = session.clone();
        let emitter = app.clone();
        let mut pipeline_tokens = 0usize;
        let run = runner
            .run(&agent, |event| match event {
                ReviewEvent::StepStarted {
                    index,
                    total,
                    name,
                    agents,
                } => {
                    let _ = emitter.emit(
                        "review://progress",
                        ReviewProgressPayload {
                            session: sid.clone(),
                            step: name.clone(),
                            index: *index,
                            total: *total,
                            agents: agents.clone(),
                        },
                    );
                    // A fan-out step opens a lanes panel like spawn_agents does.
                    if agents.len() > 1 {
                        let _ = emitter.emit(
                            "fleet://started",
                            FleetStartedPayload {
                                session: sid.clone(),
                                agents: agents.clone(),
                                source: "review",
                            },
                        );
                    }
                }
                ReviewEvent::Agent(AgentEvent::Token(t)) => {
                    let _ = emitter.emit(
                        "review://token",
                        ReviewTokenPayload {
                            session: sid.clone(),
                            token: t.clone(),
                        },
                    );
                }
                ReviewEvent::Agent(AgentEvent::ToolStart { name, .. }) => {
                    let _ = emitter.emit(
                        "review://tool",
                        ReviewToolPayload {
                            session: sid.clone(),
                            name: name.clone(),
                        },
                    );
                }
                ReviewEvent::SubagentStarted { agent, name } => {
                    let _ = emitter.emit(
                        "fleet://agent",
                        FleetAgentPayload {
                            session: sid.clone(),
                            agent: *agent,
                            name: name.clone(),
                            phase: "started",
                            tokens: 0,
                            summary: String::new(),
                        },
                    );
                }
                ReviewEvent::Subagent { agent, event } => {
                    if let Some((kind, text, tokens)) = activity_payload(event) {
                        let _ = emitter.emit(
                            "fleet://agent-activity",
                            FleetActivityPayload {
                                session: sid.clone(),
                                agent: *agent,
                                kind,
                                text,
                                tokens,
                            },
                        );
                    }
                }
                ReviewEvent::SubagentCompleted {
                    agent,
                    name,
                    ok,
                    tokens_used,
                    summary,
                } => {
                    let _ = emitter.emit(
                        "fleet://agent",
                        FleetAgentPayload {
                            session: sid.clone(),
                            agent: *agent,
                            name: name.clone(),
                            phase: if *ok { "done" } else { "failed" },
                            tokens: *tokens_used,
                            summary: summary.clone(),
                        },
                    );
                }
                ReviewEvent::StepCompleted { .. } => {
                    let _ = emitter.emit(
                        "fleet://completed",
                        SessionPayload {
                            session: sid.clone(),
                        },
                    );
                }
                ReviewEvent::Completed { tokens_used, .. } => pipeline_tokens = *tokens_used,
                _ => {}
            })
            .await;
        match run {
            Ok(report) => {
                let (user, assistant) = harness_review::session_exchange(&target, &report);
                agent
                    .inject_exchange(user.clone(), assistant.clone())
                    .map_err(|e| e.to_string())?;
                Ok(CodeReviewResult {
                    status: "ok",
                    user,
                    assistant,
                    findings: report.findings.len(),
                    tokens_used: pipeline_tokens,
                })
            }
            Err(ReviewError::NothingToReview) => Ok(CodeReviewResult {
                status: "nothing",
                user: String::new(),
                assistant: String::new(),
                findings: 0,
                tokens_used: 0,
            }),
            Err(ReviewError::Cancelled) => Ok(CodeReviewResult {
                status: "cancelled",
                user: String::new(),
                assistant: String::new(),
                findings: 0,
                tokens_used: 0,
            }),
            Err(e) => Err(e.to_string()),
        }
    };
    state.cancels.lock().await.remove(&session);
    // Reviewer agents are real spend: roll their tokens into the all-time
    // total (the same counter turns feed).
    if let Ok(res) = &result {
        let _ = bump_total_tokens(res.tokens_used);
    }
    evict_idle(&state).await;
    result
}

/// The saved code-review pipeline (steps + findings cap) for the Settings page.
#[tauri::command]
fn get_code_review_config() -> harness_review::ReviewConfig {
    harness_review::ReviewConfig::load()
}

/// Persist the code-review pipeline and snapshot the versioned config repo.
/// Applies to the next review — a running one keeps the steps it started with.
#[tauri::command]
fn save_code_review_config(config: harness_review::ReviewConfig) -> Result<(), String> {
    config.save().map_err(|e| e.to_string())?;
    harness_runtime::config_repo::snapshot("Update code review settings");
    Ok(())
}

/// The built-in default pipeline, for the Settings page's "reset to defaults".
#[tauri::command]
fn default_code_review_config() -> harness_review::ReviewConfig {
    harness_review::ReviewConfig::default()
}

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

/// Run one user turn for a specific chat, streaming session-tagged events to the
/// UI; returns the final text. Holds only that session's lock, so turns in other
/// chats keep running concurrently.
#[tauri::command]
async fn run_turn(
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
async fn retry_turn(
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
    // Track the session's cumulative counts before/after the turn so we can roll
    // just this turn's throughput (and compression savings) into the all-time
    // totals.
    let turn_delta;
    let saved_delta;
    let result = {
        let mut agent = arc.lock().await;
        agent.set_cancel_token(cancel.clone());
        let before = agent.tokens_used();
        let saved_before = agent.tokens_saved();
        let on_event = move |event: &AgentEvent| match event {
            AgentEvent::Token(t) => {
                let _ = app.emit(
                    "agent://token",
                    TokenPayload {
                        session: sid.clone(),
                        token: t.clone(),
                    },
                );
            }
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
            } => {
                let _ = app.emit(
                    "agent://usage",
                    UsagePayload {
                        session: sid.clone(),
                        tokens_used: *tokens_used,
                        context_tokens: *context_tokens,
                        context_window,
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
        turn_delta = agent.tokens_used().saturating_sub(before);
        saved_delta = agent.tokens_saved().saturating_sub(saved_before);
        r
    };
    // The turn is over (finished, stopped, or errored): drop its stop signal so a
    // later `cancel_turn` can't fire against a stale token.
    state.cancels.lock().await.remove(&session);
    // Roll this turn's throughput into the all-time running total (a cheap
    // persisted counter); the hero refreshes that grand total after the turn.
    let _ = bump_total_tokens(turn_delta);
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
async fn cancel_turn(state: State<'_, AppState>, session: String) -> Result<(), String> {
    if let Some(token) = state.cancels.lock().await.get(&session) {
        token.cancel();
    }
    Ok(())
}

/// Report the current session info, initializing the agent if needed.
#[tauri::command]
async fn session_info(app: AppHandle, state: State<'_, AppState>) -> Result<SessionInfo, String> {
    let arc = current_agent(&app, &state).await?;
    let agent = arc.lock().await;
    Ok(info_for(&agent))
}

/// Open the shared on-disk history store (same DB the agents persist to).
fn open_history_store() -> Result<HistoryStore, String> {
    let path = harness_config::paths::history_db().map_err(|e| e.to_string())?;
    HistoryStore::open(path).map_err(|e| e.to_string())
}

/// List past chat sessions (those with at least one user message), newest first.
#[tauri::command]
async fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    open_history_store()?
        .list_sessions()
        .map_err(|e| e.to_string())
}

/// Read a session's raw, persisted transcript (every message, verbatim — system
/// prompt, tool calls, and tool results included) straight from the history
/// store, for the developer inspector. Read-only and never touches the live
/// agent, so it works even while a turn is mid-flight.
#[tauri::command]
async fn session_messages(id: String) -> Result<Vec<serde_json::Value>, String> {
    open_history_store()?
        .messages(&id)
        .map_err(|e| e.to_string())
}

/// Set a chat's training-data review status: `""` (unreviewed), `"kept"`, or
/// `"rejected"`. Persisted so the dataset builder's decisions survive restarts.
#[tauri::command]
async fn set_review_status(id: String, status: String) -> Result<(), String> {
    open_history_store()?
        .set_review_status(&id, &status)
        .map_err(|e| e.to_string())
}

/// Bulk-set the review status for many chats at once (bulk keep/reject/clear
/// from the dataset builder). Returns how many rows changed.
#[tauri::command]
async fn set_review_status_many(ids: Vec<String>, status: String) -> Result<usize, String> {
    open_history_store()?
        .set_review_status_many(&ids, &status)
        .map_err(|e| e.to_string())
}

/// Permanently delete a chat session: remove it (and its messages) from history,
/// drop any cached live agent, and clear it as the current chat if it was active.
#[tauri::command]
async fn delete_session(state: State<'_, AppState>, id: String) -> Result<(), String> {
    open_history_store()?
        .delete_session(&id)
        .map_err(|e| e.to_string())?;
    state.agents.lock().await.remove(&id);
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
async fn attachment_data_uri(path: String, session: Option<String>) -> Result<String, String> {
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

/// The tool definitions (JSON schemas) the current session's agent advertises to
/// the model on every call — surfaced in the developer view so the full request
/// (transcript + tools) is inspectable. These aren't persisted per-message, so
/// we read them from the live agent.
#[tauri::command]
async fn tool_definitions(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let arc = current_agent(&app, &state).await?;
    let agent = arc.lock().await;
    Ok(agent.tool_definitions())
}

// ===========================================================================
// Tools — manage which built-in tools the agent may call and the descriptions
// it sees for them. Preferences persist to `tools.json` and are applied when an
// agent's registry is built (see `agent_parts`), so changes take effect for new
// and resumed chats.
// ===========================================================================

/// An inert question bridge for the settings registry — never invoked; only the
/// tool's name/description/schema are read.
struct NullAsker;

#[async_trait::async_trait]
impl harness_tools::QuestionAsker for NullAsker {
    async fn ask(
        &self,
        _: &[harness_tools::Question],
    ) -> Result<Option<Vec<harness_tools::QuestionAnswer>>, harness_tools::ToolError> {
        Ok(None)
    }
}

/// An inert canvas bridge for the settings registry — never invoked.
struct NullCanvasSink;

#[async_trait::async_trait]
impl harness_tools::CanvasSink for NullCanvasSink {
    async fn show(
        &self,
        _: &harness_tools::CanvasDoc,
    ) -> Result<Option<String>, harness_tools::ToolError> {
        Ok(None)
    }
}

/// The complete tool set for settings purposes: the workspace defaults plus the
/// host-bridged ask/canvas tools (wired to inert bridges — the Tools page only
/// reads names, descriptions, and schemas). Matches what [`finish_tools`]
/// registers on a real agent, so every manageable tool appears in Settings and
/// custom-tool names can't shadow any of them.
async fn settings_registry(state: &AppState) -> Result<ToolRegistry, String> {
    let root = active_root(state).await;
    let workspace = Workspace::new(&root).map_err(|e| e.to_string())?;
    let brave_key = harness_runtime::connection::brave_key_override();
    let mut registry = ToolRegistry::default_for_workspace_with_web_key(workspace, brave_key);
    registry.register_typed(AskUserTool::new(Arc::new(NullAsker)));
    registry.register_typed(CanvasTool::new(Arc::new(NullCanvasSink)));
    // An inert `spawn_agents` (never run — the page only reads name,
    // description, and schema), so the fleet is manageable like any tool.
    registry.register_typed(harness_agent::FleetTool::new(
        Arc::new(harness_agent::FleetSpawner::new(
            OxenClient::new("http://localhost", "", ""),
            ToolRegistry::new(),
            AgentConfig::default(),
        )),
        Arc::new(NullFleetSink),
    ));
    Ok(registry)
}

/// Inert fleet sink for [`settings_registry`]'s listing-only `spawn_agents`.
struct NullFleetSink;

impl harness_agent::fleet::FleetSink for NullFleetSink {
    fn started(&self, _labels: &[String], _cancel: CancellationToken) {}
    fn event(&self, _event: &harness_agent::fleet::FleetEvent) {}
    fn finished(&self) {}
}

/// Every manageable tool with its current enabled/override state, for the Tools
/// settings page. Built from a fresh full registry (so disabled tools still
/// appear, toggled off) overlaid with the saved preferences.
#[tauri::command]
async fn list_tools(
    state: State<'_, AppState>,
) -> Result<Vec<harness_runtime::tools::ToolInfo>, String> {
    let registry = settings_registry(&state).await?;
    let prefs = harness_runtime::tools::load();
    Ok(harness_runtime::tools::list(&registry, &prefs))
}

/// Add or update a custom HTTP POST tool. Takes effect for new/resumed chats.
#[tauri::command]
async fn add_custom_tool(
    state: State<'_, AppState>,
    spec: harness_tools::CustomToolSpec,
) -> Result<(), String> {
    let registry = settings_registry(&state).await?;
    harness_runtime::tools::add_custom(spec, &registry).map_err(|e| e.to_string())
}

/// Remove a custom tool. Built-ins cannot be removed, only disabled.
#[tauri::command]
async fn remove_custom_tool(name: String) -> Result<(), String> {
    harness_runtime::tools::remove_custom(&name).map_err(|e| e.to_string())
}

/// Every skill visible from the active project (global + project scope, with
/// project shadowing), for the Skills settings page.
#[tauri::command]
async fn list_skills(
    state: State<'_, AppState>,
) -> Result<Vec<harness_runtime::skills::SkillInfo>, String> {
    let root = active_root(&state).await;
    let prefs = harness_runtime::skills::load();
    Ok(harness_runtime::skills::list(&root, &prefs))
}

/// Create or update a skill's SKILL.md. Takes effect for new/resumed chats.
#[tauri::command]
async fn save_skill(
    state: State<'_, AppState>,
    scope: harness_tools::SkillScope,
    name: String,
    description: String,
    instructions: String,
) -> Result<(), String> {
    let root = active_root(&state).await;
    harness_runtime::skills::save_skill(&root, scope, &name, &description, &instructions)
        .map_err(|e| e.to_string())
}

/// Delete a skill's directory (SKILL.md plus any supporting files).
#[tauri::command]
async fn delete_skill(
    state: State<'_, AppState>,
    scope: harness_tools::SkillScope,
    name: String,
) -> Result<(), String> {
    let root = active_root(&state).await;
    harness_runtime::skills::delete_skill(&root, scope, &name).map_err(|e| e.to_string())
}

/// Enable or disable a skill. Takes effect for new/resumed chats.
#[tauri::command]
async fn set_skill_enabled(name: String, enabled: bool) -> Result<(), String> {
    let mut prefs = harness_runtime::skills::load();
    if prefs.set_enabled(&name, enabled) {
        harness_runtime::skills::save(&prefs).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Enable or disable a built-in tool. Takes effect for new/resumed chats.
#[tauri::command]
async fn set_tool_enabled(name: String, enabled: bool) -> Result<(), String> {
    let mut prefs = harness_runtime::tools::load();
    if prefs.set_enabled(&name, enabled) {
        harness_runtime::tools::save(&prefs).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Override (or clear, with `None`/blank) the description the model sees for a
/// tool. Takes effect for new/resumed chats.
#[tauri::command]
async fn set_tool_description(name: String, description: Option<String>) -> Result<(), String> {
    let mut prefs = harness_runtime::tools::load();
    if prefs.set_description(&name, description.as_deref()) {
        harness_runtime::tools::save(&prefs).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The persisted context-compression mode: `"off"`, `"audit"`, or `"on"`.
#[tauri::command]
async fn get_compression_mode() -> String {
    harness_runtime::compression::mode().as_str().to_string()
}

/// Set the context-compression mode: persist it for new chats AND apply it to
/// the live conversation in place (mirroring `set_model`), so a meter toggle
/// takes effect on the very next model call. Returns the refreshed session
/// info carrying the now-current mode.
#[tauri::command]
async fn set_compression_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: String,
) -> Result<SessionInfo, String> {
    let mode = harness_compress::CompressionMode::from_str_or_off(&mode);
    harness_runtime::compression::set_mode(mode).map_err(|e| e.to_string())?;
    let arc = current_agent(&app, &state).await?;
    let mut agent = arc.lock().await;
    agent.set_compression_mode(mode);
    Ok(info_for(&agent))
}

/// Export the given sessions as chat-completions fine-tuning JSONL (Oxen.ai
/// format: one `{"messages":[…]}` conversation per line) to `path`. Returns the
/// number of conversations written. `include_tools` keeps tool calls + results.
#[tauri::command]
async fn export_finetuning(
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
async fn total_tokens_used() -> Result<usize, String> {
    let store = open_history_store()?;
    Ok(ensure_total_tokens(&store)?.max(0) as usize)
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
fn bump_total_tokens(delta: usize) -> usize {
    let Ok(store) = open_history_store() else {
        return 0;
    };
    let _ = ensure_total_tokens(&store);
    store
        .meta_add_i64(TOTAL_TOKENS_KEY, delta as i64)
        .map(|v| v.max(0) as usize)
        .unwrap_or(0)
}

/// The `app_meta` key holding the all-time tokens saved by context compression.
const TOTAL_TOKENS_SAVED_KEY: &str = "total_tokens_saved";

/// The all-time tokens compression saved (mode `on`) or would have saved
/// (mode `audit`) across every session — the Compression settings page's stat.
/// No backfill: savings only exist from the moment the feature ships, so the
/// counter simply starts at 0.
#[tauri::command]
async fn total_tokens_saved() -> Result<usize, String> {
    let store = open_history_store()?;
    Ok(store
        .meta_get_i64(TOTAL_TOKENS_SAVED_KEY)
        .map_err(|e| e.to_string())?
        .unwrap_or(0)
        .max(0) as usize)
}

/// Add a turn's compression savings to the all-time counter and return the new
/// grand total. Best-effort: never fails a turn.
fn bump_total_tokens_saved(delta: usize) -> usize {
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
async fn new_session(app: AppHandle, state: State<'_, AppState>) -> Result<SessionInfo, String> {
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
async fn resume_session(
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

/// The installed local models, total disk used, and the runtime status.
#[tauri::command]
async fn installed_local_models() -> Result<InstalledView, String> {
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    let (disk_total, disk_free) = match harness_local::disk_space(store.dir()) {
        Some((total, free)) => (Some(total), Some(free)),
        None => (None, None),
    };
    Ok(InstalledView {
        models: store.installed(),
        total_disk_bytes: store.total_disk_used(),
        dir: store.dir().display().to_string(),
        runtime: harness_local::runtime::status(),
        disk_total,
        disk_free,
    })
}

/// The descriptive half of a [`CatalogModel`] — who the model is, before its
/// quants are annotated for this machine.
struct CatalogIdentity {
    id: String,
    display: String,
    params: String,
    context: u32,
    note: String,
    source: &'static str,
}

/// Annotate a list of installable refs (largest-first) into a [`CatalogModel`]:
/// fit + installed state per quant, plus the auto-picked recommended quant.
fn annotate_catalog_model(
    identity: CatalogIdentity,
    refs: Vec<ModelRef>,
    profile: &harness_local::HardwareProfile,
    store: &ModelStore,
) -> CatalogModel {
    let candidates: Vec<fit::QuantCandidate> = refs
        .iter()
        .map(|r| fit::QuantCandidate {
            quant: r.quant.clone(),
            weight_bytes: r.size_bytes,
        })
        .collect();
    let recommended_quant =
        fit::pick_quant(&candidates, fit::PLANNED_CONTEXT, profile.usable_budget)
            .map(|c| c.quant.clone());

    let quants: Vec<QuantOption> = refs
        .into_iter()
        .map(|r| QuantOption {
            quant: r.quant.clone(),
            size_bytes: r.size_bytes,
            fit: fit::fit_on(profile, r.size_bytes),
            installed: store.is_installed(&r.id),
            model: r,
        })
        .collect();
    // Best fit across quants (smallest quant usually fits best).
    let best_fit = quants
        .iter()
        .map(|q| q.fit)
        .min_by_key(|f| match f {
            harness_local::Fit::Good => 0,
            harness_local::Fit::Tight => 1,
            harness_local::Fit::TooBig => 2,
        })
        .unwrap_or(harness_local::Fit::TooBig);

    CatalogModel {
        id: identity.id,
        display: identity.display,
        params: identity.params,
        context: identity.context,
        note: identity.note,
        source: identity.source.to_string(),
        quants,
        recommended_quant,
        best_fit,
    }
}

/// The model catalog for the setup wizard: the curated family (hardware-fit and
/// quant annotated) plus any featured Oxen.ai-hosted models. Hugging Face models
/// come in via `resolve_hf_model` / `search_hf_models` instead.
#[tauri::command]
async fn list_model_catalog() -> Result<Vec<CatalogModel>, String> {
    let profile = harness_local::detect_hardware();
    let store = ModelStore::open().map_err(|e| e.to_string())?;

    let mut out: Vec<CatalogModel> = harness_local::catalog()
        .iter()
        .map(|spec| {
            annotate_catalog_model(
                CatalogIdentity {
                    id: spec.id.to_string(),
                    display: spec.display.to_string(),
                    params: spec.params.to_string(),
                    context: spec.context,
                    note: spec.note.to_string(),
                    source: "curated",
                },
                harness_local::quant_refs(spec),
                &profile,
                &store,
            )
        })
        .collect();

    // Featured Oxen.ai-hosted models (a stub today), grouped by repo.
    for model in harness_local::source::oxen_featured() {
        out.push(annotate_catalog_model(
            CatalogIdentity {
                id: model.id.clone(),
                display: model.display.clone(),
                params: model.params.clone(),
                context: model.context,
                note: String::new(),
                source: "oxen",
            },
            vec![model],
            &profile,
            &store,
        ));
    }
    Ok(out)
}

/// Resolve a pasted Hugging Face reference (repo or direct GGUF link) into a
/// [`CatalogModel`] with its quants annotated for this machine.
#[tauri::command]
async fn resolve_hf_model(input: String) -> Result<CatalogModel, String> {
    let (repo, file, revision) = harness_local::source::parse_hf_input(&input)
        .ok_or_else(|| "enter a Hugging Face repo like `owner/name` or a GGUF link".to_string())?;
    let token = hf_token();

    let refs = match file {
        // A direct link to one GGUF: resolve just that file.
        Some(f) => {
            let quant = harness_local::source::parse_quant(&f).unwrap_or_default();
            vec![ModelRef {
                id: harness_local::source::id_from_file(&f),
                display: format!(
                    "{}{}",
                    repo.rsplit('/').next().unwrap_or(&repo),
                    if quant.is_empty() {
                        String::new()
                    } else {
                        format!(" · {quant}")
                    }
                ),
                params: harness_local::source::parse_params(&repo),
                quant,
                context: 0,
                size_bytes: 0,
                origin: harness_local::Origin::HuggingFace {
                    repo: repo.clone(),
                    file: f,
                    revision,
                },
            }]
        }
        None => harness_local::source::hf_list_quants(&repo, &revision, token.as_deref())
            .await
            .map_err(|e| e.to_string())?,
    };

    let profile = harness_local::detect_hardware();
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    let display = repo.clone();
    let params = harness_local::source::parse_params(&repo);
    Ok(annotate_catalog_model(
        CatalogIdentity {
            id: repo,
            display,
            params,
            context: 0,
            note: String::new(),
            source: "huggingface",
        },
        refs,
        &profile,
        &store,
    ))
}

/// Search the Hugging Face hub for GGUF repos.
#[tauri::command]
async fn search_hf_models(query: String) -> Result<Vec<harness_local::HfHit>, String> {
    harness_local::source::hf_search(&query, hf_token().as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// The Hugging Face token secret name (stored in `~/.oxen-harness/.env`).
const HF_TOKEN_ENV: &str = "HF_TOKEN";

/// The saved Hugging Face token, if any.
fn hf_token() -> Option<String> {
    harness_config::secrets::get(HF_TOKEN_ENV).filter(|t| !t.trim().is_empty())
}

/// Whether a Hugging Face token is currently saved.
#[tauri::command]
async fn hf_token_present() -> bool {
    hf_token().is_some()
}

/// Save (or clear, with an empty string) the Hugging Face token for gated repos.
#[tauri::command]
async fn set_hf_token(token: String) -> Result<(), String> {
    harness_config::secrets::set(HF_TOKEN_ENV, token.trim()).map_err(|e| e.to_string())
}

/// The bearer token to use for a model's origin (HF token / Oxen API key).
fn token_for(model: &ModelRef) -> Option<String> {
    match &model.origin {
        harness_local::Origin::HuggingFace { .. } => hf_token(),
        harness_local::Origin::Oxen { .. } => {
            harness_config::secrets::get("OXEN_API_KEY").filter(|t| !t.trim().is_empty())
        }
    }
}

/// Install `llama-server` for the user, streaming progress via `llama://install`.
#[tauri::command]
async fn install_llama(app: AppHandle) -> Result<(), String> {
    install_llama_server(|line| {
        let _ = app.emit("llama://install", line.to_string());
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The machine's compute profile (RAM, accelerator), so the setup flow can
/// recommend models that fit and auto-pick a quantization.
#[tauri::command]
async fn detect_hardware() -> harness_local::HardwareProfile {
    harness_local::detect_hardware()
}

/// Status of the self-managed llama.cpp runtime (downloaded by us vs found on the
/// system vs absent), for the local-model setup screen.
#[tauri::command]
async fn runtime_status() -> harness_local::RuntimeStatus {
    harness_local::runtime::status()
}

/// Download + set up the self-managed `llama-server` for this platform, streaming
/// progress (log lines + bytes) via `runtime://install`. No Homebrew required.
#[tauri::command]
async fn install_runtime(app: AppHandle) -> Result<(), String> {
    harness_local::runtime::install(|event| {
        let _ = app.emit("runtime://install", event);
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Download a model's weights (from any source), emitting `models://progress` as
/// it streams. The `model` is a concrete [`ModelRef`] the UI chose (a specific
/// quant); the token for its origin is resolved server-side.
#[tauri::command]
async fn download_model(app: AppHandle, model: ModelRef) -> Result<(), String> {
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    let token = token_for(&model);
    let id = model.id.clone();
    store
        .download(&model, token.as_deref(), |p| {
            let _ = app.emit(
                "models://progress",
                DownloadEvent {
                    id: id.clone(),
                    downloaded: p.downloaded,
                    total: p.total,
                    fraction: p.fraction(),
                },
            );
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a downloaded model by its id.
#[tauri::command]
async fn remove_model(id: String) -> Result<(), String> {
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    store.remove(&id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Switch the session to a downloaded local model: start `llama-server` (with a
/// context window sized to this machine) and rebuild the agent against it. The
/// model must already be downloaded.
#[tauri::command]
async fn use_local_model(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionInfo, String> {
    if llama_server_path().is_none() {
        return Err(format!(
            "the local runtime isn't installed. {}",
            install_hint()
        ));
    }
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    if !store.is_installed(&id) {
        return Err(format!("{id} isn't downloaded yet"));
    }

    // Size the served context to the machine: weights + KV cache must fit budget.
    let profile = harness_local::detect_hardware();
    let weight_bytes = store.installed_size(&id).unwrap_or(0);
    let native = store.native_context(&id);
    let context = fit::plan_context(profile.usable_budget, weight_bytes, native);

    // Stream load phases to the UI so the switch shows what it's doing (runtime
    // init vs. loading the weights) instead of an opaque "Switching…".
    let server = LocalServer::start_with_context(
        &store.path_for(&id),
        &id,
        context,
        local_status_emitter(&app, &id),
    )
    .await
    .map_err(|e| e.to_string())?;
    let context_window = Some(server.context_size() as usize);
    let root = active_root(&state).await;
    let agent = new_agent(
        &app,
        state.pending.clone(),
        OxenClient::new(server.base_url(), "local", &id),
        &id,
        context_window,
        &root,
    )?;

    // Remember the local server + model so new sessions reuse it, persist the
    // choice so it survives a restart, then install the agent as the current chat.
    *state.local_server.lock().await = Some(server);
    *state.local_model.lock().await = Some(id.clone());
    let _ = harness_runtime::models::set_active_local(&id);
    Ok(install_agent(&state, agent).await)
}

// ===========================================================================
// Cloud models — a small catalog of built-in models plus any the user adds,
// and the selected default. Switching swaps the live conversation in place
// (continuing the chat), unlike a local model, which needs a fresh server.
// ===========================================================================

/// The cloud model catalog (built-ins + custom), with the selected one flagged.
#[tauri::command]
async fn list_cloud_models() -> Result<Vec<harness_runtime::models::CloudModel>, String> {
    Ok(harness_runtime::models::catalog())
}

/// Add (or rename) a custom cloud model; returns the updated catalog.
#[tauri::command]
async fn add_cloud_model(
    id: String,
    name: String,
) -> Result<Vec<harness_runtime::models::CloudModel>, String> {
    harness_runtime::models::add(&id, &name).map_err(|e| e.to_string())
}

/// Remove a custom cloud model (built-ins can't be removed); returns the catalog.
#[tauri::command]
async fn remove_cloud_model(
    id: String,
) -> Result<Vec<harness_runtime::models::CloudModel>, String> {
    harness_runtime::models::remove(&id).map_err(|e| e.to_string())
}

/// Switch the current chat to a cloud `model`, continuing the same conversation:
/// the transcript stays, only the model (and, if coming from a local model, the
/// client) is swapped. Also makes it the default for new chats and persists the
/// choice so it survives a restart.
#[tauri::command]
async fn set_model(
    app: AppHandle,
    state: State<'_, AppState>,
    model: String,
) -> Result<SessionInfo, String> {
    let model = model.trim().to_string();
    if model.is_empty() {
        return Err("model id cannot be empty".into());
    }
    harness_runtime::models::set_selected(&model).map_err(|e| e.to_string())?;
    *state.cloud_model.lock().await = model.clone();
    // We're going cloud — drop any active local model/server.
    *state.local_server.lock().await = None;
    *state.local_model.lock().await = None;

    // Swap the live conversation onto the cloud client + model in place. (If the
    // chat was on a local model, replacing the client moves it to the cloud
    // endpoint; the small local context window is cleared so it re-derives.)
    let arc = current_agent(&app, &state).await?;
    let client = build_client(&model)?;
    let mut agent = arc.lock().await;
    agent.set_client(client);
    agent.set_model(&model);
    agent.set_context_window(None);
    Ok(info_for(&agent))
}

// ===========================================================================
// Projects — list, open a folder as a project, switch the active one.
// ===========================================================================

/// List known projects — the persisted set unioned with every directory that
/// already has chats — with chat counts and the active one flagged.
#[tauri::command]
async fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectView>, String> {
    let active = active_root(&state).await.display().to_string();

    // Chats per workspace, so each directory with history shows up as a project.
    let mut counts: HashMap<String, usize> = HashMap::new();
    if let Ok(store) = open_history_store() {
        if let Ok(sessions) = store.list_sessions() {
            for s in sessions {
                *counts.entry(s.workspace).or_default() += 1;
            }
        }
    }

    let mut paths = read_projects_config().paths;
    for k in counts.keys() {
        if !paths.contains(k) {
            paths.push(k.clone());
        }
    }
    if !paths.contains(&active) {
        paths.push(active.clone());
    }

    let mut projects: Vec<ProjectView> = paths
        .into_iter()
        .map(|p| ProjectView {
            name: project_name(&p),
            session_count: counts.get(&p).copied().unwrap_or(0),
            active: p == active,
            path: p,
        })
        .collect();
    // Active first, then busiest, then alphabetical.
    projects.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.session_count.cmp(&a.session_count))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(projects)
}

/// Add a directory as a project and make it active. New chats root here.
#[tauri::command]
async fn open_project(state: State<'_, AppState>, path: String) -> Result<ProjectView, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    let canonical = dir
        .canonicalize()
        .map(|c| c.display().to_string())
        .unwrap_or(path);
    remember_project(&canonical)?;
    *state.active_project.lock().await = PathBuf::from(&canonical);
    Ok(ProjectView {
        name: project_name(&canonical),
        session_count: 0,
        active: true,
        path: canonical,
    })
}

/// Switch the active project to an already-known directory.
#[tauri::command]
async fn set_active_project(state: State<'_, AppState>, path: String) -> Result<(), String> {
    *state.active_project.lock().await = PathBuf::from(&path);
    remember_project(&path)
}

// ===========================================================================
// Themes — list, switch, import/export, and vibe-code a new one via the model.
// ===========================================================================

fn theme_store() -> Result<harness_theme::Store, String> {
    harness_theme::Store::open().map_err(|e| e.to_string())
}

/// A one-shot, agent-free model completion using the active model + endpoint.
/// Used for side tasks (theme generation) so they never block — or wait on — a
/// chat's agent, which may be mid-turn.
async fn complete_oneshot(
    app: &AppHandle,
    state: &AppState,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let (client, model, _) = client_for(app, state).await?;
    let request = ChatRequest::new(
        &model,
        vec![
            ChatMessage::system(system.to_string()),
            ChatMessage::user(user.to_string()),
        ],
    )
    .streaming(true);
    let assembled = client
        .stream_chat(&request, &CancellationToken::new(), |_| {})
        .await
        .map_err(|e| e.to_string())?;
    Ok(assembled.content)
}

/// All available themes (built-in + installed), with the active one marked.
#[tauri::command]
async fn list_themes() -> Result<Vec<harness_theme::store::ThemeSummary>, String> {
    Ok(theme_store()?.list())
}

/// The full active theme (palette + voice) for the UI to apply.
#[tauri::command]
async fn active_theme() -> Result<harness_theme::Theme, String> {
    Ok(theme_store()?.load_active())
}

/// Switch the active theme; returns the resolved theme so the UI can re-skin.
#[tauri::command]
async fn use_theme(name: String) -> Result<harness_theme::Theme, String> {
    theme_store()?.set_active(&name).map_err(|e| e.to_string())
}

/// Install a theme from pasted/loaded TOML or JSON, then activate it.
#[tauri::command]
async fn import_theme(contents: String) -> Result<harness_theme::Theme, String> {
    let store = theme_store()?;
    let theme = store
        .install_from_str(&contents)
        .map_err(|e| e.to_string())?;
    store
        .set_active(&theme.meta.name)
        .map_err(|e| e.to_string())
}

/// Export a theme as a shareable TOML document.
#[tauri::command]
async fn export_theme(name: String) -> Result<String, String> {
    let theme = theme_store()?.resolve(&name).map_err(|e| e.to_string())?;
    theme.to_toml().map_err(|e| e.to_string())
}

/// Remove an installed theme (built-ins always remain).
#[tauri::command]
async fn remove_theme(name: String) -> Result<(), String> {
    theme_store()?.remove(&name).map_err(|e| e.to_string())
}

/// Vibe-code a new theme: send the brief to the model, parse its output, save
/// and activate it. Reuses the session's model + endpoint.
#[tauri::command]
async fn new_theme(
    app: AppHandle,
    state: State<'_, AppState>,
    brief: String,
) -> Result<harness_theme::Theme, String> {
    let raw = complete_oneshot(
        &app,
        &state,
        &harness_theme::Theme::generation_system_prompt(),
        &brief,
    )
    .await?;
    let theme = harness_theme::Theme::from_model_output(&raw).map_err(|e| e.to_string())?;
    let store = theme_store()?;
    store.save(&theme).map_err(|e| e.to_string())?;
    store
        .set_active(&theme.meta.name)
        .map_err(|e| e.to_string())
}

/// Deliver the user's answer to a pending `ask_user_question`, unblocking the
/// agent. Unknown ids are ignored (the question may have been cancelled).
#[tauri::command]
async fn answer_question(
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

/// Entry point shared by the binary and mobile targets.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Load ~/.oxen-harness/.env so saved API keys reach the environment before
    // any agent or tool reads them, then migrate any legacy plaintext keys out
    // of connection.json into the .env.
    harness_config::secrets::load();
    let _ = harness_runtime::connection::load();
    // Start in the last active project (or the launch directory on first run).
    let initial_project = read_projects_config()
        .active
        .map(PathBuf::from)
        .unwrap_or_else(launch_dir);
    // Start on the model the user last chose: the selected cloud model, plus any
    // persisted local model (its server is started lazily on first use). Both are
    // restored so the dropdown choice survives a restart.
    let initial_model = harness_runtime::models::selected();
    let initial_local = harness_runtime::models::active_local();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            active_project: Mutex::new(initial_project),
            cloud_model: Mutex::new(initial_model),
            local_model: Mutex::new(initial_local),
            ..AppState::default()
        })
        .invoke_handler(tauri::generate_handler![
            run_turn,
            cancel_turn,
            run_code_review,
            get_code_review_config,
            save_code_review_config,
            default_code_review_config,
            session_info,
            list_sessions,
            session_messages,
            set_review_status,
            set_review_status_many,
            delete_session,
            attachment_data_uri,
            tool_definitions,
            list_tools,
            add_custom_tool,
            remove_custom_tool,
            set_tool_enabled,
            set_tool_description,
            get_compression_mode,
            set_compression_mode,
            total_tokens_saved,
            list_skills,
            save_skill,
            delete_skill,
            set_skill_enabled,
            export_finetuning,
            total_tokens_used,
            new_session,
            resume_session,
            list_projects,
            open_project,
            set_active_project,
            get_connection,
            set_connection,
            configure_brave_key,
            configure_oxen_key,
            retry_turn,
            installed_local_models,
            install_llama,
            detect_hardware,
            runtime_status,
            install_runtime,
            list_model_catalog,
            resolve_hf_model,
            search_hf_models,
            hf_token_present,
            set_hf_token,
            download_model,
            remove_model,
            use_local_model,
            list_cloud_models,
            add_cloud_model,
            remove_cloud_model,
            set_model,
            answer_question,
            list_themes,
            active_theme,
            use_theme,
            import_theme,
            export_theme,
            remove_theme,
            new_theme
        ])
        .build(tauri::generate_context!())
        .expect("error while building oxen-harness desktop app")
        .run(|app, event| {
            // The local `llama-server` runs as a separate child process. On a
            // normal quit (Cmd+Q, window close, app menu) drop it so it doesn't
            // linger after the app is gone — dropping the `LocalServer` kills the
            // child (it spawned with `kill_on_drop`). A SIGKILL of the app itself
            // can't be intercepted, so that case can still orphan the server.
            if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(async {
                    state.local_server.lock().await.take();
                });
            }
        });
}
