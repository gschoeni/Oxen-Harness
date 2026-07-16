//! [`SessionService`] — the multi-session orchestration every host shares:
//! building, resuming, caching, and evicting per-session agents; driving
//! turns onto the protocol event stream; and delivering client answers back
//! to parked round-trips.
//!
//! The agents map is a cache, never the source of truth — every message is
//! persisted as it's made, so an evicted chat rehydrates from the store via
//! [`SessionService::agent_or_build`] and continues exactly where it left
//! off. Hosts reach agents only through the methods here, which keep the
//! locking discipline in one place: hold the map lock briefly to look an
//! agent up; a turn holds only its own session's lock.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use harness_agent::{Agent, AgentConfig, AgentEvent};
use harness_llm::{Attachment, OxenClient};
use harness_local::{fit, llama_server_path, LocalServer, ModelStore};
use harness_protocol::{ProtocolEvent, SessionInfo, SessionView};
use harness_store::{HistoryStore, SessionMeta, SessionSummary};
use harness_tools::{AskUserTool, CanvasTool, ToolRegistry, Workspace};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::bridges::{
    HostApprover, HostAsker, HostCanvasSink, HostFleetSink, HostViewerSink, NoScreenshotLens,
    NullAsker, NullCanvasSink, NullFleetSink, NullPreviewLens, NullPreviewSink, NullViewerSink,
    ProtocolPreviewSink,
};
use crate::{translate, EventSink, PendingApprovals, PendingQuestions};

/// The `app_meta` key holding the all-time tokens saved by context compression.
const TOTAL_TOKENS_SAVED_KEY: &str = "total_tokens_saved";

const TOKEN_BATCH_BYTES: usize = 512;

/// Builds a client for a model id. Defaults to the shared runtime resolution
/// (saved connection settings / env / CLI login); tests and special hosts
/// inject their own.
pub type ClientFactory = Box<dyn Fn(&str) -> Result<OxenClient, String> + Send + Sync>;

/// Builds a per-session host surface (a preview sink or lens).
pub type SurfaceFactory<T> = Box<dyn Fn(&str) -> Arc<T> + Send + Sync>;
/// Notifies the host about a session-scoped moment (e.g. deletion).
pub type SessionNotify = Box<dyn Fn(&str) + Send + Sync>;

/// Host-specific extension points beyond the [`EventSink`]. Every hook has a
/// protocol-stream default, so a headless host configures nothing; the
/// desktop overrides the preview pair with its native webview surfaces.
#[derive(Default)]
pub struct HostHooks {
    /// Per-session dev-server lifecycle sink. Default: statuses become
    /// `preview.status` protocol events.
    pub preview_sink: Option<SurfaceFactory<dyn harness_preview::PreviewSink>>,
    /// Per-session preview lens (`preview_screenshot` / `preview_console`).
    /// Default: no screenshots, empty console.
    pub preview_lens: Option<SurfaceFactory<dyn harness_preview::PreviewLens>>,
    /// Called after a session is deleted (the desktop closes its preview
    /// webview). Default: nothing.
    pub on_session_deleted: Option<SessionNotify>,
}

/// Batches streamed tokens so the transport isn't flooded with one event per
/// SSE delta: flushed at [`TOKEN_BATCH_BYTES`], before any non-token event
/// (ordering stays exact), and at turn end.
#[derive(Clone)]
struct TokenBatch {
    sink: Arc<dyn EventSink>,
    session: String,
    buffer: Arc<StdMutex<String>>,
}

impl TokenBatch {
    fn new(sink: Arc<dyn EventSink>, session: String) -> Self {
        Self {
            sink,
            session,
            buffer: Arc::new(StdMutex::new(String::with_capacity(TOKEN_BATCH_BYTES))),
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
        self.sink.emit(ProtocolEvent::Token {
            session: self.session.clone(),
            token,
        });
    }
}

/// Whether a turn starts fresh (pushing a new user message) or retries the
/// existing transcript's trailing user turn (e.g. after authenticating past a
/// 401). Both drive the identical streaming/accounting scaffold in
/// [`SessionService::execute_turn`].
enum TurnKind {
    Fresh {
        prompt: String,
        attachments: Vec<Attachment>,
    },
    Retry,
}

/// Configures and builds a [`SessionService`].
pub struct SessionServiceBuilder {
    sink: Arc<dyn EventSink>,
    cloud_model: Option<String>,
    local_model: Option<String>,
    active_project: Option<PathBuf>,
    store: Option<Arc<HistoryStore>>,
    client_factory: Option<ClientFactory>,
    hooks: HostHooks,
}

impl SessionServiceBuilder {
    /// The cloud model new sessions start on. Default: the persisted selection.
    pub fn cloud_model(mut self, model: impl Into<String>) -> Self {
        self.cloud_model = Some(model.into());
        self
    }

    /// A persisted local-model selection to restore (its server starts
    /// lazily on first use). Default: none.
    pub fn local_model(mut self, model: Option<String>) -> Self {
        self.local_model = model;
        self
    }

    /// The active project directory new chats are rooted in. Default: the
    /// launch directory (resolved lazily).
    pub fn active_project(mut self, root: impl Into<PathBuf>) -> Self {
        self.active_project = Some(root.into());
        self
    }

