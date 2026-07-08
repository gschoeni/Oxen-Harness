//! The shared [`AppState`] and the agent lifecycle every command leans on:
//! building, resuming, caching, and evicting the per-session agents.
//!
//! The agents map is a cache, never the source of truth — every message is
//! persisted as it's made, so an evicted chat rehydrates from the database
//! via [`agent_or_build`] and continues exactly where it left off. Commands
//! reach agents only through the helpers here, which keep the locking
//! discipline in one place: hold the map lock briefly to look an agent up;
//! a turn holds only its own session's lock.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use harness_agent::{Agent, AgentConfig};
use harness_llm::OxenClient;
use harness_local::{fit, llama_server_path, LocalServer, ModelStore};
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{AskUserTool, CanvasTool, QuestionAnswer, ToolRegistry, Workspace};
use tauri::{AppHandle, Manager};
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

use crate::bridges::{
    NullAsker, NullCanvasSink, NullFleetSink, TauriAsker, TauriCanvasSink, TauriFleetSink,
};
use crate::commands::session::SessionInfo;
use crate::events::{emit_local_status, local_status_emitter};

/// Outstanding `ask_user_question` prompts awaiting a UI answer, keyed by id.
pub(crate) type Pending = Arc<StdMutex<HashMap<String, oneshot::Sender<Vec<QuestionAnswer>>>>>;

/// Per-session agents, each behind its own lock so turns in different chats run
/// concurrently — a background chat keeps streaming while you start or read
/// another. The map lock is held only briefly to look an agent up; the turn
/// itself holds just that session's lock.
#[derive(Default)]
pub struct AppState {
    pub(crate) agents: Mutex<HashMap<String, Arc<Mutex<Agent>>>>,
    /// The session the UI currently shows. Commands that act on "this" chat
    /// (session_info, new_theme, model/connection switches) use it.
    pub(crate) current: Mutex<Option<String>>,
    /// A local `llama-server` kept alive while a local model is selected.
    pub(crate) local_server: Mutex<Option<LocalServer>>,
    /// The local model id in use, so new sessions reuse it instead of the cloud.
    pub(crate) local_model: Mutex<Option<String>>,
    /// The selected cloud model id, used for new sessions (and live swaps) when
    /// no local model is active. Seeded from the persisted selection at startup.
    pub(crate) cloud_model: Mutex<String>,
    /// The active project's directory — new chats are rooted here (the agent's
    /// workspace), so each project's chats run against its own codebase. Empty
    /// means "the launch directory" (resolved lazily by [`active_root`]).
    pub(crate) active_project: Mutex<PathBuf>,
    /// Questions the agent is currently waiting on the user to answer.
    pub(crate) pending: Pending,
    /// Stop signals for in-flight turns, keyed by session. Held here (not on the
    /// agent) so `cancel_turn` can fire one without taking the agent's lock,
    /// which the running turn holds for its whole duration.
    pub(crate) cancels: Mutex<HashMap<String, CancellationToken>>,
    /// Each session agent's `spawn_agents` spawner, so `execute_turn` can hand
    /// it the turn's stop signal — cancelling the turn then cancels any fleet
    /// the model launched inside it. Std mutex: touched briefly, from sync
    /// builders too. Evicted alongside the agents map.
    pub(crate) fleet_spawners: StdMutex<HashMap<String, Arc<harness_agent::FleetSpawner>>>,
}

/// The client, model label, and context window for a new agent: the selected
/// local model + server if one is active, otherwise the configured cloud client.
pub(crate) async fn client_for(
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
pub(crate) async fn ensure_local_server(
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
pub(crate) fn build_client(model: &str) -> Result<OxenClient, String> {
    harness_runtime::connection::build_client(model).map_err(|e| e.to_string())
}

/// Shared agent dependencies — the tool registry (defaults + the question
/// bridge, *before* user preferences) and the history store. Fresh and resumed
/// agents build these the same way; only how they bind a session differs.
/// [`finish_tools`] completes the registry once the session id is known.
pub(crate) fn agent_parts(
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
pub(crate) fn finish_tools(
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
pub(crate) fn register_fleet(
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
pub(crate) fn new_agent(
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
pub(crate) fn resume_agent(
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

/// The directory the app was launched from — the default/initial project.
pub(crate) fn launch_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The active project's directory, falling back to the launch directory.
pub(crate) async fn active_root(state: &AppState) -> PathBuf {
    let p = state.active_project.lock().await.clone();
    if p.as_os_str().is_empty() {
        launch_dir()
    } else {
        p
    }
}

/// The `spawn_agents` spawner for a session, if one is registered. Callers that
/// swap the live agent's client/model use it to keep future subagents in step
/// (the spawner captured client/model when the agent was built).
pub(crate) fn fleet_spawner_for(
    state: &AppState,
    session: &str,
) -> Option<Arc<harness_agent::FleetSpawner>> {
    state
        .fleet_spawners
        .lock()
        .expect("fleet spawners poisoned")
        .get(session)
        .cloned()
}

/// Build a fresh agent for a new session rooted at `root`, reusing the active
/// local model if any.
pub(crate) async fn build_fresh_agent(
    app: &AppHandle,
    state: &AppState,
    root: &Path,
) -> Result<Agent, String> {
    let (client, label, ctx) = client_for(app, state).await?;
    new_agent(app, state.pending.clone(), client, &label, ctx, root)
}

/// Build an agent bound to an existing session id, rooted at `root` (its own
/// recorded workspace), without leaking a throwaway session row.
pub(crate) async fn build_resumed_agent(
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
pub(crate) async fn agent_for(state: &AppState, id: &str) -> Option<Arc<Mutex<Agent>>> {
    state.agents.lock().await.get(id).cloned()
}

/// A session's recorded working directory (its project), read from the store;
/// falls back to the launch directory when unknown.
pub(crate) fn session_workspace(id: &str) -> PathBuf {
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
pub(crate) async fn agent_or_build(
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

pub(crate) fn info_for(agent: &Agent) -> SessionInfo {
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
pub(crate) async fn evict_idle(state: &AppState) {
    let current = { state.current.lock().await.clone() };
    let kept: std::collections::HashSet<String> = {
        let mut agents = state.agents.lock().await;
        agents.retain(|id, arc| Some(id.as_str()) == current.as_deref() || arc.try_lock().is_err());
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
pub(crate) async fn install_agent(state: &AppState, agent: Agent) -> SessionInfo {
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
pub(crate) async fn current_agent(
    app: &AppHandle,
    state: &AppState,
) -> Result<Arc<Mutex<Agent>>, String> {
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

/// Open the shared on-disk history store (same DB the agents persist to).
pub(crate) fn open_history_store() -> Result<HistoryStore, String> {
    let path = harness_config::paths::history_db().map_err(|e| e.to_string())?;
    HistoryStore::open(path).map_err(|e| e.to_string())
}

/// The complete tool set for settings purposes: the workspace defaults plus the
/// host-bridged ask/canvas tools (wired to inert bridges — the Tools page only
/// reads names, descriptions, and schemas). Matches what [`finish_tools`]
/// registers on a real agent, so every manageable tool appears in Settings and
/// custom-tool names can't shadow any of them.
pub(crate) async fn settings_registry(state: &AppState) -> Result<ToolRegistry, String> {
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
