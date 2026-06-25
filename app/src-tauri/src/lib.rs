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
use harness_core::DEFAULT_MODEL;
use harness_llm::{Attachment, ChatMessage, ChatRequest, OxenClient};
use harness_local::{
    can_auto_install, install_hint, install_llama_server, llama_server_path, LocalServer,
    ModelStatus, ModelStore,
};
use harness_store::{HistoryStore, SessionMeta, SessionSummary};
use harness_tools::{
    AskUserTool, CanvasDoc, CanvasSink, CanvasTool, Question, QuestionAnswer, QuestionAsker,
    ToolError, ToolRegistry, Workspace,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{oneshot, Mutex};

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
    /// The active project's directory — new chats are rooted here (the agent's
    /// workspace), so each project's chats run against its own codebase. Empty
    /// means "the launch directory" (resolved lazily by [`active_root`]).
    active_project: Mutex<PathBuf>,
    /// Questions the agent is currently waiting on the user to answer.
    pending: Pending,
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

#[derive(Clone, Serialize)]
struct ModelsView {
    models: Vec<ModelStatus>,
    total_disk_bytes: u64,
    dir: String,
    /// Whether `llama-server` is available to actually run a local model.
    llama_installed: bool,
    /// Whether the app can install `llama-server` for the user automatically.
    can_auto_install: bool,
    /// How to install `llama-server` when it's missing.
    install_hint: String,
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

#[derive(Clone, Serialize)]
struct SessionInfo {
    model: String,
    workspace: String,
    session_id: String,
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
async fn client_for(state: &AppState) -> Result<(OxenClient, String, Option<usize>), String> {
    let server = state.local_server.lock().await;
    let model = state.local_model.lock().await;
    match (server.as_ref(), model.as_ref()) {
        (Some(s), Some(id)) => Ok((
            OxenClient::new(s.base_url(), "local", id),
            id.clone(),
            Some(s.context_size() as usize),
        )),
        _ => Ok((configured_client()?, DEFAULT_MODEL.to_string(), None)),
    }
}

/// Build an Oxen client honoring the user's saved connection overrides, falling
/// back to the `OXEN_*` env vars / the `oxen` CLI login / the default endpoint
/// for any field left blank.
fn configured_client() -> Result<OxenClient, String> {
    let cfg = read_connection_config();
    let base_url = match cfg.host.trim() {
        "" => harness_llm::resolve_base_url(),
        host => harness_llm::base_url_from_host(host),
    };
    match cfg.api_key.trim() {
        "" => OxenClient::connect(base_url, DEFAULT_MODEL).map_err(|e| e.to_string()),
        key => Ok(OxenClient::new(base_url, key, DEFAULT_MODEL)),
    }
}

/// Shared agent dependencies — the tool registry (with the question bridge), the
/// history store, and the run config. Fresh and resumed agents build these the
/// same way; only how they bind a session differs.
fn agent_parts(
    app: &AppHandle,
    pending: Pending,
    model_label: &str,
    context_window: Option<usize>,
    workspace_root: &Path,
) -> Result<(ToolRegistry, Arc<HistoryStore>, AgentConfig), String> {
    let workspace = Workspace::new(workspace_root).map_err(|e| e.to_string())?;
    let brave_key = brave_key_override(&read_connection_config());
    let mut tools = ToolRegistry::default_for_workspace_with_web_key(workspace, brave_key);
    tools.register(Arc::new(AskUserTool::new(Arc::new(TauriAsker {
        app: app.clone(),
        pending,
    }))));
    // Only advertise web search in the prompt when it actually registered, so
    // the model never calls a tool the registry would reject as unknown.
    let web_search = tools.get("web_search").is_some();

    let dir = dirs_home()?.join(".oxen-harness");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let store =
        Arc::new(HistoryStore::open(dir.join("history.sqlite")).map_err(|e| e.to_string())?);

    let config = AgentConfig {
        model: model_label.to_string(),
        // The host always registers the `canvas` tool (per session, below), so
        // advertise it in the prompt.
        system_prompt: Some(harness_agent::system_prompt_with(web_search, true)),
        context_window,
        ..AgentConfig::default()
    };
    Ok((tools, store, config))
}

/// Register the session-scoped `canvas` tool on a registry. Done once the
/// session id is known so canvas events can be tagged with it.
fn register_canvas(tools: &mut ToolRegistry, app: &AppHandle, session: &str) {
    tools.register(Arc::new(CanvasTool::new(Arc::new(TauriCanvasSink {
        app: app.clone(),
        session: session.to_string(),
    }))));
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
    let (mut tools, store, config) =
        agent_parts(app, pending, model_label, context_window, workspace_root)?;
    let session = store
        .create_session(&SessionMeta {
            workspace: workspace_root.display().to_string(),
            model: model_label.to_string(),
        })
        .map_err(|e| e.to_string())?;
    register_canvas(&mut tools, app, &session);
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
    let (mut tools, store, config) =
        agent_parts(app, pending, model_label, context_window, workspace_root)?;
    register_canvas(&mut tools, app, &session_id);
    Agent::resume_from_store(client, tools, store, session_id, config).map_err(|e| e.to_string())
}

fn dirs_home() -> Result<std::path::PathBuf, String> {
    #[allow(deprecated)]
    std::env::home_dir().ok_or_else(|| "could not determine home directory".to_string())
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

fn projects_config_path() -> Result<PathBuf, String> {
    Ok(dirs_home()?.join(".oxen-harness").join("projects.json"))
}

fn read_projects_config() -> ProjectsConfig {
    projects_config_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_projects_config(cfg: &ProjectsConfig) -> Result<(), String> {
    let path = projects_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
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

/// Persisted Oxen connection overrides (`~/.oxen-harness/connection.json`). An
/// empty `host` or `api_key` means "fall back to `OXEN_*` env vars, the `oxen`
/// CLI login, or the default endpoint".
#[derive(Default, Serialize, Deserialize)]
struct ConnectionConfig {
    #[serde(default)]
    host: String,
    #[serde(default)]
    api_key: String,
    /// Brave Search API key enabling the `web_search` tool. Blank = fall back to
    /// the `BRAVE_API_KEY` environment variable (web search off if neither set).
    #[serde(default)]
    brave_api_key: String,
}

/// What the Settings page shows: the saved overrides plus context so the UI can
/// render helpful placeholders and tell the user whether they're already
/// authenticated without typing a key.
#[derive(Serialize)]
struct ConnectionView {
    host: String,
    api_key: String,
    /// Effective Brave Search API key in use (override, else `BRAVE_API_KEY`).
    brave_api_key: String,
    /// The default Oxen host, shown as the host field's placeholder.
    default_host: String,
    /// Whether a key already resolves from the environment / `oxen` CLI login
    /// for the effective host, so a blank key field still works.
    env_key_available: bool,
}

fn connection_config_path() -> Result<std::path::PathBuf, String> {
    Ok(dirs_home()?.join(".oxen-harness").join("connection.json"))
}

/// Read the saved overrides, treating a missing or unparseable file as "none".
fn read_connection_config() -> ConnectionConfig {
    connection_config_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_connection_config(cfg: &ConnectionConfig) -> Result<(), String> {
    let path = connection_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// Resolve the effective base URL from the saved host override (or env/default).
fn effective_base_url(cfg: &ConnectionConfig) -> String {
    match cfg.host.trim() {
        "" => harness_llm::resolve_base_url(),
        host => harness_llm::base_url_from_host(host),
    }
}

/// Resolve the API key actually in use: the saved override if set, otherwise
/// whatever resolves from `OXEN_API_KEY` / the `oxen` CLI login for `base_url`'s
/// host (empty if nothing resolves).
fn effective_api_key(cfg: &ConnectionConfig, base_url: &str) -> String {
    match cfg.api_key.trim() {
        "" => harness_llm::auth::resolve_api_key_for_base_url(base_url).unwrap_or_default(),
        key => key.to_string(),
    }
}

/// Return the connection settings for the Settings page, pre-filled with the
/// values actually in use — so the fields reflect the resolved host and key
/// (from env / the `oxen` CLI login / the default endpoint), not just whatever
/// override happens to be saved.
/// The Brave Search API key actually in use: the saved override, else whatever
/// `BRAVE_API_KEY` provides (empty if neither is set, i.e. web search is off).
fn effective_brave_key(cfg: &ConnectionConfig) -> String {
    match cfg.brave_api_key.trim() {
        "" => harness_tools::web::brave_api_key().unwrap_or_default(),
        key => key.to_string(),
    }
}

/// Build the Brave key option passed to the tool registry: the saved override
/// if set, else `None` (which lets the tool fall back to `BRAVE_API_KEY`).
fn brave_key_override(cfg: &ConnectionConfig) -> Option<String> {
    match cfg.brave_api_key.trim() {
        "" => None,
        key => Some(key.to_string()),
    }
}

#[tauri::command]
fn get_connection() -> ConnectionView {
    let cfg = read_connection_config();
    let base_url = effective_base_url(&cfg);
    let api_key = effective_api_key(&cfg, &base_url);
    ConnectionView {
        host: harness_llm::host_from_base_url(&base_url),
        env_key_available: !api_key.is_empty(),
        api_key,
        brave_api_key: effective_brave_key(&cfg),
        default_host: harness_llm::auth::DEFAULT_OXEN_HOST.to_string(),
    }
}

/// Save just the Brave Search API key and apply it to the running agent.
///
/// Unlike [`set_connection`], this does **not** rebuild the agent or start a new
/// session — it persists the key and sets `BRAVE_API_KEY` in the process so the
/// already-registered `web_search` tool picks it up on its next call. Lets the
/// user fix a failed web search inline and immediately retry in the same chat.
#[tauri::command]
fn configure_brave_key(key: String) -> Result<(), String> {
    let key = key.trim().to_string();
    let mut cfg = read_connection_config();
    cfg.brave_api_key = key.clone();
    write_connection_config(&cfg)?;
    if !key.is_empty() {
        std::env::set_var(harness_tools::web::BRAVE_API_KEY_ENV, &key);
    }
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
    write_connection_config(&ConnectionConfig {
        host: host.trim().to_string(),
        api_key: api_key.trim().to_string(),
        brave_api_key: brave_api_key.trim().to_string(),
    })?;

    let root = active_root(&state).await;
    let agent = new_agent(
        &app,
        state.pending.clone(),
        configured_client()?,
        DEFAULT_MODEL,
        None,
        &root,
    )?;
    *state.local_server.lock().await = None;
    *state.local_model.lock().await = None;
    Ok(install_agent(&state, agent).await)
}

/// Build a fresh agent for a new session rooted at `root`, reusing the active
/// local model if any.
async fn build_fresh_agent(app: &AppHandle, state: &AppState, root: &Path) -> Result<Agent, String> {
    let (client, label, ctx) = client_for(state).await?;
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
    let (client, label, ctx) = client_for(state).await?;
    resume_agent(app, state.pending.clone(), client, &label, ctx, session_id, root)
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
    }
}

/// Release cached agents we don't need in memory: everything except the current
/// chat and any whose turn is still running (whose per-session lock is held).
/// The dropped chats live on in SQLite and rehydrate via [`agent_or_build`], so
/// resident memory tracks concurrency (running turns + the open chat), never the
/// number of chats in history.
async fn evict_idle(state: &AppState) {
    let current = { state.current.lock().await.clone() };
    state
        .agents
        .lock()
        .await
        .retain(|id, arc| Some(id.as_str()) == current.as_deref() || arc.try_lock().is_err());
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

    // Get the live agent or rehydrate it from the database. The agents map is a
    // cache, not the source of truth, so an evicted chat simply rebuilds here.
    let arc = agent_or_build(&app, &state, &session).await?;

    let sid = session.clone();
    let result = {
        let mut agent = arc.lock().await;
        agent
            .run_turn_with_attachments(prompt, attachments, move |event| match event {
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
                    let _ = app.emit("agent://canvas-writing", SessionPayload { session: sid.clone() });
                }
                AgentEvent::ToolPending { .. } => {}
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
            })
            .await
    };
    // The turn is persisted message-by-message, so once it's done the agent is
    // just a cache. Release idle background agents (keeping the current chat and
    // any still-running turns) so memory tracks concurrency, not chat count.
    evict_idle(&state).await;
    result.map_err(|e| e.to_string())
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
    let path = dirs_home()?.join(".oxen-harness").join("history.sqlite");
    HistoryStore::open(path).map_err(|e| e.to_string())
}

/// List past chat sessions (those with at least one user message), newest first.
#[tauri::command]
async fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    open_history_store()?.list_sessions().map_err(|e| e.to_string())
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
            },
            messages: vec![],
            running: true,
        },
    };
    Ok(view)
}

/// List local models with their on-disk status and total disk usage.
#[tauri::command]
async fn list_models() -> Result<ModelsView, String> {
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    Ok(ModelsView {
        models: store.statuses(),
        total_disk_bytes: store.total_disk_used(),
        dir: store.dir().display().to_string(),
        llama_installed: llama_server_path().is_some(),
        can_auto_install: can_auto_install(),
        install_hint: install_hint(),
    })
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

/// Download a model's weights, emitting `models://progress` as it streams.
#[tauri::command]
async fn pull_model(app: AppHandle, id: String) -> Result<(), String> {
    let spec = harness_local::find(&id).ok_or_else(|| format!("unknown model `{id}`"))?;
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    store
        .pull(spec, |p| {
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

/// Delete a downloaded model.
#[tauri::command]
async fn remove_model(id: String) -> Result<(), String> {
    let spec = harness_local::find(&id).ok_or_else(|| format!("unknown model `{id}`"))?;
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    store.remove(spec).map_err(|e| e.to_string())?;
    Ok(())
}

/// Switch the session to a downloaded local model: start `llama-server` and
/// rebuild the agent against it. The model must already be downloaded.
#[tauri::command]
async fn use_local_model(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionInfo, String> {
    let spec = harness_local::find(&id).ok_or_else(|| format!("unknown model `{id}`"))?;
    if llama_server_path().is_none() {
        return Err(format!("llama-server isn't installed. {}", install_hint()));
    }
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    if !store.is_installed(spec) {
        return Err(format!("{id} isn't downloaded yet — pull it first"));
    }

    let server = LocalServer::start(&store.path_for(spec), &id)
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

    // Remember the local server + model so new sessions reuse it, then install
    // the agent as the current chat.
    *state.local_server.lock().await = Some(server);
    *state.local_model.lock().await = Some(id.clone());
    Ok(install_agent(&state, agent).await)
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
async fn complete_oneshot(state: &AppState, system: &str, user: &str) -> Result<String, String> {
    let (client, model, _) = client_for(state).await?;
    let request = ChatRequest::new(
        &model,
        vec![
            ChatMessage::system(system.to_string()),
            ChatMessage::user(user.to_string()),
        ],
    )
    .streaming(true);
    let assembled = client
        .stream_chat(&request, |_| {})
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
    state: State<'_, AppState>,
    brief: String,
) -> Result<harness_theme::Theme, String> {
    let raw =
        complete_oneshot(&state, &harness_theme::Theme::generation_system_prompt(), &brief).await?;
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
    // Start in the last active project (or the launch directory on first run).
    let initial_project = read_projects_config()
        .active
        .map(PathBuf::from)
        .unwrap_or_else(launch_dir);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            active_project: Mutex::new(initial_project),
            ..AppState::default()
        })
        .invoke_handler(tauri::generate_handler![
            run_turn,
            session_info,
            list_sessions,
            new_session,
            resume_session,
            list_projects,
            open_project,
            set_active_project,
            get_connection,
            set_connection,
            configure_brave_key,
            list_models,
            install_llama,
            pull_model,
            remove_model,
            use_local_model,
            answer_question,
            list_themes,
            active_theme,
            use_theme,
            import_theme,
            export_theme,
            remove_theme,
            new_theme
        ])
        .run(tauri::generate_context!())
        .expect("error while running oxen-harness desktop app");
}