    /// The history store. Default: the shared on-disk database at
    /// `harness_config::paths::history_db()`.
    pub fn store(mut self, store: Arc<HistoryStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// How clients are built for a model id. Default: the shared runtime
    /// resolution (saved connection settings / env / CLI login).
    pub fn client_factory(
        mut self,
        factory: impl Fn(&str) -> Result<OxenClient, String> + Send + Sync + 'static,
    ) -> Self {
        self.client_factory = Some(Box::new(factory));
        self
    }

    /// Host-specific extension points (native preview surfaces, …).
    pub fn hooks(mut self, hooks: HostHooks) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn build(self) -> SessionService {
        SessionService {
            sink: self.sink,
            store: self.store.map(Ok).unwrap_or_else(|| {
                open_history_store().map(Arc::new)
            }),
            cloud_model: Mutex::new(
                self.cloud_model
                    .unwrap_or_else(harness_runtime::models::selected),
            ),
            active_project: Mutex::new(self.active_project.unwrap_or_default()),
            client_factory: self.client_factory.unwrap_or_else(|| {
                Box::new(|model| {
                    harness_runtime::connection::build_client(model).map_err(|e| e.to_string())
                })
            }),
            hooks: self.hooks,
            agents: Mutex::new(HashMap::new()),
            current: Mutex::new(None),
            local_server: Mutex::new(None),
            local_model: Mutex::new(self.local_model),
            pending_questions: PendingQuestions::default(),
            pending_approvals: PendingApprovals::default(),
            cancels: Mutex::new(HashMap::new()),
            interjections: Mutex::new(HashMap::new()),
            fleet_spawners: StdMutex::new(HashMap::new()),
            dev_servers: harness_preview::DevServerManager::new(),
            crash_announced: StdMutex::new(HashMap::new()),
        }
    }
}

/// One loop journal as its wire result.
fn loop_result(journal: &harness_loop::LoopJournal) -> harness_protocol::LoopResult {
    let succeeded = journal.succeeded();
    let iterations = journal.iterations();
    let summary = if succeeded {
        format!("Loop complete: all gates passed after {iterations} iteration(s).")
    } else {
        let reason = journal
            .stop
            .clone()
            .map(|s| s.headline())
            .unwrap_or_else(|| "stopped".into());
        format!("Loop stopped after {iterations} iteration(s): {reason}.")
    };
    harness_protocol::LoopResult {
        succeeded,
        iterations,
        summary,
    }
}

/// Open the shared on-disk history store (the same DB the agents persist to).
fn open_history_store() -> Result<HistoryStore, String> {
    let path = harness_config::paths::history_db().map_err(|e| e.to_string())?;
    HistoryStore::open(path).map_err(|e| e.to_string())
}

/// The directory the process was launched from — the default/initial project.
pub fn launch_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The transport-agnostic host: per-session agents, each behind its own lock
/// so turns in different chats run concurrently. Fields are public — this is
/// a host-integration layer, and front ends (the Tauri app, the HTTP server)
/// legitimately reach the managers and pending maps directly.
pub struct SessionService {
    pub sink: Arc<dyn EventSink>,
    /// The history store every agent persists to. `Err` when the on-disk
    /// database couldn't open (checked at first use so construction is
    /// infallible, mirroring how hosts always open a window/socket first).
    store: Result<Arc<HistoryStore>, String>,
    /// Builds clients for model ids.
    pub client_factory: ClientFactory,
    /// Host-specific extension points.
    pub hooks: HostHooks,
    /// Per-session agents — a cache, never the source of truth.
    pub agents: Mutex<HashMap<String, Arc<Mutex<Agent>>>>,
    /// The session the host currently shows.
    pub current: Mutex<Option<String>>,
    /// A local `llama-server` kept alive while a local model is selected.
    pub local_server: Mutex<Option<LocalServer>>,
    /// The local model id in use, so new sessions reuse it instead of the cloud.
    pub local_model: Mutex<Option<String>>,
    /// The selected cloud model id, used for new sessions (and live swaps)
    /// when no local model is active.
    pub cloud_model: Mutex<String>,
    /// The active project's directory — new chats are rooted here. Empty
    /// means "the launch directory" (resolved lazily by [`Self::active_root`]).
    pub active_project: Mutex<PathBuf>,
    /// Questions the agent is currently waiting on the client to answer.
    pub pending_questions: PendingQuestions,
    /// Permission approvals the agent is currently waiting on the client for.
    pub pending_approvals: PendingApprovals,
    /// Stop signals for in-flight turns, keyed by session. Held here (not on
    /// the agent) so [`Self::cancel_turn`] can fire one without taking the
    /// agent's lock, which the running turn holds for its whole duration.
    pub cancels: Mutex<HashMap<String, CancellationToken>>,
    /// Mid-turn steering channels for in-flight turns, keyed by session —
    /// same lifecycle (and same reason to live here) as `cancels`. See
    /// [`Self::interject`].
    pub interjections: Mutex<HashMap<String, harness_agent::Interjections>>,
    /// Each session agent's `spawn_agents` spawner, so a turn can hand it the
    /// turn's stop signal. Std mutex: touched briefly, from sync builders too.
    pub fleet_spawners: StdMutex<HashMap<String, Arc<harness_agent::FleetSpawner>>>,
    /// Dev servers the agent started for live preview, at most one per
    /// session. NOT evicted with the agents map — a background chat's server
    /// keeps serving until stopped or the host exits.
    pub dev_servers: harness_preview::DevServerManager,
    /// Sessions already told (once) that their dev server died.
    crash_announced: StdMutex<HashMap<String, String>>,
}

impl SessionService {
    pub fn builder(sink: Arc<dyn EventSink>) -> SessionServiceBuilder {
        SessionServiceBuilder {
            sink,
            cloud_model: None,
            local_model: None,
            active_project: None,
            store: None,
            client_factory: None,
            hooks: HostHooks::default(),
        }
    }

    /// The history store, or why it couldn't open.
    pub fn store(&self) -> Result<Arc<HistoryStore>, String> {
        self.store.clone()
    }

    // --- Client & model selection -------------------------------------------

    /// The client, model label, and context window for a new agent: the
    /// selected local model + server if one is active, otherwise the
    /// configured cloud client.
    pub async fn client_for(&self) -> Result<(OxenClient, String, Option<usize>), String> {
        // If a local model is selected (including one restored from a previous
        // run), make sure its server is running and use it.
        let local_id = self.local_model.lock().await.clone();
        if let Some(id) = local_id {
            match self.ensure_local_server(&id).await {
                Ok((base_url, ctx)) => {
                    return Ok((OxenClient::new(base_url, "local", &id), id, Some(ctx)));
                }
                // The runtime or weights aren't available right now — fall back
                // to the cloud model rather than failing to open a chat. The
                // persisted choice is kept, so it retries on the next launch.
                Err(_) => {
                    self.sink.emit(ProtocolEvent::LocalStatus {
                        model: id,
                        phase: harness_protocol::LocalPhase::Error,
                    });
                    *self.local_model.lock().await = None;
                    *self.local_server.lock().await = None;
                }
            }
        }
        let model = self.cloud_model.lock().await.clone();
        Ok(((self.client_factory)(&model)?, model, None))
    }

    /// Ensure a `llama-server` is running for local model `id`, returning its
    /// base URL and context size. Reuses the running server if there is one;
    /// otherwise validates the runtime + weights and starts it (sized to this
    /// machine), streaming `local.status` load phases.
    pub async fn ensure_local_server(&self, id: &str) -> Result<(String, usize), String> {
        let mut guard = self.local_server.lock().await;
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
        let sink = self.sink.clone();
        let model = id.to_string();
        let server = LocalServer::start_with_context(
            &store.path_for(id),
            id,
            context,
            move |phase| {
                sink.emit(ProtocolEvent::LocalStatus {
                    model: model.clone(),
                    phase: translate::local_phase(phase),
                });
            },
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

    /// Switch the current chat to a cloud `model`, continuing the same
    /// conversation: the transcript stays, only the model (and, if coming
    /// from a local model, the client) is swapped. Also makes it the default
    /// for new chats and persists the choice so it survives a restart.
    pub async fn set_model(&self, model: &str) -> Result<SessionInfo, String> {
        let model = model.trim().to_string();
        if model.is_empty() {
            return Err("model id cannot be empty".into());
        }
        harness_runtime::models::set_selected(&model).map_err(|e| e.to_string())?;
        *self.cloud_model.lock().await = model.clone();
        // We're going cloud — drop any active local model/server.
        *self.local_server.lock().await = None;
        *self.local_model.lock().await = None;

        // Swap the live conversation onto the cloud client + model in place.
        let arc = self.current_agent().await?;
        let client = (self.client_factory)(&model)?;
        let mut agent = arc.lock().await;
        agent.set_client(client.clone());
        agent.set_model(&model);
        agent.set_context_window(None);
        // Follow the swap through to the fleet spawner so a later
        // spawn_agents fleet runs on the new model/endpoint.
        let session = agent.session_id().to_string();
        if let Some(spawner) = self.fleet_spawner_for(&session) {
            spawner.set_client(client);
            spawner.set_model(&model);
        }
        Ok(self.info_for(&agent))
    }

    // --- Agent assembly ------------------------------------------------------

    /// Shared agent dependencies — the tool registry (defaults + the question
    /// bridge, *before* user preferences) and the history store.
    /// [`Self::finish_tools`] completes the registry once the session id is
    /// known.
    fn agent_parts(
        &self,
        workspace_root: &Path,
        session: &str,
    ) -> Result<(ToolRegistry, Arc<HistoryStore>), String> {
        let workspace = Workspace::new(workspace_root).map_err(|e| e.to_string())?;
        let brave_key = harness_runtime::connection::brave_key_override();
        let mut tools = ToolRegistry::default_for_workspace_with_web_key(workspace, brave_key);
        tools.register_typed(AskUserTool::new(Arc::new(HostAsker {
            sink: self.sink.clone(),
            session: session.to_string(),
            pending: self.pending_questions.clone(),
        })));
        Ok((tools, self.store()?))
    }

    /// Complete a session's tool registry and derive its run config: the
    /// session-scoped canvas/viewer/preview tools, the user's saved tool
    /// preferences applied to the complete set, the `skill` tool when enabled
    /// skills exist, the permission gate, and a system prompt gated on what
    /// actually survived.
    fn finish_tools(
        &self,
        tools: &mut ToolRegistry,
        session: &str,
        model_label: &str,
        context_window: Option<usize>,
        workspace_root: &Path,
    ) -> AgentConfig {
        tools.register_typed(CanvasTool::new(Arc::new(HostCanvasSink {
            sink: self.sink.clone(),
            session: session.to_string(),
        })));
        // Open project files in the client's editor/viewer surface (the
        // host-surface pattern). The workspace was validated by agent_parts.
        if let Ok(viewer_workspace) = Workspace::new(workspace_root) {
            tools.register_typed(harness_tools::OpenFileTool::new(
                viewer_workspace,
                Arc::new(HostViewerSink {
                    sink: self.sink.clone(),
                    session: session.to_string(),
                }),
            ));
        }
        // Live preview: the dev-server trio plus the sight pair (screenshot +
        // console), sharing the service-wide manager so preview surfaces and
        // commands see the servers the tools start.
        let preview_sink = match &self.hooks.preview_sink {
            Some(factory) => factory(session),
            None => Arc::new(ProtocolPreviewSink {
                sink: self.sink.clone(),
                session: session.to_string(),
            }),
        };
        let (start_server, stop_server, server_logs) = harness_preview::session_tools(
            self.dev_servers.clone(),
            session,
            workspace_root,
            preview_sink,
        );
        // Auto-verify (Settings → Preview) decides how hard the model is
        // pushed to look at what it built before reporting done.
        let verify_hint = if harness_runtime::preview::auto_verify() {
            "The user can see the live app in the Preview panel next to the chat. \
             After each batch of code edits, verify your work before reporting \
             done: call preview_screenshot to look at the app and preview_console \
             to check for browser errors, and fix what you find."
        } else {
            "The user can see the live app in the Preview panel next to the chat. \
             The preview_screenshot and preview_console tools are available when \
             you need to check the running app."
        };
        tools.register_typed(start_server.with_verify_hint(verify_hint));
        tools.register_typed(stop_server);
        tools.register_typed(server_logs);
        let preview_lens = match &self.hooks.preview_lens {
            Some(factory) => factory(session),
            None => Arc::new(NoScreenshotLens),
        };
        let (screenshot, console) =
            harness_preview::sight_tools(self.dev_servers.clone(), session, preview_lens);
        tools.register_typed(screenshot);
        tools.register_typed(console);
        harness_runtime::tools::load().apply(tools);
        // Skills load on demand through the `skill` tool; it's only
        // registered when the user has enabled skills, so an empty set costs
        // no prompt tokens.
        if let Some(skill_tool) = harness_runtime::skills::enabled_tool(workspace_root) {
            tools.register_typed(skill_tool);
        }

        let system_prompt = format!(
            "{}{}",
            harness_agent::system_prompt_with_env(
                harness_agent::OptionalTools::from_registry(tools),
                workspace_root
            ),
            harness_runtime::project::prompt_section(workspace_root)
        );
        AgentConfig {
            model: model_label.to_string(),
            system_prompt: Some(system_prompt),
            context_window,
            attachment_root: Some(workspace_root.to_path_buf()),
            initial_attachments: harness_runtime::project::binary_context_paths(workspace_root),
            compression: harness_runtime::compression::mode(),
            // Retry attempts and failed turns append to
            // ~/.oxen-harness/errors.jsonl so a developer can dig in later.
            error_log: harness_config::paths::errors_log().ok(),
            // Gate tool calls behind the permission layer, with approval
            // prompts carried over the protocol (agent.approval_request ↔
            // answer_approval). Fleet/review subagents get the gate's
            // auto-deny form automatically.
            permissions: Some(Arc::new(harness_permissions::PermissionGate::new(
                workspace_root,
                Arc::new(HostApprover {
                    sink: self.sink.clone(),
                    session: session.to_string(),
                    pending: self.pending_approvals.clone(),
                }),
            ))),
            ..AgentConfig::default()
        }
    }

    /// Register the `spawn_agents` tool on a session's registry: the spawner
    /// snapshots the registry *before* the tool registers (subagents get
    /// every tool except the fleet itself — one level deep, no fork bombs)
    /// and is kept so a turn can wire its stop signal to it. Skipped entirely
    /// when the user disabled the tool.
    fn register_fleet(
        &self,
        session: &str,
        tools: &mut ToolRegistry,
        client: &OxenClient,
        config: &AgentConfig,
        usage_store: Arc<HistoryStore>,
    ) {
        if !harness_runtime::tools::load().is_enabled(harness_agent::FLEET_TOOL) {
            return;
        }
        let spawner = Arc::new(
            harness_agent::FleetSpawner::new(client.clone(), tools.clone(), config.clone())
                .with_usage_store(usage_store),
        );
        tools.register_typed(harness_agent::FleetTool::new(
            spawner.clone(),
            Arc::new(HostFleetSink {
                sink: self.sink.clone(),
                session: session.to_string(),
                source: harness_protocol::FleetSource::Turn,
            }),
        ));
        self.fleet_spawners
            .lock()
            .expect("fleet spawners poisoned")
            .insert(session.to_string(), spawner);
    }

    /// Build an agent for a brand-new session (creates the session row).
    fn new_agent(
        &self,
        client: OxenClient,
        model_label: &str,
        context_window: Option<usize>,
        workspace_root: &Path,
    ) -> Result<Agent, String> {
        let store = self.store()?;
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
        let (mut tools, store) = self.agent_parts(workspace_root, &session)?;
        let config = self.finish_tools(
            &mut tools,
            &session,
            model_label,
            context_window,
            workspace_root,
        );
        self.register_fleet(&session, &mut tools, &client, &config, store.clone());
        Agent::new(client, tools, store, session, config).map_err(|e| e.to_string())
    }

    /// Build an agent bound to an *existing* session, loading its transcript —
    /// used to resume a cold history session without leaking a throwaway
    /// session row. Rooted at `workspace_root` (the session's own directory).
    fn resume_agent(
        &self,
        client: OxenClient,
        model_label: &str,
        context_window: Option<usize>,
        session_id: String,
        workspace_root: &Path,
    ) -> Result<Agent, String> {
        let (mut tools, store) = self.agent_parts(workspace_root, &session_id)?;
        let config = self.finish_tools(
            &mut tools,
            &session_id,
            model_label,
            context_window,
            workspace_root,
        );
        self.register_fleet(&session_id, &mut tools, &client, &config, store.clone());
        Agent::resume_from_store(client, tools, store, session_id, config)
            .map_err(|e| e.to_string())
    }

    /// Build a fresh agent for a new session rooted at `root`, reusing the
    /// active local model if any.
    async fn build_fresh_agent(&self, root: &Path) -> Result<Agent, String> {
        let (client, label, ctx) = self.client_for().await?;
        self.new_agent(client, &label, ctx, root)
    }

    /// Build an agent bound to an existing session id, rooted at `root` (its
    /// own recorded workspace), without leaking a throwaway session row.
    async fn build_resumed_agent(&self, session_id: String, root: &Path) -> Result<Agent, String> {
        let (client, label, ctx) = self.client_for().await?;
        self.resume_agent(client, &label, ctx, session_id, root)
    }

    // --- Cache & lifecycle ----------------------------------------------------

    /// The active project's directory, falling back to the launch directory.
    pub async fn active_root(&self) -> PathBuf {
        let p = self.active_project.lock().await.clone();
        if p.as_os_str().is_empty() {
            launch_dir()
        } else {
            p
        }
    }

    /// The agent handle for a session id, if one is live in memory.
    pub async fn agent_for(&self, id: &str) -> Option<Arc<Mutex<Agent>>> {
        self.agents.lock().await.get(id).cloned()
    }

    /// A session's recorded working directory (its project), read from the
    /// store; falls back to the launch directory when unknown.
    pub fn session_workspace(&self, id: &str) -> PathBuf {
        self.store()
            .ok()
            .and_then(|s| s.session_meta(id).ok())
            .map(|m| PathBuf::from(m.workspace))
            .unwrap_or_else(launch_dir)
    }

    /// The `spawn_agents` spawner for a session, if one is registered.
    pub fn fleet_spawner_for(&self, session: &str) -> Option<Arc<harness_agent::FleetSpawner>> {
        self.fleet_spawners
            .lock()
            .expect("fleet spawners poisoned")
            .get(session)
            .cloned()
    }

    /// The live agent for a session, rehydrating it from the database if it
    /// isn't cached (evicted, or the first turn after a cold resume). The DB
    /// is the source of truth, so a rebuilt agent continues the exact
    /// conversation, in the session's own workspace.
    pub async fn agent_or_build(&self, session: &str) -> Result<Arc<Mutex<Agent>>, String> {
        if let Some(a) = self.agent_for(session).await {
            return Ok(a);
        }
        let root = self.session_workspace(session);
        let agent = self.build_resumed_agent(session.to_string(), &root).await?;
        let arc = Arc::new(Mutex::new(agent));
        Ok(self
            .agents
            .lock()
            .await
            .entry(session.to_string())
            .or_insert(arc)
            .clone())
    }

    /// Release cached agents we don't need in memory: everything except the
    /// current chat and any whose turn is still running. Dropped chats live
    /// on in the store and rehydrate via [`Self::agent_or_build`].
    pub async fn evict_idle(&self) {
        let current = { self.current.lock().await.clone() };
        let kept: std::collections::HashSet<String> = {
            let mut agents = self.agents.lock().await;
            agents.retain(|id, arc| {
                Some(id.as_str()) == current.as_deref() || arc.try_lock().is_err()
            });
            agents.keys().cloned().collect()
        };
        // The fleet-spawner map mirrors the agents map, so evict in lockstep.
        self.fleet_spawners
            .lock()
            .expect("fleet spawners poisoned")
            .retain(|id, _| kept.contains(id));
    }

    /// Register an agent under its session id, make it the current chat, then
    /// evict any now-idle background agents.
    pub async fn install_agent(&self, agent: Agent) -> SessionInfo {
        let info = self.info_for(&agent);
        self.agents
            .lock()
            .await
            .insert(info.session_id.clone(), Arc::new(Mutex::new(agent)));
        *self.current.lock().await = Some(info.session_id.clone());
        self.evict_idle().await;
        info
    }

    /// The current chat's agent, lazily building one on first use so a host
    /// always opens even without an API key configured.
    pub async fn current_agent(&self) -> Result<Arc<Mutex<Agent>>, String> {
        // Read + drop the `current` guard before locking `agents` — never
        // hold both, so the two maps can't form a lock-ordering cycle.
        let current = { self.current.lock().await.clone() };
        if let Some(id) = current {
            if let Some(a) = self.agent_for(&id).await {
                return Ok(a);
            }
        }
        let root = self.active_root().await;
        let agent = self.build_fresh_agent(&root).await?;
        let arc = Arc::new(Mutex::new(agent));
        let id = arc.lock().await.session_id().to_string();
        self.agents.lock().await.insert(id.clone(), arc.clone());
        *self.current.lock().await = Some(id);
        Ok(arc)
    }

    /// A session's live vitals.
    pub fn info_for(&self, agent: &Agent) -> SessionInfo {
        SessionInfo {
            model: agent.model().to_string(),
            workspace: self
                .session_workspace(agent.session_id())
                .display()
                .to_string(),
            session_id: agent.session_id().to_string(),
            tokens_used: agent.tokens_used(),
            context_tokens: agent.context_tokens(),
            context_window: agent.context_window(),
            compression_mode: agent.compression_mode().as_str().to_string(),
        }
    }

    // --- Session commands -------------------------------------------------------

    /// Report the current session info, initializing the agent if needed.
    pub async fn session_info(&self) -> Result<SessionInfo, String> {
        let arc = self.current_agent().await?;
        let agent = arc.lock().await;
        Ok(self.info_for(&agent))
    }

    /// Start a fresh chat session as its own agent. Any in-flight chats keep
    /// running in the background. Returns the new session's info.
    pub async fn new_session(&self) -> Result<SessionInfo, String> {
        let root = self.active_root().await;
        let agent = self.build_fresh_agent(&root).await?;
        Ok(self.install_agent(agent).await)
    }

    /// Switch to an existing session, returning its info and full transcript.
    /// Reuses the session's live agent if one exists; otherwise loads it cold
    /// from history. A chat still mid-turn can't be locked, so its transcript
    /// comes back empty with `running: true`.
    pub async fn resume_session(&self, id: &str) -> Result<SessionView, String> {
        // The session belongs to its own project; opening it enters that
        // project so new chats land in the same directory.
        let workspace = self.session_workspace(id);
        *self.active_project.lock().await = workspace.clone();

        let arc = match self.agent_for(id).await {
            Some(a) => a,
            None => {
                // Cold resume: build an agent bound to the existing session
                // (no throwaway row), then insert via the map entry so a
                // concurrent resume can't leave two behind.
                let agent = self
                    .build_resumed_agent(id.to_string(), &workspace)
                    .await?;
                let arc = Arc::new(Mutex::new(agent));
                self.agents
                    .lock()
                    .await
                    .entry(id.to_string())
                    .or_insert(arc)
                    .clone()
            }
        };
        *self.current.lock().await = Some(id.to_string());
        self.evict_idle().await;

        let view = match arc.try_lock() {
            Ok(agent) => {
                let messages = agent
                    .messages()
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                SessionView {
                    info: self.info_for(&agent),
                    messages,
                    running: false,
                }
            }
            // Mid-turn: can't read it. The client keeps its live in-memory
            // thread; the explicit `running` flag says not to touch it.
            Err(_) => SessionView {
                info: SessionInfo {
                    model: String::new(),
                    workspace: workspace.display().to_string(),
                    session_id: id.to_string(),
                    tokens_used: 0,
                    context_tokens: 0,
                    context_window: 0,
                    compression_mode: harness_runtime::compression::mode().as_str().to_string(),
                },
                messages: vec![],
                running: true,
            },
        };
        Ok(view)
    }

    /// List past chat sessions (those with at least one user message),
    /// newest first.
    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>, String> {
        self.store()?.list_sessions().map_err(|e| e.to_string())
    }

    /// Read a session's raw, persisted transcript straight from the store.
    /// Read-only and never touches the live agent, so it works mid-turn.
    pub fn session_messages(&self, id: &str) -> Result<Vec<serde_json::Value>, String> {
        self.store()?.messages(id).map_err(|e| e.to_string())
    }

    /// Permanently delete a chat session: remove it (and its messages) from
    /// history, drop any cached live agent, stop its dev server, and clear it
    /// as the current chat if it was active.
    pub async fn delete_session(&self, id: &str) -> Result<(), String> {
        self.store()?.delete_session(id).map_err(|e| e.to_string())?;
        self.agents.lock().await.remove(id);
        // A deleted chat's dev server has no owner left.
        self.dev_servers.stop(id).await;
        if let Some(hook) = &self.hooks.on_session_deleted {
            hook(id);
        }
        // Drop the session's fleet spawner in lockstep with its agent.
        self.fleet_spawners
            .lock()
            .expect("fleet spawners poisoned")
            .remove(id);
        let mut current = self.current.lock().await;
        if current.as_deref() == Some(id) {
            *current = None;
        }
        Ok(())
    }

    // --- Turns ---------------------------------------------------------------

    /// Run one user turn for a specific chat, streaming session-tagged
    /// protocol events; returns the final text. Holds only that session's
    /// lock, so turns in other chats keep running concurrently. `attachments`
    /// are host-readable file paths; unreadable ones are skipped so a bad
    /// path never blocks the turn.
    pub async fn run_turn(
        &self,
        session: &str,
        prompt: String,
        attachments: Vec<String>,
    ) -> Result<String, String> {
        let attachments: Vec<Attachment> = attachments
            .iter()
            .filter_map(|p| Attachment::from_path(p).ok())
            .collect();
        self.execute_turn(
            session,
            TurnKind::Fresh {
                prompt,
                attachments,
            },
        )
        .await
    }

    /// Retry a chat's failed turn (e.g. after its API key was set),
    /// continuing the same conversation. The user message from the failed
    /// attempt is already in the transcript, so this drives it again without
    /// re-appending it.
    pub async fn retry_turn(&self, session: &str) -> Result<String, String> {
        self.execute_turn(session, TurnKind::Retry).await
    }

    /// The shared body of a turn: rehydrate the agent, register a cancel
    /// token, run the turn while forwarding streamed events, then account for
    /// tokens and release idle background agents.
    ///
    /// A message [`Self::interject`] accepted in the instant *after* the turn
    /// loop's final drain would otherwise be stranded in the agent's buffer
    /// (and lost entirely if the agent is then evicted) — so after a
    /// successful turn, any leftover steering runs as its own follow-up turn,
    /// which is what its sender was promised by `accepted: true`.
    async fn execute_turn(&self, session: &str, kind: TurnKind) -> Result<String, String> {
        let mut kind = kind;
        loop {
            let (result, leftovers) = self.execute_one_turn(session, kind).await;
            match result {
                Ok(text) if leftovers.is_empty() => return Ok(text),
                Ok(_) => {
                    kind = TurnKind::Fresh {
                        prompt: leftovers.join("\n\n"),
                        attachments: Vec::new(),
                    };
                }
                // On a failed or cancelled turn the leftovers stay in the
                // agent's buffer (see below): a cached agent delivers them at
                // the next turn's first drain; only eviction loses them.
                Err(e) => return Err(e),
            }
        }
    }

    /// One turn of [`Self::execute_turn`]'s loop. Returns the turn's result
    /// plus any interjections still undelivered after a *successful* turn —
    /// drained here, while the agent is guaranteed alive, because
    /// `evict_idle` below may drop the agent (and its buffer) for background
    /// chats. A failed turn leaves them buffered for the next turn instead.
    async fn execute_one_turn(
        &self,
        session: &str,
        kind: TurnKind,
    ) -> (Result<String, String>, Vec<String>) {
        // Get the live agent or rehydrate it from the database.
        let arc = match self.agent_or_build(session).await {
            Ok(arc) => arc,
            Err(e) => return (Err(e), Vec::new()),
        };

        let sid = session.to_string();
        // The context window is fixed for the turn; capture it once so live
        // usage events can report "% of context". The steering handle rides
        // along so `interject` can reach the turn without the agent lock.
        let (context_window, steer) = {
            let agent = arc.lock().await;
            (agent.context_window(), agent.interjections())
        };
        // A fresh stop signal for this turn, registered so cancel_turn can
        // fire it without waiting on the agent lock the turn holds.
        let cancel = CancellationToken::new();
        self.cancels
            .lock()
            .await
            .insert(sid.clone(), cancel.clone());
        self.interjections
            .lock()
            .await
            .insert(sid.clone(), steer.clone());
        // Hand the turn's stop signal to the session's fleet spawner too, so
        // cancelling the turn also stops any fleet launched inside it.
        if let Some(spawner) = self.fleet_spawner_for(&sid) {
            spawner.set_cancel(cancel.clone());
        }
        let saved_delta;
        let token_batch = TokenBatch::new(self.sink.clone(), sid.clone());
        // A dev server that died between turns is otherwise invisible to the
        // model — ride a one-time note along with the next fresh prompt.
        let crash_note = self.crash_note(&sid);
        let result = {
            let mut agent = arc.lock().await;
            agent.set_cancel_token(cancel.clone());
            let saved_before = agent.tokens_saved();
            self.sink
                .emit(ProtocolEvent::TurnStarted { session: sid.clone() });
            let sink = self.sink.clone();
            let event_tokens = token_batch.clone();
            let event_session = sid.clone();
            let on_event = move |event: &AgentEvent| {
                // Tokens batch; everything else flushes first so ordering on
                // the wire matches ordering in the turn exactly.
                if let AgentEvent::Token(t) = event {
                    event_tokens.push(t);
                    return;
                }
                event_tokens.flush();
                if let Some(event) =
                    translate::agent_event(&event_session, context_window, event)
                {
                    sink.emit(event);
                }
            };
            let r = match kind {
                TurnKind::Fresh {
                    prompt,
                    attachments,
                } => {
                    let prompt = match crash_note {
                        Some(note) => format!("{prompt}\n\n{note}"),
                        None => prompt,
                    };
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
        // The turn is over (finished, stopped, or errored): drop its stop
        // signal so a later cancel can't fire against a stale token, and its
        // steering channel so a later interject falls back to a normal prompt.
        self.cancels.lock().await.remove(session);
        self.interjections.lock().await.remove(session);
        // After a successful turn, claim any steering that landed after the
        // final drain — before evict_idle below can drop the buffer with the
        // agent. The caller runs it as its own follow-up turn.
        let leftovers = if result.is_ok() {
            steer.take_all()
        } else {
            Vec::new()
        };
        // Account what compression saved (or would have, in audit mode).
        self.bump_total_tokens_saved(saved_delta);
        // The turn is persisted message-by-message; release idle background
        // agents so memory tracks concurrency, not chat count.
        self.evict_idle().await;
        let result = match result {
            Ok(text) => {
                self.sink.emit(ProtocolEvent::TurnCompleted {
                    session: sid,
                    text: text.clone(),
                });
                Ok(text)
            }
            Err(e) => {
                let error = e.to_string();
                self.sink.emit(ProtocolEvent::TurnFailed {
                    session: sid,
                    error: error.clone(),
                });
                Err(error)
            }
        };
        (result, leftovers)
    }

    /// Stop the in-flight turn for `session`, if any. Fires that turn's
    /// cancellation token, which breaks the streaming read and drops the HTTP
    /// connection. The turn returns its partial reply and settles normally; a
    /// no-op if the session isn't currently running.
    pub async fn cancel_turn(&self, session: &str) {
        if let Some(token) = self.cancels.lock().await.get(session) {
            token.cancel();
        }
    }

    /// Deliver a user message into `session`'s *running* turn (mid-turn
    /// steering): it enters the transcript at the turn loop's next safe point,
    /// so the model sees it during the work rather than after. Returns whether
    /// a running turn accepted it — on `false` (no turn in flight) the caller
    /// should send the text as an ordinary prompt instead.
    pub async fn interject(&self, session: &str, text: impl Into<String>) -> bool {
        match self.interjections.lock().await.get(session) {
            Some(handle) => {
                handle.push(text);
                true
            }
            None => false,
        }
    }

    /// Deliver the user's answer to a pending `ask_user_question`, unblocking
    /// the agent. Unknown ids are ignored (the question may have been
    /// cancelled).
    pub fn answer_question(&self, id: &str, answers: Vec<harness_protocol::QuestionAnswer>) {
        self.pending_questions.deliver(
            id,
            answers.into_iter().map(translate::question_answer).collect(),
        );
    }

    /// Deliver the user's decision on a pending permission approval,
    /// unblocking the gated tool call. Unknown ids are ignored.
    pub fn answer_approval(&self, id: &str, answer: harness_protocol::ApprovalAnswer) {
        self.pending_approvals.deliver(id, answer);
    }

    // --- Runners (code review, verification loops) -----------------------------

    /// Run the configurable code-review pipeline for a chat's workspace:
    /// uncommitted changes by default, or PR-style against `base_branch`.
    /// Streams progress via `review.*` / `fleet.*` events, then injects the
    /// findings into the session (as a settled user/assistant exchange) so
    /// follow-up turns can act on them. Holds the session's agent lock for
    /// the duration, so it can't interleave with a running turn;
    /// [`Self::cancel_turn`] stops it.
    pub async fn run_code_review(
        &self,
        session: &str,
        base_branch: Option<String>,
    ) -> Result<harness_protocol::ReviewResult, String> {
        use harness_review::{ReviewError, ReviewEvent};

        let arc = self.agent_or_build(session).await?;
        let root = self.session_workspace(session);
        let target = match base_branch.filter(|b| !b.trim().is_empty()) {
            Some(branch) => harness_review::ReviewTarget::BaseBranch(branch.trim().to_string()),
            None => harness_review::ReviewTarget::Uncommitted,
        };

        let cancel = CancellationToken::new();
        {
            // Registering under the session's key is also the mutual-exclusion
            // check: an existing entry means a turn (or another review) is
            // already in flight, and overwriting its token would orphan its
            // stop button.
            let mut cancels = self.cancels.lock().await;
            if cancels.contains_key(session) {
                return Err("a turn is already running in this chat".to_string());
            }
            cancels.insert(session.to_string(), cancel.clone());
        }

        let result = {
            let mut agent = arc.lock().await;
            let runner = harness_review::ReviewRunner::new(
                harness_review::ReviewConfig::load(),
                target.clone(),
                &root,
            )
            .with_cancel(cancel);
            let sid = session.to_string();
            let sink = self.sink.clone();
            let mut pipeline_tokens = 0usize;
            let run = runner
                .run(&agent, |event| match event {
                    ReviewEvent::StepStarted {
                        index,
                        total,
                        name,
                        agents,
                    } => {
                        sink.emit(ProtocolEvent::ReviewProgress {
                            session: sid.clone(),
                            step: name.clone(),
                            index: *index,
                            total: *total,
                            agents: agents.clone(),
                        });
                        // A fan-out step opens a lanes panel like spawn_agents.
                        if agents.len() > 1 {
                            sink.emit(ProtocolEvent::FleetStarted {
                                session: sid.clone(),
                                agents: agents.clone(),
                                source: harness_protocol::FleetSource::Review,
                            });
                        }
                    }
                    ReviewEvent::Agent(AgentEvent::Token(t)) => {
                        sink.emit(ProtocolEvent::ReviewToken {
                            session: sid.clone(),
                            token: t.clone(),
                        });
                    }
                    ReviewEvent::Agent(AgentEvent::ToolStart { name, .. }) => {
                        sink.emit(ProtocolEvent::ReviewTool {
                            session: sid.clone(),
                            name: name.clone(),
                        });
                    }
                    // A fan-out step's lanes ARE a fleet; the review forwards
                    // the FleetEvent verbatim, so it rides the exact same
                    // translation (and wire format) as a spawn_agents fleet.
                    ReviewEvent::Fleet(event) => {
                        if let Some(event) = translate::fleet_event(&sid, event) {
                            sink.emit(event);
                        }
                    }
                    ReviewEvent::StepCompleted { .. } => {
                        sink.emit(ProtocolEvent::FleetCompleted {
                            session: sid.clone(),
                        });
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
                    Ok(harness_protocol::ReviewResult {
                        status: "ok".into(),
                        user,
                        assistant,
                        findings: report.findings.len(),
                        tokens_used: pipeline_tokens,
                    })
                }
                Err(ReviewError::NothingToReview) => Ok(harness_protocol::ReviewResult {
                    status: "nothing".into(),
                    user: String::new(),
                    assistant: String::new(),
                    findings: 0,
                    tokens_used: 0,
                }),
                Err(ReviewError::Cancelled { tokens_used }) => {
                    Ok(harness_protocol::ReviewResult {
                        status: "cancelled".into(),
                        user: String::new(),
                        assistant: String::new(),
                        findings: 0,
                        // Reviewers that ran before the stop spent real
                        // tokens; report them so counters don't undercount.
                        tokens_used,
                    })
                }
                Err(e) => Err(e.to_string()),
            }
        };
        self.cancels.lock().await.remove(session);
        // Detached reviewers record their own provider usage in the ledger.
        self.evict_idle().await;
        result
    }

    /// Run a saved verification loop (or an ad-hoc one from `goal`) on a
    /// chat's agent, streaming its agent activity as `agent.*` events. Owns
    /// the session lock and cancellation token for the whole cycle.
    pub async fn run_loop(
        &self,
        session: &str,
        name: Option<String>,
        goal: Option<String>,
    ) -> Result<harness_protocol::LoopResult, String> {
        use harness_loop::{LoopEvent, LoopRunner, LoopSpec, LoopStore};

        let store = LoopStore::open().map_err(|e| e.to_string())?;
        let spec = if let Some(goal) = goal.filter(|s| !s.trim().is_empty()) {
            LoopSpec::from_goal(goal)
        } else {
            store
                .resolve(name.as_deref().unwrap_or("default"))
                .map_err(|e| e.to_string())?
        };
        let arc = self.agent_or_build(session).await?;
        let cancel = CancellationToken::new();
        {
            let mut cancels = self.cancels.lock().await;
            if cancels.contains_key(session) {
                return Err("a turn is already running in this chat".to_string());
            }
            cancels.insert(session.to_string(), cancel.clone());
        }
        let root = self.session_workspace(session);
        let runner =
            LoopRunner::new(spec.clone(), root).persisting_to(store.journal_path_for(&spec.name));
        let sid = session.to_string();
        let sink = self.sink.clone();
        let result = {
            let mut agent = arc.lock().await;
            agent.set_cancel_token(cancel);
            runner
                .run(&mut agent, |event| {
                    // Only the thread-visible slice rides the wire: streamed
                    // text and tool starts/ends.
                    if let LoopEvent::Agent(
                        event @ (AgentEvent::Token(_)
                        | AgentEvent::ToolStart { .. }
                        | AgentEvent::ToolEnd { .. }),
                    ) = event
                    {
                        if let Some(event) = translate::agent_event(&sid, 0, event) {
                            sink.emit(event);
                        }
                    }
                })
                .await
                .map(|journal| loop_result(&journal))
                .map_err(|e| e.to_string())
        };
        self.cancels.lock().await.remove(session);
        self.evict_idle().await;
        result
    }

    // --- Live settings swaps -----------------------------------------------------

    /// Switch the current chat to a downloaded local model: start
    /// `llama-server` (with a context window sized to this machine) and
    /// rebuild the agent against it. The model must already be downloaded.
    pub async fn use_local_model(&self, id: &str) -> Result<SessionInfo, String> {
        if llama_server_path().is_none() {
            return Err(format!(
                "the local runtime isn't installed. {}",
                harness_local::install_hint()
            ));
        }
        let store = ModelStore::open().map_err(|e| e.to_string())?;
        if !store.is_installed(id) {
            return Err(format!("{id} isn't downloaded yet"));
        }

        // Size the served context to the machine: weights + KV cache must
        // fit budget.
        let profile = harness_local::detect_hardware();
        let weight_bytes = store.installed_size(id).unwrap_or(0);
        let native = store.native_context(id);
        let context = fit::plan_context(profile.usable_budget, weight_bytes, native);

        // Stream load phases so the switch shows what it's doing (runtime
        // init vs. loading weights) instead of an opaque "Switching…".
        let sink = self.sink.clone();
        let model = id.to_string();
        let server = LocalServer::start_with_context(
            &store.path_for(id),
            id,
            context,
            move |phase| {
                sink.emit(ProtocolEvent::LocalStatus {
                    model: model.clone(),
                    phase: translate::local_phase(phase),
                });
            },
        )
        .await
        .map_err(|e| e.to_string())?;
        let context_window = Some(server.context_size() as usize);
        let root = self.active_root().await;
        let agent = self.new_agent(
            OxenClient::new(server.base_url(), "local", id),
            id,
            context_window,
            &root,
        )?;

        // Remember the local server + model so new sessions reuse it,
        // persist the choice so it survives a restart, then install the
        // agent as the current chat.
        *self.local_server.lock().await = Some(server);
        *self.local_model.lock().await = Some(id.to_string());
        let _ = harness_runtime::models::set_active_local(id);
        Ok(self.install_agent(agent).await)
    }

    /// Set the context-compression mode: persist it for new chats AND apply
    /// it to the live conversation in place, so a meter toggle takes effect
    /// on the very next model call.
    pub async fn set_compression_mode(
        &self,
        mode: harness_compress::CompressionMode,
    ) -> Result<SessionInfo, String> {
        harness_runtime::compression::set_mode(mode).map_err(|e| e.to_string())?;
        let arc = self.current_agent().await?;
        let mut agent = arc.lock().await;
        agent.set_compression_mode(mode);
        Ok(self.info_for(&agent))
    }

    /// Rebuild a session agent's client for its own model (picking up freshly
    /// saved credentials) without disturbing the model choice or the
    /// conversation — the inline "paste a key after a 401 and retry" path.
    pub async fn refresh_client(&self, session: &str) -> Result<(), String> {
        let arc = self.agent_or_build(session).await?;
        let mut agent = arc.lock().await;
        let client = (self.client_factory)(agent.model())?;
        agent.set_client(client.clone());
        // Keep the fleet spawner on the same endpoint, so a spawn_agents
        // fleet launched after the key is set doesn't keep failing.
        if let Some(spawner) = self.fleet_spawner_for(session) {
            spawner.set_client(client);
        }
        Ok(())
    }

    // --- Accounting & notes ----------------------------------------------------

    /// The all-time tokens compression saved (mode `on`) or would have saved
    /// (mode `audit`) across every session.
    pub fn total_tokens_saved(&self) -> Result<usize, String> {
        Ok(self
            .store()?
            .meta_get_i64(TOTAL_TOKENS_SAVED_KEY)
            .map_err(|e| e.to_string())?
            .unwrap_or(0)
            .max(0) as usize)
    }

    /// Add a turn's compression savings to the all-time counter and return
    /// the new grand total. Best-effort: never fails a turn.
    pub fn bump_total_tokens_saved(&self, delta: usize) -> usize {
        if delta == 0 {
            return 0; // the common case (compression off) — skip the DB trip
        }
        let Ok(store) = self.store() else {
            return 0;
        };
        store
            .meta_add_i64(TOTAL_TOKENS_SAVED_KEY, delta as i64)
            .map(|v| v.max(0) as usize)
            .unwrap_or(0)
    }

    /// A one-time note for the model when `session`'s dev server has died
    /// since it was last told — without it the model, having been told the
    /// server "keeps running across turns", would cheerfully point the user
    /// at a preview that is showing a crash.
    pub fn crash_note(&self, session: &str) -> Option<String> {
        let status = self.dev_servers.get(session)?.status();
        if status.phase != harness_preview::PreviewPhase::Error {
            // Healthy again — a later crash is worth announcing afresh.
            self.crash_announced
                .lock()
                .expect("crash announcements poisoned")
                .remove(session);
            return None;
        }
        let message = status.message.unwrap_or_else(|| "it stopped".into());
        let mut announced = self
            .crash_announced
            .lock()
            .expect("crash announcements poisoned");
        if announced.get(session) == Some(&message) {
            return None; // already told them about this one
        }
        announced.insert(session.to_string(), message.clone());
        Some(format!(
            "[system] The dev server for this project is no longer running — {message}. \
             The live preview is showing an error, not the app. If the user's request \
             involves the running app, diagnose why it stopped (dev_server_logs) and \
             restart it with start_dev_server."
        ))
    }

    /// The complete tool set for settings purposes: the workspace defaults
    /// plus the host-bridged tools wired to inert bridges — a settings page
    /// only reads names, descriptions, and schemas. Matches what a real agent
    /// registers, so every manageable tool appears and custom-tool names
    /// can't shadow any of them.
    pub async fn settings_registry(&self) -> Result<ToolRegistry, String> {
        let root = self.active_root().await;
        let workspace = Workspace::new(&root).map_err(|e| e.to_string())?;
        let brave_key = harness_runtime::connection::brave_key_override();
        let mut registry =
            ToolRegistry::default_for_workspace_with_web_key(workspace.clone(), brave_key);
        registry.register_typed(AskUserTool::new(Arc::new(NullAsker)));
        registry.register_typed(CanvasTool::new(Arc::new(NullCanvasSink)));
        registry.register_typed(harness_tools::OpenFileTool::new(
            workspace,
            Arc::new(NullViewerSink),
        ));
        // Inert dev-server trio + sight pair (fresh manager, never started).
        let (start_server, stop_server, server_logs) = harness_preview::session_tools(
            harness_preview::DevServerManager::new(),
            "settings",
            &root,
            Arc::new(NullPreviewSink),
        );
        registry.register_typed(start_server);
        registry.register_typed(stop_server);
        registry.register_typed(server_logs);
        let (screenshot, console) = harness_preview::sight_tools(
            harness_preview::DevServerManager::new(),
            "settings",
            Arc::new(NullPreviewLens),
        );
        registry.register_typed(screenshot);
        registry.register_typed(console);
        // An inert `spawn_agents` (never run — only name/description/schema
        // are read), so the fleet is manageable like any tool.
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
}

#[cfg(test)]
mod tests {
    use harness_loop::{LoopJournal, StopReason};

    use super::loop_result;

    #[test]
    fn loop_result_distinguishes_success_from_a_stopped_run() {
        let mut success = LoopJournal::new("green", "make checks pass");
        success.finish(StopReason::Succeeded);
        let result = loop_result(&success);
        assert!(result.succeeded);
        assert!(result.summary.contains("all gates passed"));

        let mut stopped = LoopJournal::new("green", "make checks pass");
        stopped.finish(StopReason::MaxIterations);
        let result = loop_result(&stopped);
        assert!(!result.succeeded);
        assert!(result.summary.contains("iteration limit"));
    }
}
