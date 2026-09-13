//! `spawn_agents` — the model-facing face of the fleet: fan a job out across
//! N parallel subagents from inside any turn.
//!
//! Because it's an ordinary registered tool, it works in every mode — a chat
//! turn in the CLI or desktop, a loop pass, a review step — with no special
//! plumbing per surface. The pieces:
//!
//! - [`FleetSpawner`] — builds each subagent: the same client, tools, and
//!   config as the session's agent, minus `spawn_agents` itself (a subagent
//!   cannot fan out again — one level deep, no fork bombs), on an in-memory
//!   store so nothing touches the user's session.
//! - [`FleetSink`] — the host's lanes display, injected at registry build time
//!   (the `CanvasSink` pattern). Bracketed by a drop guard, so a turn that is
//!   cancelled mid-fleet still tears the display down.
//! - Cancellation — each run uses a child of the token in the spawner's slot
//!   (hosts refresh it per turn), so stopping the turn stops the fleet; the
//!   child token also goes to the sink, so a host can stop *just the fleet*.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{ToolError, ToolRegistry, TypedTool};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::error::AgentError;
use crate::fleet::{run_fleet, FleetEvent, FleetLimits, FleetSink, SpawnAgent, SubagentTask};
use crate::lane::{lane_budget, render_results, AgentTree, LiveLane, SubagentResult};
use harness_llm::types::ChatMessage;

/// Stable identifier the model uses to call the fleet tool.
pub const FLEET_TOOL: &str = "spawn_agents";

/// Most agents one call may spawn (also stated in the model-facing schema).
pub const MAX_FLEET_AGENTS: usize = 6;

/// Default (and ceiling) for how many subagents run at once.
pub const DEFAULT_FLEET_PARALLEL: usize = 3;

/// Most fleets one session may have in flight at once. `wait: false` lets a
/// call return before its fleet ends, so without a ceiling N calls would be
/// N × `max_parallel` lanes, unbounded.
pub const MAX_LIVE_FLEETS: usize = 3;

/// A fresh fleet id, unique for the life of this process. Hosts key lanes
/// panels and stop buttons by it, so two fleets overlapping in one session
/// never collide.
pub(crate) fn next_fleet_id() -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    format!("fleet-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Builds the detached agents a fleet runs on, from the session agent's
/// client, tools, and config. The tool registry is snapshotted at build time
/// (taken *before* `spawn_agents` registers, so subagents can't recurse), but
/// the client and config live behind a mutex so a host that swaps the live
/// agent's endpoint or model — a `/model` switch, an API key pasted after a
/// 401 — can keep the spawner in step; otherwise fleets would keep running on
/// the stale client/model captured at startup.
pub struct FleetSpawner {
    tools: ToolRegistry,
    /// The project root, so a fleet can cut worktrees from it. `None` (a
    /// spawner built without one) simply never isolates.
    workspace_root: StdMutex<Option<std::path::PathBuf>>,
    /// The client + config subagents are built from, updated in lockstep with
    /// the live agent via [`FleetSpawner::set_client`] / [`set_model`].
    endpoint: StdMutex<Endpoint>,
    /// The current turn's stop signal; hosts refresh it when they install a
    /// turn's token so cancelling the turn cancels any running fleet too.
    cancel: StdMutex<CancellationToken>,
    /// The parent's history store. Lanes persist their transcripts here as
    /// sessions under the spawning session (so a finished lane can be read
    /// back and resumed) and their spend lands in its ledger. `None` (tests)
    /// keeps lanes in memory.
    store: Option<Arc<HistoryStore>>,
    /// The session lanes are spawned from — their `parent_session`, and
    /// where their spend is attributed (see [`FleetSpawner::set_session`]).
    /// A slot rather than a builder argument: the CLI registers the tool
    /// before its session exists.
    session: StdMutex<Option<String>>,
    /// The lanes running right now, for a host to stop or steer one.
    tree: Arc<AgentTree>,
    /// How many fleets are in flight — a `wait: false` fleet can overlap a
    /// later call, and a session holds at most [`MAX_LIVE_FLEETS`].
    live: Arc<AtomicUsize>,
    /// The host's lanes display, once a `FleetTool` is built on this
    /// spawner; a lane that may spawn its own fleet renders through it too.
    sink: StdMutex<Option<Arc<dyn FleetSink>>>,
    /// `map_agents` rows already answered this session, by
    /// (item, task, schema) — a stopped run re-issued only runs what's left.
    /// Mirrored to the session's state so it survives a restart.
    memo: StdMutex<std::collections::HashMap<u64, SubagentResult>>,
    memo_loaded: std::sync::atomic::AtomicBool,
    /// The parent's transcript as of its latest `spawn_agents` call, for
    /// lanes spawned with `fork: true` (see [`ForkSlot`]).
    fork: ForkSlot,
}

/// Where a session's agent publishes its transcript right before a
/// `spawn_agents` call, so a `fork: true` lane can start from it: a fork
/// inherits everything the parent has read and decided so far, which a
/// fresh lane would have to be told. Shared by the agent and its spawner.
pub type ForkSlot = Arc<StdMutex<Option<Arc<Vec<ChatMessage>>>>>;

/// The mutable half of a [`FleetSpawner`]: what subagents inherit that can
/// change over a session's life.
struct Endpoint {
    client: OxenClient,
    config: AgentConfig,
}

impl FleetSpawner {
    pub fn new(client: OxenClient, tools: ToolRegistry, mut config: AgentConfig) -> Self {
        // Every lane of a turn spends from one wallet; a host that didn't
        // hand one in gets the defaults.
        config
            .tree
            .get_or_insert_with(|| Arc::new(crate::tree::TreeBudget::default()));
        Self {
            tools,
            workspace_root: StdMutex::new(None),
            endpoint: StdMutex::new(Endpoint { client, config }),
            cancel: StdMutex::new(CancellationToken::new()),
            store: None,
            session: StdMutex::new(None),
            tree: Arc::default(),
            live: Arc::default(),
            sink: StdMutex::new(None),
            memo: StdMutex::new(std::collections::HashMap::new()),
            memo_loaded: std::sync::atomic::AtomicBool::new(false),
            fork: Arc::default(),
        }
    }

    /// The slot the session's agent publishes fork snapshots into.
    pub fn fork_slot(&self) -> ForkSlot {
        self.fork.clone()
    }

    /// The transcript a fork starts from, if the parent has published one.
    fn fork_source(&self) -> Option<Arc<Vec<ChatMessage>>> {
        self.fork.lock().expect("fork slot poisoned").clone()
    }

    /// A finished `map_agents` row under this key, if one was memoized —
    /// in this spawner, or by an earlier spawner of the same session (the
    /// memo persists beside the session, so a restart doesn't re-run
    /// answered items).
    pub(crate) fn memo_get(&self, key: u64) -> Option<SubagentResult> {
        self.load_memo();
        self.memo
            .lock()
            .expect("fleet memo poisoned")
            .get(&key)
            .cloned()
    }

    pub(crate) fn memo_put(&self, key: u64, result: SubagentResult) {
        self.load_memo();
        let snapshot = {
            let mut memo = self.memo.lock().expect("fleet memo poisoned");
            memo.insert(key, result);
            memo.clone()
        };
        if let (Some(store), Some(session)) = (&self.store, self.session()) {
            let rows: std::collections::HashMap<String, SubagentResult> = snapshot
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            let _ = store.save_session_state(&session, harness_store::MAP_MEMO_STATE, &rows);
        }
    }

    /// Pull the session's persisted memo in once, the first time it's needed.
    fn load_memo(&self) {
        if self.memo_loaded.swap(true, Ordering::SeqCst) {
            return;
        }
        let (Some(store), Some(session)) = (&self.store, self.session()) else {
            return;
        };
        let Ok(Some(rows)) = store
            .session_state::<std::collections::HashMap<String, SubagentResult>>(
                &session,
                harness_store::MAP_MEMO_STATE,
            )
        else {
            return;
        };
        let mut memo = self.memo.lock().expect("fleet memo poisoned");
        for (key, result) in rows {
            if let Ok(key) = key.parse::<u64>() {
                memo.entry(key).or_insert(result);
            }
        }
    }

    /// The registry's overflow store — where parked content lives.
    pub fn overflow_store(&self) -> Option<Arc<harness_compress::CcrStore>> {
        self.tools.overflow_store().cloned()
    }

    /// The client and config lanes are built from right now.
    pub(crate) fn endpoint_snapshot(&self) -> (OxenClient, AgentConfig) {
        let endpoint = self.endpoint.lock().expect("fleet endpoint poisoned");
        (endpoint.client.clone(), endpoint.config.clone())
    }

    /// The wallet every lane of this session's turn spends from.
    pub fn tree_budget(&self) -> Arc<crate::tree::TreeBudget> {
        self.endpoint
            .lock()
            .expect("fleet endpoint poisoned")
            .config
            .tree
            .clone()
            .expect("a spawner always carries a tree budget")
    }

    /// Where lanes render; set when a `FleetTool` is built on this spawner.
    pub(crate) fn set_sink(&self, sink: Arc<dyn FleetSink>) {
        *self.sink.lock().expect("fleet sink slot poisoned") = Some(sink);
    }

    fn sink(&self) -> Option<Arc<dyn FleetSink>> {
        self.sink.lock().expect("fleet sink slot poisoned").clone()
    }

    /// A spawner for a lane that may spawn lanes of its own: the same tools
    /// snapshot, store, display, and live registry, but the lane's config
    /// (one level deeper, same tree budget), the lane as the parent of what
    /// it spawns, and the lane's own stop token — stopping the lane stops
    /// its children.
    fn child(
        &self,
        lane_config: &AgentConfig,
        lane_session: &str,
        cancel: &CancellationToken,
    ) -> Arc<FleetSpawner> {
        let client = self
            .endpoint
            .lock()
            .expect("fleet endpoint poisoned")
            .client
            .clone();
        let mut child = FleetSpawner::new(client, self.tools.clone(), lane_config.clone());
        *child
            .workspace_root
            .get_mut()
            .expect("fleet workspace poisoned") = self.root();
        child.store = self.store.clone();
        child.tree = self.tree.clone();
        *child
            .session
            .get_mut()
            .expect("fleet session slot poisoned") = Some(lane_session.to_string());
        *child.cancel.get_mut().expect("fleet cancel slot poisoned") = cancel.clone();
        *child.sink.get_mut().expect("fleet sink slot poisoned") = self.sink();
        Arc::new(child)
    }

    /// Give a lane its own agent tools on a child spawner: `ask_model` at
    /// every depth (a leaf may still ask cheap tool-less questions), and
    /// below the depth cap the fleet tools too. Leaves get no fleet tools:
    /// `subagent_tools` already stripped the parent's.
    fn add_nested_tools(
        &self,
        tools: &mut ToolRegistry,
        lane_config: &AgentConfig,
        lane_session: &str,
        cancel: &CancellationToken,
    ) {
        let child = self.child(lane_config, lane_session, cancel);
        tools.register_typed(crate::ask_tool::AskModelTool::new(child.clone()));
        if !lane_config.may_spawn() {
            return;
        }
        let Some(sink) = self.sink() else {
            return;
        };
        tools.register_typed(
            FleetTool::new(child.clone(), sink.clone()).with_asides(tools.asides()),
        );
        tools.register_typed(
            crate::map_tool::MapAgentsTool::new(child.clone(), sink.clone())
                .with_asides(tools.asides()),
        );
        tools.register_typed(crate::lane_tools::SendToAgentTool::new(child.clone(), sink));
        tools.register_typed(crate::lane_tools::ReadAgentTool::new(child));
    }

    /// The lanes running right now (see [`AgentTree`]).
    pub fn tree(&self) -> &Arc<AgentTree> {
        &self.tree
    }

    /// The parent's store, when lanes persist.
    pub fn store(&self) -> Option<&Arc<HistoryStore>> {
        self.store.as_ref()
    }

    /// The spawning session, once known.
    pub fn session(&self) -> Option<String> {
        self.session
            .lock()
            .expect("fleet session slot poisoned")
            .clone()
    }

    /// Refuse a new fleet of `lanes` when the session already has
    /// [`MAX_LIVE_FLEETS`] in flight, or the tree budget has no room for
    /// more lanes.
    pub(crate) fn admit_fleet(&self, lanes: u32) -> Result<(), ToolError> {
        if self.live.load(Ordering::SeqCst) >= MAX_LIVE_FLEETS {
            return Err(ToolError::Execution(format!(
                "{MAX_LIVE_FLEETS} fleets are already running in this session; their results \
                 arrive automatically — wait for them before starting another"
            )));
        }
        self.tree_budget()
            .admit_spawn(lanes)
            .map_err(ToolError::Execution)
    }

    /// Tell the spawner which project it is working in, enabling `isolation:
    /// "worktree"`. Without it a fleet always shares the parent's workspace.
    pub fn with_workspace(self, root: impl Into<std::path::PathBuf>) -> Self {
        *self
            .workspace_root
            .lock()
            .expect("fleet workspace poisoned") = Some(root.into());
        self
    }

    /// Cut one detached worktree per lane, or `None` when the project isn't a
    /// git repository (isolation is an upgrade, not a precondition). The
    /// lanes belong to the fleet run that opened them — the spawner keeps no
    /// slot of its own, so two fleets in flight at once (a `wait: false`
    /// fleet plus a foreground one) can't tear down each other's checkouts.
    fn open_lanes(
        &self,
        count: usize,
    ) -> Option<Vec<std::sync::Arc<crate::worktree::LaneWorktree>>> {
        let root = self
            .workspace_root
            .lock()
            .expect("fleet workspace poisoned")
            .clone()?;
        let lanes: Vec<_> = crate::worktree::create(&root, "fleet", count)?
            .into_iter()
            .map(std::sync::Arc::new)
            .collect();
        Some(lanes)
    }

    /// The project root, when the host told us about one.
    fn root(&self) -> Option<std::path::PathBuf> {
        self.workspace_root
            .lock()
            .expect("fleet workspace poisoned")
            .clone()
    }

    /// Persist lanes (and their spend) in `store`, the parent's history.
    pub fn with_store(mut self, store: Arc<HistoryStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// [`Self::with_store`] under its older name.
    pub fn with_usage_store(self, store: Arc<HistoryStore>) -> Self {
        self.with_store(store)
    }

    /// Attribute future lanes' spend to `session` in that ledger — the chat
    /// the fleet runs in, so its cost meter counts what its lanes burn.
    pub fn with_session(self, session: impl Into<String>) -> Self {
        self.set_session(session);
        self
    }

    /// [`Self::with_session`] for a spawner that already exists.
    pub fn set_session(&self, session: impl Into<String>) {
        *self.session.lock().expect("fleet session slot poisoned") = Some(session.into());
    }

    /// Point future subagents at a new inference client — call it wherever the
    /// live agent's client is swapped ([`Agent::set_client`]).
    pub fn set_client(&self, client: OxenClient) {
        self.endpoint
            .lock()
            .expect("fleet endpoint poisoned")
            .client = client;
    }

    /// Point future subagents at a new model — call it wherever the live
    /// agent's model is swapped ([`Agent::set_model`]).
    pub fn set_model(&self, model: impl Into<String>) {
        self.endpoint
            .lock()
            .expect("fleet endpoint poisoned")
            .config
            .model = model.into();
    }

    /// Install the turn's stop signal (hosts call this alongside
    /// [`Agent::set_cancel_token`]); in-flight fleets keep the token they
    /// started with.
    pub fn set_cancel(&self, token: CancellationToken) {
        *self.cancel.lock().expect("fleet cancel slot poisoned") = token;
    }

    /// A stop signal for one fleet run: a child of the turn's token, so the
    /// turn stopping stops the fleet, while a host can also stop just the
    /// fleet without killing the turn.
    fn run_token(&self) -> CancellationToken {
        self.cancel
            .lock()
            .expect("fleet cancel slot poisoned")
            .child_token()
    }

    /// One detached subagent: a session of its own under the spawning
    /// session (in memory when there is no store), the current client/config
    /// through [`AgentConfig::for_subagent`], and the subagent-narrowed tool
    /// set. `lane` is the isolated checkout this subagent works in, when the
    /// fleet opened one for it; `cancel` is the token that stops just this
    /// lane, registered with the tree so a host can fire it.
    pub(crate) fn build_agent(
        &self,
        label: &str,
        fleet: &str,
        lane: Option<std::sync::Arc<crate::worktree::LaneWorktree>>,
        cancel: CancellationToken,
    ) -> Result<Agent, AgentError> {
        self.build_agent_with(label, fleet, lane, cancel, false)
    }

    /// [`Self::build_agent`], optionally as a fork: the lane starts from the
    /// parent's published transcript (its system prompt made a lane's, the
    /// trail mandate gone and the lane appendix added) instead of a fresh
    /// context.
    pub(crate) fn build_agent_with(
        &self,
        label: &str,
        fleet: &str,
        lane: Option<std::sync::Arc<crate::worktree::LaneWorktree>>,
        cancel: CancellationToken,
        fork: bool,
    ) -> Result<Agent, AgentError> {
        // Everything a lane inherits differently from its parent — model
        // role, gate, round budget, attachments, prompt — is decided in one
        // place, shared with `Agent::side_agent`.
        let (client, config) = {
            let endpoint = self.endpoint.lock().expect("fleet endpoint poisoned");
            (endpoint.client.clone(), endpoint.config.for_subagent())
        };
        // An isolated lane works in its own checkout, so its file and shell
        // tools must point there rather than at the shared project.
        let tools = match (&lane, self.root()) {
            // An isolated lane works in its own checkout, so its file and
            // shell tools point there, with file state of its own (nothing
            // else can touch that tree).
            (Some(lane), _) => rooted_tools(
                &self.tools,
                lane.path(),
                self.tools
                    .files()
                    .map(|files| files.fresh_with_rules())
                    .unwrap_or_else(harness_tools::FileState::gated),
            ),
            // A shared lane keeps the parent's file state — that shared state
            // is what makes the per-path lock serialize two lanes editing one
            // file — but still gets its own shell, since `run_shell` now
            // carries a working directory and environment between calls and
            // one lane's `cd` must not relocate another's next command.
            (None, Some(root)) => {
                let files = self
                    .tools
                    .files()
                    .cloned()
                    .unwrap_or_else(harness_tools::FileState::gated);
                rooted_tools(&self.tools, &root, files)
            }
            (None, None) => self.tools.clone(),
        };
        let parent = self.session();
        let meta = SessionMeta {
            model: config.model.clone(),
            workspace: self
                .root()
                .map(|r| r.display().to_string())
                .unwrap_or_default(),
            parent_session: parent.clone().unwrap_or_default(),
            ..Default::default()
        };
        // A lane's transcript lives in the parent's store, under the parent,
        // so it can be inspected and resumed later; without a store (tests)
        // it lives and dies in memory.
        let (store, persisted) = match (&self.store, &parent) {
            (Some(store), Some(_)) => (store.clone(), true),
            _ => (Arc::new(HistoryStore::open_in_memory()?), false),
        };
        // A fork with nothing to fork from is refused before any session row
        // exists for it.
        let source = match fork {
            true => Some(self.fork_source().ok_or_else(|| {
                AgentError::Tool(ToolError::Execution(
                    "fork: true needs a conversation to fork from, and none was published for \
                     this call"
                        .into(),
                ))
            })?),
            false => None,
        };
        let session = store.create_session(&meta)?;
        let mut tools = crate::agent::subagent_tools(tools);
        self.add_nested_tools(&mut tools, &config, &session, &cancel);
        let mut agent = if let Some(source) = source {
            // The parent's messages become the lane's starting context as one
            // snapshot row (not a message row each: a long conversation forked
            // twice must not double the store), with the parent's system
            // prompt rewritten the way `for_subagent` does.
            let mut inherited: Vec<ChatMessage> = source.as_ref().clone();
            match inherited.first_mut() {
                Some(first) if first.role == "system" => {
                    if let Some(prompt) = &config.system_prompt {
                        *first = ChatMessage::system(prompt.clone());
                    }
                }
                _ => {
                    if let Some(prompt) = &config.system_prompt {
                        inherited.insert(0, ChatMessage::system(prompt.clone()));
                    }
                }
            }
            store.save_context_snapshot(&session, -1, &inherited)?;
            Agent::resume_from_store(client, tools, store, session, config)?
        } else {
            Agent::new_with(client, tools, store, session, config, false)?
        };
        if !persisted {
            agent.disable_transcript_persistence();
            if let Some(store) = &self.store {
                agent.set_usage_store(store.clone());
            }
        }
        self.adopt(&mut agent, label, fleet, cancel);
        Ok(agent)
    }

    /// Bring a finished lane back with its transcript, for a follow-up.
    /// The lane must belong to this session.
    pub(crate) fn resume_lane(
        &self,
        id: &str,
        label: &str,
        fleet: &str,
        cancel: CancellationToken,
    ) -> Result<Agent, AgentError> {
        let store = self.owned_lane_store(id)?;
        // Two agents on one transcript would interleave their rounds; a lane
        // that is still running gets its result delivered, not a second turn.
        if self.tree.is_live(id) {
            return Err(AgentError::Tool(ToolError::Execution(format!(
                "agent {id} is still running — wait for its result (it arrives \
                 automatically) or stop it first"
            ))));
        }
        let (client, config) = {
            let endpoint = self.endpoint.lock().expect("fleet endpoint poisoned");
            (endpoint.client.clone(), endpoint.config.for_subagent())
        };
        let tools = match self.root() {
            Some(root) => {
                let files = self
                    .tools
                    .files()
                    .cloned()
                    .unwrap_or_else(harness_tools::FileState::gated);
                rooted_tools(&self.tools, &root, files)
            }
            None => self.tools.clone(),
        };
        let mut tools = crate::agent::subagent_tools(tools);
        self.add_nested_tools(&mut tools, &config, id, &cancel);
        let mut agent = Agent::resume_from_store(client, tools, store, id.to_string(), config)?;
        self.adopt(&mut agent, label, fleet, cancel);
        Ok(agent)
    }

    /// The store holding lane `id`, checked to be one of this session's
    /// lanes — a lane id from another chat (or a chat id) is refused.
    pub(crate) fn owned_lane_store(&self, id: &str) -> Result<Arc<HistoryStore>, AgentError> {
        let not_mine = || {
            AgentError::Tool(ToolError::InvalidArguments(format!(
                "no agent {id} belongs to this session — use an agent id from a spawn_agents \
                 result"
            )))
        };
        let (Some(store), Some(session)) = (&self.store, self.session()) else {
            return Err(not_mine());
        };
        let meta = store.session_meta(id).map_err(|_| not_mine())?;
        if meta.parent_session != session {
            return Err(not_mine());
        }
        Ok(store.clone())
    }

    /// What every lane gets after construction: spend attributed to the
    /// spawning session, its stop token, and a place in the live registry.
    fn adopt(&self, agent: &mut Agent, label: &str, fleet: &str, cancel: CancellationToken) {
        if let Some(session) = self.session() {
            agent.set_usage_session(session);
        }
        agent.set_cancel_token(cancel.clone());
        self.tree.register(LiveLane {
            id: agent.session_id().to_string(),
            label: label.to_string(),
            fleet: fleet.to_string(),
            started: std::time::Instant::now(),
            cancel,
            steer: agent.interjections(),
        });
    }

    /// Run `tasks` as one named fleet on lanes from `spawn`, bracketed on the
    /// host's lanes display, and type every outcome. Lane transcripts are
    /// persisted as they run; the typed record is written by
    /// [`Self::record`] once the caller has finished with it (a patch may
    /// still be attached). Returns the results and whether the fleet's token
    /// was cancelled.
    pub(crate) async fn run_lanes<S: SpawnAgent>(
        &self,
        sink: &Arc<dyn FleetSink>,
        fleet: &str,
        tasks: Vec<SubagentTask>,
        limits: FleetLimits,
        spawn: S,
    ) -> Result<(Vec<SubagentResult>, bool), ToolError> {
        let labels: Vec<String> = tasks.iter().map(|t| t.label.clone()).collect();
        let cancel = self.run_token();
        let (_, config) = self.endpoint_snapshot();
        crate::errlog::record(
            config.error_log.as_deref(),
            "fleet_started",
            serde_json::json!({
                "session": self.session(),
                "fleet": fleet,
                "depth": config.depth,
                "lanes": labels,
                "tree": self.tree_budget().usage(),
            }),
        );
        let started = std::time::Instant::now();
        let guard = SinkGuard::open(
            sink.clone(),
            self.live.clone(),
            fleet,
            &labels,
            cancel.clone(),
        );
        let tree = self.tree_budget();
        let _inflight = InFlight::begin(tree.clone());
        let outcomes = run_fleet(spawn, tasks, limits, cancel.clone(), |event| {
            sink.event(fleet, event);
            // Spend changed: let the host show where the tree stands.
            let spend_changed = match event {
                FleetEvent::Agent { event, .. } => {
                    matches!(event.as_ref(), crate::event::AgentEvent::Usage { .. })
                }
                FleetEvent::TaskCompleted { .. } => true,
                _ => false,
            };
            if spend_changed {
                sink.event(
                    fleet,
                    &FleetEvent::Budget {
                        usage: tree.usage(),
                        limits: tree.limits(),
                    },
                );
            }
        })
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;
        drop(guard); // normal teardown; the guard covers the abnormal paths
        self.tree.finish_fleet(fleet);

        // What the parent reads is bounded; a lane that pasted a whole file
        // is cut, with the rest parked in the registry's overflow store so
        // `retrieve_original` (or `read_agent`) can still fetch it.
        let budget = lane_budget(outcomes.len());
        let spill = self.tools.overflow_store();
        let results: Vec<SubagentResult> = outcomes
            .iter()
            .map(|o| SubagentResult::from_outcome(o, fleet, budget, spill.map(Arc::as_ref)))
            .collect();
        // The trajectory: one line per fleet with every lane's verdict and
        // spend, beside the turn's other developer-log events, so a tree
        // can be reconstructed after the fact (`jq 'select(.fleet == …)'`).
        crate::errlog::record(
            config.error_log.as_deref(),
            "fleet_finished",
            serde_json::json!({
                "session": self.session(),
                "fleet": fleet,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "cancelled": cancel.is_cancelled(),
                "lanes": results.iter().map(|r| serde_json::json!({
                    "id": r.id,
                    "label": r.label,
                    "status": r.status,
                    "failure": r.failure,
                    "stop": r.stop,
                    "tokens": r.tokens,
                    "rounds": r.rounds,
                    "denied": r.denied.len(),
                })).collect::<Vec<_>>(),
                "tree": self.tree_budget().usage(),
            }),
        );
        Ok((results, cancel.is_cancelled()))
    }

    /// Persist each lane's typed record beside its transcript.
    pub(crate) fn record(&self, results: &[SubagentResult]) {
        let Some(store) = &self.store else {
            return;
        };
        for result in results {
            if result.id.is_empty() {
                continue;
            }
            let _ = store.save_session_state(&result.id, harness_store::LANE_STATE, result);
        }
    }
}

/// The parent's tool set with every workspace-rooted tool rebuilt against
/// `root`, so an isolated lane reads, writes, greps, and shells inside its own
/// checkout. Host-surface tools (canvas, preview, custom HTTP tools) are
/// carried over untouched — they aren't path-scoped.
fn rooted_tools(
    base: &ToolRegistry,
    root: &std::path::Path,
    files: std::sync::Arc<harness_tools::FileState>,
) -> ToolRegistry {
    use harness_tools::fs::{EditFileTool, FindFilesTool, ReadFileTool, SearchTool, WriteFileTool};
    use harness_tools::tasks::{BackgroundTasks, KillTaskTool, TaskOutputTool};

    let Ok(workspace) = harness_tools::Workspace::new(root) else {
        return base.clone();
    };
    let mut tools = base.clone();
    tools.register_typed(ReadFileTool::with_state(workspace.clone(), files.clone()));
    tools.register_typed(WriteFileTool::with_state(workspace.clone(), files.clone()));
    tools.register_typed(EditFileTool::with_state(workspace.clone(), files));
    tools.register_typed(FindFilesTool::new(workspace.clone()));
    tools.register_typed(SearchTool::new(workspace.clone()));
    tools.register_typed(harness_tools::git::GitTool::new(workspace.clone()));
    // A lane's background tasks are its own, so `task_output` ids resolve
    // against the commands that lane actually started — but truncated output
    // spills into the shared overflow store, so the lane's `retrieve_original`
    // (which reads that store) can still recover it.
    let tasks = BackgroundTasks::in_temp_with_overflow(base.overflow_store().cloned());
    tools.register_typed(harness_tools::shell::ShellTool::with_tasks(
        workspace,
        tasks.clone(),
    ));
    tools.register_typed(TaskOutputTool::new(tasks.clone()));
    tools.register_typed(KillTaskTool::new(tasks));
    tools
}

/// The most of one lane's patch that reaches the model. A lane that rewrote
/// half the repo is a fact worth reporting, not worth pasting.
const MAX_PATCH_CHARS: usize = 20_000;

/// The `## Inputs` block appended to a lane's prompt for the parked content
/// it was handed: each handle with its size and first lines, and how to
/// read it. The content itself is never pasted — the lane reads the slice
/// it needs.
pub(crate) fn inputs_section(
    inputs: &[String],
    store: Option<&harness_compress::CcrStore>,
) -> String {
    let mut out = String::from(
        "## Inputs\n\nThese are parked for you; read them with retrieve_original (the whole \
         thing, lines:\"a-b\", or grep:\"pattern\").\n",
    );
    for (index, input) in inputs.iter().enumerate() {
        let hash = input
            .trim()
            .trim_start_matches("<<ccr:")
            .trim_end_matches(">>")
            .split_whitespace()
            .next()
            .unwrap_or_default();
        match store.and_then(|s| s.get(hash)) {
            Some(content) => {
                let preview: Vec<String> = content
                    .lines()
                    .take(3)
                    .map(|l| harness_core::text::ellipsize(l, 120))
                    .collect();
                out.push_str(&format!(
                    "\n{}. {}\n   {}\n",
                    index + 1,
                    harness_compress::ccr::describe(hash, "input", &content),
                    preview.join("\n   ")
                ));
            }
            None => out.push_str(&format!(
                "\n{}. <<ccr:{hash}>> — nothing is parked under this hash; tell the caller.\n",
                index + 1
            )),
        }
    }
    out
}

/// What each isolated lane changed, appended to the fleet's result: a summary
/// per lane and the patch itself, so the parent can review and apply rather
/// than discovering the edits already merged. The whole patch is parked in
/// `spill` (when there is one) and its handle recorded on the lane's result.
fn patches_section(
    lanes: &[std::sync::Arc<crate::worktree::LaneWorktree>],
    results: &mut [SubagentResult],
    spill: Option<&harness_compress::CcrStore>,
) -> String {
    let mut out = String::from("\n\n## Changes from isolated agents\n\n");
    let mut any = false;
    for (index, lane) in lanes.iter().enumerate() {
        let label = results
            .get(index)
            .map(|r| r.label.clone())
            .unwrap_or_else(|| "agent".into());
        match crate::worktree::changes(lane) {
            Some(changes) => {
                any = true;
                let handle = spill.map(|store| store.put(&changes.patch));
                if let (Some(result), Some(hash)) = (results.get_mut(index), &handle) {
                    result.patch = Some(hash.clone());
                }
                let marker = match &handle {
                    Some(hash) => format!(
                        "\n… [patch truncated — the whole patch is {}]",
                        harness_compress::ccr::marker(hash, Some("full_patch"))
                    ),
                    None => "\n… [patch truncated — ask this agent for the rest, or redo the change yourself]".to_string(),
                };
                out.push_str(&format!(
                    "### {label}\n\n{}\n\n```diff\n{}\n```\n\n",
                    changes.summary,
                    harness_core::text::truncate_with_marker(
                        &changes.patch,
                        MAX_PATCH_CHARS,
                        &marker,
                    )
                    .trim_end()
                ));
            }
            None => out.push_str(&format!("### {label}\n\n(no file changes)\n\n")),
        }
    }
    if !any {
        return "\n\n(no agent changed any files)\n".to_string();
    }
    out.push_str(
        "These changes are NOT in the project — each agent worked in its own copy. Apply the \
         ones you want (write the diff to a file and `git apply` it, or make the edits \
         yourself), and say which you kept.\n",
    );
    out
}

/// One subagent the model asked for.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FleetAgentSpec {
    /// Short display name for this agent's lane, e.g. "auth-flow" (1-3 words).
    pub name: String,
    /// The complete task for this agent. It runs with the full tool set but a
    /// fresh context: it cannot see this conversation, so include everything
    /// it needs (paths, symbols, constraints, expected output format).
    pub prompt: String,
    /// Optional: make the agent's final answer a JSON object. Give the keys it
    /// must carry as `{"required": ["verdict", "files"]}`; the parsed object
    /// comes back beside its reply, and an agent that answers in prose is
    /// re-asked once or twice.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Optional parked content this agent should work from: `<<ccr:HASH>>`
    /// markers (or bare hashes) from an oversized tool result, a
    /// retrieve_original `chunks` listing, or another agent's reply. The
    /// agent is told what each is and reads it with retrieve_original; you
    /// never paste it.
    #[serde(default)]
    pub inputs: Option<Vec<String>>,
    /// Set true to start this agent from a copy of THIS conversation instead
    /// of a fresh context: it knows everything read and decided so far, so
    /// the prompt can be short ("try the other approach", "continue this
    /// investigation in src/"). Costs a full copy of the context per agent —
    /// use for work that depends on the conversation, not for reading.
    #[serde(default)]
    pub fork: Option<bool>,
}

/// Arguments for `spawn_agents`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FleetArgs {
    /// The agents to run in parallel (2-6). Give each an independent,
    /// self-contained subtask — they cannot talk to each other.
    pub agents: Vec<FleetAgentSpec>,
    /// How many agents run at once (1-6). Defaults to 3.
    #[serde(default)]
    pub max_parallel: Option<usize>,
    /// Set true when the agents will EDIT files. Each gets its own git
    /// worktree and returns a patch instead of changing the project, so
    /// parallel edits can't collide. Leave false for read-only work.
    #[serde(default)]
    pub isolate_edits: Option<bool>,
    /// Default true: wait for every agent and return their results. Set
    /// false to return at once and keep working; the results are delivered
    /// to you automatically when the fleet finishes — never poll for them.
    #[serde(default)]
    pub wait: Option<bool>,
}

/// The `spawn_agents` tool. Register it *after* snapshotting the registry into
/// the [`FleetSpawner`], so subagents get every tool except this one.
pub struct FleetTool {
    spawner: Arc<FleetSpawner>,
    sink: Arc<dyn FleetSink>,
    /// Where a `wait: false` fleet leaves its results for the agent to
    /// deliver. Without one, every fleet waits.
    asides: Option<harness_tools::Asides>,
}

impl FleetTool {
    pub fn new(spawner: Arc<FleetSpawner>, sink: Arc<dyn FleetSink>) -> Self {
        spawner.set_sink(sink.clone());
        Self {
            spawner,
            sink,
            asides: None,
        }
    }

    /// Let `wait: false` fleets hand their results to `asides` (the
    /// registry's queue, see `ToolRegistry::asides`).
    pub fn with_asides(mut self, asides: harness_tools::Asides) -> Self {
        self.asides = Some(asides);
        self
    }
}

/// A fleet's hold on the tree budget: while it lives, a root turn's reset
/// waits (see `TreeBudget::reset`). Dropped on every exit path.
struct InFlight(Arc<crate::tree::TreeBudget>);

impl InFlight {
    fn begin(tree: Arc<crate::tree::TreeBudget>) -> Self {
        tree.begin_fleet();
        Self(tree)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.end_fleet();
    }
}

/// One fleet's hold on the host's lanes display. Created around `started`,
/// it counts the fleet as live; dropping it — on every exit path, including a
/// turn future dropped mid-fleet (CLI Ctrl-C) — counts it out and tells the
/// sink *this* fleet is finished. Fleets are named, so a background fleet
/// finishing first closes its own lanes and nothing else's.
struct SinkGuard {
    sink: Arc<dyn FleetSink>,
    live: Arc<AtomicUsize>,
    fleet: String,
}

impl SinkGuard {
    fn open(
        sink: Arc<dyn FleetSink>,
        live: Arc<AtomicUsize>,
        fleet: &str,
        labels: &[String],
        cancel: CancellationToken,
    ) -> Self {
        live.fetch_add(1, Ordering::SeqCst);
        sink.started(fleet, labels, cancel);
        Self {
            sink,
            live,
            fleet: fleet.to_string(),
        }
    }
}

impl Drop for SinkGuard {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
        self.sink.finished(&self.fleet);
    }
}

#[async_trait]
impl TypedTool for FleetTool {
    const NAME: &'static str = FLEET_TOOL;

    type Args = FleetArgs;

    fn description(&self) -> &str {
        "Run several agents in parallel, each with its own prompt and a fresh context, and get \
         all their results back at once. Use this to fan independent work out — reviewing or \
         searching from several angles, exploring different parts of a codebase, drafting \
         alternative approaches — when the subtasks don't depend on each other. Each agent has \
         the full tool set but sees ONLY its own prompt (not this conversation), so make every \
         prompt self-contained: include paths, names, constraints, and the output you want back. \
         Results return labeled by agent name. Use 2-6 agents; prefer a few well-scoped agents \
         over many vague ones. Subagents cannot spawn further agents. If the agents will EDIT \
         files, set isolate_edits: each then works in its own copy of the project and returns a \
         patch for you to review and apply, instead of several agents writing over each other. \
         Every result carries an agent id: send_to_agent continues that agent with a follow-up \
         (it keeps its context), read_agent reads its full reply. Delegate reading and \
         searching so your own context stays for decisions; the agents of one turn share a \
         budget, so prefer a few substantial tasks over many tiny ones. Parked content (a \
         <<ccr:HASH>> handle from an oversized result or a retrieve_original chunks listing) \
         goes in an agent's `inputs`, never pasted into its prompt. An agent with `fork: \
         true` starts from a copy of this conversation instead — for work that depends on \
         what has been read and decided here."
    }

    /// A fleet edits, runs commands, and returns patches: it runs alone in
    /// its wave, and the permission gate treats the call as mutating.
    fn concurrency(&self) -> harness_tools::Concurrency {
        harness_tools::Concurrency::Exclusive
    }

    async fn run(&self, args: FleetArgs) -> Result<String, ToolError> {
        if args.agents.is_empty() {
            return Err(ToolError::InvalidArguments(
                "spawn_agents needs at least one agent".into(),
            ));
        }
        if args.agents.len() > MAX_FLEET_AGENTS {
            return Err(ToolError::InvalidArguments(format!(
                "spawn_agents runs at most {MAX_FLEET_AGENTS} agents per call (got {})",
                args.agents.len()
            )));
        }
        self.spawner.admit_fleet(args.agents.len() as u32)?;
        let labels: Vec<String> = args.agents.iter().map(|a| a.name.clone()).collect();
        let fleet = next_fleet_id();
        // A fleet the model doesn't wait for runs on its own task and leaves
        // its report in the registry's aside queue; the agent delivers it at
        // the next step boundary, exactly like a finished background task.
        if args.wait == Some(false) {
            if let Some(asides) = self.asides.clone() {
                let spawner = self.spawner.clone();
                let sink = self.sink.clone();
                let count = labels.len();
                let names = labels.join(", ");
                let started = format!(
                    "Fleet {fleet} started: {count} agent(s) ({names}) running in the \
                     background. Their results will be delivered to you automatically when \
                     they finish — keep working on other things, do not poll."
                );
                tokio::spawn(async move {
                    let body = match Self::execute(spawner, sink, fleet, args).await {
                        Ok(text) => text,
                        Err(e) => format!("the fleet failed: {e}"),
                    };
                    asides.push(harness_tools::Aside {
                        kind: "fleet".into(),
                        title: format!("fleet of {count} finished ({names})"),
                        body,
                    });
                });
                return Ok(started);
            }
        }
        Self::execute(self.spawner.clone(), self.sink.clone(), fleet, args).await
    }
}

impl FleetTool {
    /// Run a fleet to completion and render its combined report.
    async fn execute(
        spawner: Arc<FleetSpawner>,
        sink: Arc<dyn FleetSink>,
        fleet: String,
        args: FleetArgs,
    ) -> Result<String, ToolError> {
        let concurrency = args
            .max_parallel
            .unwrap_or(DEFAULT_FLEET_PARALLEL)
            .clamp(1, MAX_FLEET_AGENTS);

        let labels: Vec<String> = args.agents.iter().map(|a| a.name.clone()).collect();
        let forks: Vec<bool> = args
            .agents
            .iter()
            .map(|a| a.fork.unwrap_or(false))
            .collect();
        let spill = spawner.tools.overflow_store().cloned();
        let tasks: Vec<SubagentTask> = args
            .agents
            .into_iter()
            .map(|a| {
                let prompt = match &a.inputs {
                    Some(inputs) if !inputs.is_empty() => {
                        format!(
                            "{}\n\n{}",
                            a.prompt,
                            inputs_section(inputs, spill.as_deref())
                        )
                    }
                    _ => a.prompt,
                };
                SubagentTask::new(a.name, prompt).with_schema(a.output_schema)
            })
            .collect();

        // Editing lanes each get their own checkout. Falling back to the
        // shared workspace when git can't oblige keeps the fleet working, so
        // the note below says which way it went — silently sharing when the
        // model asked for isolation would be the dangerous outcome. This run
        // owns its lanes: dropping `lanes` (normal return or a cancelled,
        // dropped future) removes the worktrees, and no other fleet can reach
        // them.
        let isolated = args.isolate_edits.unwrap_or(false);
        let lanes = isolated.then(|| spawner.open_lanes(labels.len())).flatten();

        let lane_for_build = lanes.clone().unwrap_or_default();
        let (mut results, cancelled) = spawner
            .run_lanes(
                &sink,
                &fleet,
                tasks,
                FleetLimits::with_concurrency(concurrency),
                {
                    let spawner = spawner.clone();
                    let fleet = fleet.clone();
                    move |index: usize, cancel: CancellationToken| {
                        spawner.build_agent_with(
                            &labels[index],
                            &fleet,
                            lane_for_build.get(index).cloned(),
                            cancel,
                            forks[index],
                        )
                    }
                },
            )
            .await?;

        let mut out = String::new();
        let past_deadline = results
            .iter()
            .any(|r| r.stop.as_deref().is_some_and(|s| s.contains("deadline")));
        if past_deadline {
            out.push_str(
                "NOTE: the fleet ran past its deadline and was stopped; results below are \
                 partial. Smaller, better-scoped tasks finish in time.\n\n",
            );
        } else if cancelled {
            out.push_str(
                "NOTE: the fleet was stopped before finishing; results below may be partial.\n\n",
            );
        }
        if isolated && lanes.is_none() {
            out.push_str(
                "NOTE: isolation was requested but this project is not a git repository, so \
                 the agents shared one workspace and their edits are already applied (and may \
                 have collided). Check the result before trusting it.\n\n",
            );
        }
        out.push_str(&render_results(&results));
        if let Some(lanes) = &lanes {
            let spill = spawner.tools.overflow_store().cloned();
            out.push_str(&patches_section(lanes, &mut results, spill.as_deref()));
        }
        drop(lanes);
        spawner.record(&results);
        Ok(out.trim_end().to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use harness_tools::ToolRegistry;

    use super::*;
    use crate::fleet::FleetEvent;
    use crate::test_support::sse_prose;

    /// A sink that records lifecycle calls so tests can assert bracketing.
    #[derive(Default)]
    struct RecordingSink {
        calls: Mutex<Vec<String>>,
    }

    impl FleetSink for RecordingSink {
        fn started(&self, fleet: &str, labels: &[String], _cancel: CancellationToken) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("started:{fleet}:{}", labels.join(",")));
        }
        fn event(&self, _fleet: &str, event: &FleetEvent) {
            if let FleetEvent::TaskCompleted { label, ok, .. } = event {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("completed:{label}:{ok}"));
            }
        }
        fn finished(&self, fleet: &str) {
            self.calls.lock().unwrap().push(format!("finished:{fleet}"));
        }
    }

    fn spawner(url: &str) -> Arc<FleetSpawner> {
        let client = OxenClient::new(url, "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            ..AgentConfig::default()
        };
        Arc::new(FleetSpawner::new(client, ToolRegistry::new(), config))
    }

    #[test]
    fn lanes_run_under_the_shared_subagent_config() {
        // The parent's config carries everything a lane must NOT inherit
        // verbatim: an attachment list, an unbounded round budget, a smol
        // role to route to. `build_agent` and `side_agent` decide this in one
        // place (`AgentConfig::for_subagent`), so a lane can't drift from a
        // review step on any of it.
        let sp = FleetSpawner::new(
            OxenClient::new("http://localhost/api/ai", "k", "frontier"),
            ToolRegistry::new(),
            AgentConfig {
                model: "frontier".into(),
                system_prompt: None,
                initial_attachments: vec![std::path::PathBuf::from("spec.pdf")],
                round_budget: None,
                roles: crate::config::ModelRoles {
                    smol: Some("tiny".into()),
                    ..Default::default()
                },
                ..AgentConfig::default()
            },
        );
        let lane = sp
            .build_agent("lane", "fleet-t", None, CancellationToken::new())
            .unwrap();
        assert_eq!(
            lane.config().round_budget,
            Some(crate::config::RoundBudget::SUBAGENT),
            "a lane without a round budget can loop until its window fills"
        );
        assert!(
            lane.config().initial_attachments.is_empty(),
            "a lane must not re-upload the project's binary context"
        );
        assert_eq!(lane.model(), "tiny");
    }

    #[test]
    fn subagents_cannot_recurse_ask_or_chart() {
        use harness_tools::{AskUserTool, Question, QuestionAnswer, QuestionAsker, ToolError};

        struct NoopAsker;
        #[async_trait]
        impl QuestionAsker for NoopAsker {
            async fn ask(&self, _q: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
                Ok(None)
            }
        }

        // A registry carrying the session-singular tools, plus the fleet tool
        // itself — the shape a real session hands the spawner.
        let mut tools = ToolRegistry::new();
        tools.register_typed(AskUserTool::new(Arc::new(NoopAsker)));
        tools.register_typed(harness_tools::TrailTool::new());
        let sp = FleetSpawner::new(
            OxenClient::new("http://localhost/api/ai", "k", "m"),
            tools,
            AgentConfig {
                // The parent's real prompt mandates charting; a lane must not
                // inherit a mandate for a tool its registry rejects.
                system_prompt: Some(crate::prompt::default_system_prompt(false)),
                ..AgentConfig::default()
            },
        );
        let sub = sp
            .build_agent("lane", "fleet-t", None, CancellationToken::new())
            .unwrap();
        let names: Vec<_> = sub
            .tool_definitions()
            .iter()
            .filter_map(|d| d["function"]["name"].as_str().map(str::to_string))
            .collect();
        assert!(
            !names.contains(&harness_tools::ASK_USER_TOOL.to_string()),
            "a subagent must not inherit ask_user_question (it would deadlock a lane): {names:?}"
        );
        assert!(
            !names.contains(&FLEET_TOOL.to_string()),
            "a subagent must not inherit spawn_agents (no recursive fan-out): {names:?}"
        );
        assert!(
            !names.contains(&harness_tools::TRAIL_TOOL.to_string()),
            "a subagent must not inherit update_trail (its trail never reaches a board): {names:?}"
        );
        let system = sub.messages()[0].content_text().unwrap_or_default();
        assert!(
            !system.contains("update_trail"),
            "the inherited prompt must drop the trail mandate along with the tool"
        );
    }

    /// A git repository with one commit, the state `worktree add` needs.
    fn git_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .expect("git")
                .status
                .success()
        };
        assert!(run(&["init", "-q"]));
        assert!(run(&["config", "user.email", "t@example.com"]));
        assert!(run(&["config", "user.name", "T"]));
        std::fs::write(dir.path().join("shared.txt"), "original\n").unwrap();
        assert!(run(&["add", "-A"]));
        assert!(run(&["commit", "-qm", "init"]));
        dir
    }

    #[tokio::test]
    async fn isolated_lanes_write_to_their_own_checkouts() {
        let project = git_project();
        let spawner = FleetSpawner::new(
            OxenClient::new("http://localhost/api/ai", "k", "m"),
            ToolRegistry::default_for_workspace(
                harness_tools::Workspace::new(project.path()).unwrap(),
            ),
            AgentConfig {
                system_prompt: None,
                ..AgentConfig::default()
            },
        )
        .with_workspace(project.path());

        let lanes = spawner.open_lanes(2).expect("worktrees");

        // Each lane's tools are rooted in its own checkout, so the same
        // relative path is a different file for each — which is the whole
        // point: two lanes editing `shared.txt` can no longer collide.
        for (index, contents) in [(0usize, "from lane 0\n"), (1, "from lane 1\n")] {
            // Build through the spawner so the tools come out re-rooted the
            // way a real lane's do.
            let tools = rooted_tools(
                &spawner.tools,
                lanes[index].path(),
                harness_tools::FileState::gated(),
            );
            // Read first — the lane's own tools enforce that, as they should.
            tools
                .invoke(
                    harness_tools::READ_FILE_TOOL,
                    serde_json::json!({"path": "shared.txt"}),
                )
                .await
                .unwrap();
            let result = tools
                .invoke(
                    harness_tools::WRITE_FILE_TOOL,
                    serde_json::json!({"path": "shared.txt", "contents": contents}),
                )
                .await
                .unwrap();
            assert!(result.contains("wrote"), "{result}");
        }

        for (index, expected) in [(0usize, "from lane 0\n"), (1, "from lane 1\n")] {
            assert_eq!(
                std::fs::read_to_string(lanes[index].path().join("shared.txt")).unwrap(),
                expected
            );
        }
        // …and the project itself is untouched until the parent applies a patch.
        assert_eq!(
            std::fs::read_to_string(project.path().join("shared.txt")).unwrap(),
            "original\n"
        );

        // Each lane's work comes back as its own patch, whole in the overflow
        // store and recorded on the lane's result.
        let spill = harness_compress::CcrStore::default();
        let mut results: Vec<SubagentResult> = ["alpha", "beta"]
            .iter()
            .map(|label| {
                SubagentResult::from_outcome(
                    &crate::fleet::SubagentOutcome {
                        label: label.to_string(),
                        session: String::new(),
                        result: Ok("done".into()),
                        structured: None,
                        tokens_used: 1,
                        rounds: 1,
                        stopped: None,
                        denied: Vec::new(),
                    },
                    "fleet-t",
                    100,
                    None,
                )
            })
            .collect();
        let section = patches_section(&lanes, &mut results, Some(&spill));
        assert!(section.contains("### alpha"), "{section}");
        assert!(section.contains("from lane 0"), "{section}");
        assert!(section.contains("from lane 1"), "{section}");
        assert!(section.contains("NOT in the project"), "{section}");
        let patch = results[0]
            .patch
            .clone()
            .expect("the patch handle is recorded");
        assert!(spill.get(&patch).unwrap().contains("from lane 0"));
        let paths: Vec<_> = lanes.iter().map(|l| l.path().to_path_buf()).collect();
        drop(lanes);
        assert!(
            paths.iter().all(|p| !p.exists()),
            "dropping the lanes removes them"
        );
    }

    #[test]
    fn a_project_without_git_declines_isolation_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let spawner = FleetSpawner::new(
            OxenClient::new("http://localhost/api/ai", "k", "m"),
            ToolRegistry::new(),
            AgentConfig::default(),
        )
        .with_workspace(dir.path());

        // The caller falls back to a shared workspace and says so, rather than
        // failing a fleet that would have worked.
        assert!(spawner.open_lanes(2).is_none());
    }

    #[test]
    fn each_fleet_run_owns_its_own_lanes() {
        // Two fleets in flight at once (a `wait: false` fleet overlapping a
        // foreground one) open lanes independently; finishing one must leave
        // the other's checkouts standing.
        let project = git_project();
        let spawner = Arc::new(
            FleetSpawner::new(
                OxenClient::new("http://localhost/api/ai", "k", "m"),
                ToolRegistry::new(),
                AgentConfig::default(),
            )
            .with_workspace(project.path()),
        );
        let first = spawner.open_lanes(1).expect("worktree");
        let second = spawner.open_lanes(1).expect("worktree");
        let first_path = first[0].path().to_path_buf();
        let second_path = second[0].path().to_path_buf();
        assert_ne!(first_path, second_path);

        drop(first);
        assert!(!first_path.exists(), "a finished run releases its own lane");
        assert!(
            second_path.exists(),
            "…without touching the lanes of a fleet still running"
        );
        drop(second);
        assert!(!second_path.exists());
    }

    #[test]
    fn each_fleet_opens_and_closes_its_own_lanes() {
        let sink = Arc::new(RecordingSink::default());
        let live = Arc::new(AtomicUsize::new(0));
        let labels_a = vec!["a".to_string()];
        let labels_b = vec!["b".to_string()];

        let first = SinkGuard::open(
            sink.clone(),
            live.clone(),
            "fleet-x",
            &labels_a,
            CancellationToken::new(),
        );
        let second = SinkGuard::open(
            sink.clone(),
            live.clone(),
            "fleet-y",
            &labels_b,
            CancellationToken::new(),
        );
        assert_eq!(live.load(Ordering::SeqCst), 2);

        // The background fleet finishing first closes only its own lanes; the
        // fleet still running keeps its display and its live slot.
        drop(first);
        assert_eq!(live.load(Ordering::SeqCst), 1);
        assert_eq!(
            sink.calls.lock().unwrap().as_slice(),
            ["started:fleet-x:a", "started:fleet-y:b", "finished:fleet-x"]
        );
        drop(second);
        assert_eq!(live.load(Ordering::SeqCst), 0);
        assert_eq!(
            sink.calls.lock().unwrap().last().unwrap(),
            "finished:fleet-y"
        );
    }

    #[tokio::test]
    async fn lanes_persist_under_the_session_and_can_be_resumed_and_read() {
        use crate::lane_tools::{ReadAgentTool, SendToAgentTool};

        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("FIRST-TASK".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("first reply, then a second thought"))
            .expect(1)
            .create_async()
            .await;
        // The follow-up request must carry the lane's earlier exchange: that
        // is what "resumes with its context" means on the wire.
        let follow = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("FOLLOW-UP".into()),
                mockito::Matcher::Regex("first reply, then a second thought".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("follow-up reply"))
            .expect(1)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let parent = store.create_session(&SessionMeta::default()).unwrap();
        let sp = Arc::new(
            FleetSpawner::new(
                OxenClient::new(server.url(), "key", "claude-opus-4-8"),
                ToolRegistry::new(),
                AgentConfig {
                    system_prompt: None,
                    ..AgentConfig::default()
                },
            )
            .with_store(store.clone())
            .with_session(parent.clone()),
        );
        let sink: Arc<dyn FleetSink> = Arc::new(RecordingSink::default());
        let out = FleetTool::new(sp.clone(), sink.clone())
            .invoke(serde_json::json!({
                "agents": [{ "name": "scan", "prompt": "FIRST-TASK go" }]
            }))
            .await
            .unwrap();

        // The lane is a session under the parent, with its typed record.
        let lanes = store.lanes_of(&parent).unwrap();
        assert_eq!(lanes.len(), 1);
        let id = lanes[0].id.clone();
        let record = lanes[0]
            .record
            .clone()
            .expect("a finished lane has a record");
        assert_eq!(record["label"], "scan");
        assert_eq!(record["status"], "done");
        assert!(out.contains(&format!("agent id: {id}")), "{out}");
        assert_eq!(
            store.messages(&id).unwrap().len(),
            2,
            "user + assistant; the system prompt is not written per lane"
        );
        assert!(
            sp.tree().live().is_empty(),
            "a settled lane leaves the registry"
        );
        // Lane spend is the parent's spend.
        assert!(store.usage_for_session(&parent).unwrap().prompt_tokens > 0);

        // The full reply is readable, sliceable, and greppable by id — and a
        // session that isn't one of this chat's lanes is refused.
        let read = ReadAgentTool::new(sp.clone());
        let text = read
            .invoke(serde_json::json!({ "agent": id }))
            .await
            .unwrap();
        assert_eq!(text, "first reply, then a second thought");
        let hit = read
            .invoke(serde_json::json!({ "agent": id, "grep": "SECOND" }))
            .await
            .unwrap();
        assert_eq!(hit, "1: first reply, then a second thought");
        let sliced = read
            .invoke(serde_json::json!({ "agent": id, "lines": "2-" }))
            .await
            .unwrap();
        assert_eq!(sliced, "");
        assert!(read
            .invoke(serde_json::json!({ "agent": parent }))
            .await
            .is_err());

        // A lane still running is not resumed on top of itself.
        sp.tree().register(crate::lane::LiveLane {
            id: id.clone(),
            label: "scan".into(),
            fleet: "fleet-busy".into(),
            started: std::time::Instant::now(),
            cancel: CancellationToken::new(),
            steer: crate::Interjections::default(),
        });
        let err = SendToAgentTool::new(sp.clone(), sink.clone())
            .invoke(serde_json::json!({ "agent": id, "message": "FOLLOW-UP please" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("still running"), "{err}");
        sp.tree().finish_fleet("fleet-busy");

        // A follow-up resumes the same lane with its history.
        let out = SendToAgentTool::new(sp.clone(), sink)
            .invoke(serde_json::json!({ "agent": id, "message": "FOLLOW-UP please" }))
            .await
            .unwrap();
        assert!(out.contains("follow-up reply"), "{out}");
        assert!(out.contains("### scan — done"), "{out}");
        assert_eq!(store.messages(&id).unwrap().len(), 4);
        assert_eq!(
            store.lanes_of(&parent).unwrap().len(),
            1,
            "same lane, not a new one"
        );
        first.assert_async().await;
        follow.assert_async().await;
    }

    #[test]
    fn a_lane_below_the_depth_cap_can_spawn_and_a_leaf_cannot() {
        let sp = Arc::new(FleetSpawner::new(
            OxenClient::new("http://localhost/api/ai", "k", "m"),
            ToolRegistry::new(),
            AgentConfig {
                system_prompt: Some("base prompt".into()),
                max_depth: 2,
                ..AgentConfig::default()
            },
        ));
        // Building the tool is what tells the spawner where lanes render;
        // without a display a lane could not show its own fleet.
        let _tool = FleetTool::new(sp.clone(), Arc::new(RecordingSink::default()));
        let names = |agent: &Agent| -> Vec<String> {
            agent
                .tool_definitions()
                .iter()
                .filter_map(|d| d["function"]["name"].as_str().map(str::to_string))
                .collect()
        };

        let lane = sp
            .build_agent("lane", "fleet-t", None, CancellationToken::new())
            .unwrap();
        assert_eq!(lane.config().depth, 1);
        for tool in [
            FLEET_TOOL,
            crate::lane_tools::SEND_TO_AGENT_TOOL,
            crate::lane_tools::READ_AGENT_TOOL,
        ] {
            assert!(
                names(&lane).contains(&tool.to_string()),
                "a lane may {tool}"
            );
        }
        let prompt = lane.messages()[0].content_text().unwrap_or_default();
        assert!(prompt.starts_with("base prompt"));
        assert!(prompt.contains("You may spawn agents of your own"));

        let child = sp.child(lane.config(), "lane-session", &CancellationToken::new());
        let leaf = child
            .build_agent("leaf", "fleet-u", None, CancellationToken::new())
            .unwrap();
        assert_eq!(leaf.config().depth, 2);
        assert!(!leaf.config().may_spawn());
        for tool in [
            FLEET_TOOL,
            crate::lane_tools::SEND_TO_AGENT_TOOL,
            crate::lane_tools::READ_AGENT_TOOL,
        ] {
            assert!(
                !names(&leaf).contains(&tool.to_string()),
                "a leaf may not {tool}"
            );
        }
        let prompt = leaf.messages()[0].content_text().unwrap_or_default();
        assert!(prompt.contains("there are no further agents to delegate to"));
        // Both share the turn's wallet.
        assert!(Arc::ptr_eq(&sp.tree_budget(), &child.tree_budget()));
        assert!(Arc::ptr_eq(
            &sp.tree_budget(),
            leaf.config().tree.as_ref().unwrap()
        ));
    }

    #[tokio::test]
    async fn the_tree_budget_refuses_lanes_past_its_spawn_cap() {
        let sp = Arc::new(FleetSpawner::new(
            OxenClient::new("http://127.0.0.1:1/api/ai", "k", "m"),
            ToolRegistry::new(),
            AgentConfig {
                tree: Some(Arc::new(crate::tree::TreeBudget::new(
                    crate::tree::TreeLimits {
                        max_spawns: 2,
                        ..Default::default()
                    },
                ))),
                ..AgentConfig::default()
            },
        ));
        let tool = FleetTool::new(sp, Arc::new(RecordingSink::default()));
        let err = tool
            .invoke(serde_json::json!({
                "agents": [
                    { "name": "a", "prompt": "go" },
                    { "name": "b", "prompt": "go" },
                    { "name": "c", "prompt": "go" }
                ]
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("allows 2 agents"), "{err}");
    }

    #[tokio::test]
    async fn a_lane_stops_with_what_it_has_when_the_tree_wallet_is_spent() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("first lane's answer"))
            .expect(1)
            .create_async()
            .await;
        // A one-token wallet: the first lane's single call spends it, so the
        // second lane (one slot, so it runs after) stops before calling.
        let sp = Arc::new(FleetSpawner::new(
            OxenClient::new(server.url(), "k", "claude-opus-4-8"),
            ToolRegistry::new(),
            AgentConfig {
                system_prompt: None,
                tree: Some(Arc::new(crate::tree::TreeBudget::new(
                    crate::tree::TreeLimits {
                        max_tokens: 1,
                        ..Default::default()
                    },
                ))),
                ..AgentConfig::default()
            },
        ));
        let out = FleetTool::new(sp.clone(), Arc::new(RecordingSink::default()))
            .invoke(serde_json::json!({
                "agents": [
                    { "name": "first", "prompt": "go" },
                    { "name": "second", "prompt": "go" }
                ],
                "max_parallel": 1
            }))
            .await
            .unwrap();
        assert!(out.contains("### first — done"), "{out}");
        assert!(out.contains("first lane's answer"), "{out}");
        assert!(
            out.contains(
                "### second — partial — stopped early (the agents' shared budget is spent)"
            ),
            "{out}"
        );
        assert!(sp.tree_budget().usage().tokens > 0);
    }

    #[tokio::test]
    async fn a_lane_can_spawn_its_own_fleet_one_level_down() {
        use crate::test_support::sse_tool_call;

        let mut server = mockito::Server::new_async().await;
        // Mockito serves the first matching mock that hasn't met its expected
        // count, so the three calls are told apart by what their bodies carry.
        let outer_first = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("OUTER-TASK".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_tool_call(
                "call_inner",
                FLEET_TOOL,
                serde_json::json!({ "agents": [{ "name": "inner", "prompt": "INNER-TASK go" }] }),
            ))
            .expect(1)
            .create_async()
            .await;
        let inner = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("INNER-TASK".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("inner answer 42"))
            .expect(1)
            .create_async()
            .await;
        let outer_second = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("inner answer 42".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("outer done with 42"))
            .expect(1)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let parent = store.create_session(&SessionMeta::default()).unwrap();
        let sp = Arc::new(
            FleetSpawner::new(
                OxenClient::new(server.url(), "k", "claude-opus-4-8"),
                ToolRegistry::new(),
                AgentConfig {
                    system_prompt: None,
                    ..AgentConfig::default()
                },
            )
            .with_store(store.clone())
            .with_session(parent.clone()),
        );
        let sink = Arc::new(RecordingSink::default());
        let out = FleetTool::new(sp.clone(), sink.clone())
            .invoke(serde_json::json!({
                "agents": [{ "name": "outer", "prompt": "OUTER-TASK go" }]
            }))
            .await
            .unwrap();
        outer_first.assert_async().await;
        inner.assert_async().await;
        outer_second.assert_async().await;

        assert!(out.contains("outer done with 42"), "{out}");
        // The outer lane lives under the parent; the inner one under the outer.
        let outer_lanes = store.lanes_of(&parent).unwrap();
        assert_eq!(outer_lanes.len(), 1);
        let inner_lanes = store.lanes_of(&outer_lanes[0].id).unwrap();
        assert_eq!(inner_lanes.len(), 1);
        assert_eq!(inner_lanes[0].record.as_ref().unwrap()["label"], "inner");
        assert_eq!(
            sp.tree_budget().usage().spawns,
            2,
            "both levels spend the one wallet"
        );
        // Both fleets rendered on the one display, each under its own id.
        let calls = sink.calls.lock().unwrap();
        assert_eq!(
            calls.iter().filter(|c| c.starts_with("started:")).count(),
            2
        );
        assert!(sp.tree().live().is_empty());
    }

    #[test]
    fn a_lane_is_told_about_its_inputs_without_being_handed_them() {
        let store = harness_compress::CcrStore::default();
        let content = (1..=200)
            .map(|n| format!("row {n}: SECRET-PAYLOAD"))
            .collect::<Vec<_>>()
            .join("\n");
        let hash = store.put(&content);
        let section = inputs_section(
            &[format!("<<ccr:{hash} full_output>>"), "deadbeef0000".into()],
            Some(&store),
        );
        assert!(section.starts_with("## Inputs"), "{section}");
        assert!(
            section.contains(&format!("<<ccr:{hash} input>> (200 lines,")),
            "{section}"
        );
        assert!(section.contains("row 1: SECRET-PAYLOAD"), "a short preview");
        assert!(
            !section.contains("row 4: SECRET-PAYLOAD"),
            "not the content: {section}"
        );
        assert!(
            section.contains("<<ccr:deadbeef0000>> — nothing is parked"),
            "{section}"
        );
        assert!(section.chars().count() < 800, "{}", section.chars().count());
    }

    #[tokio::test]
    async fn a_forked_lane_starts_from_the_parents_conversation() {
        let mut server = mockito::Server::new_async().await;
        // The lane's request carries what the parent already discussed.
        let forked = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("PARENT-CONTEXT-MARKER".into()),
                mockito::Matcher::Regex("try the other approach".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("continued from where you were"))
            .expect(1)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let parent = store.create_session(&SessionMeta::default()).unwrap();
        let sp = Arc::new(
            FleetSpawner::new(
                OxenClient::new(server.url(), "k", "claude-opus-4-8"),
                ToolRegistry::new(),
                AgentConfig {
                    system_prompt: Some(crate::prompt::default_system_prompt(false)),
                    ..AgentConfig::default()
                },
            )
            .with_store(store.clone())
            .with_session(parent.clone()),
        );
        let tool = FleetTool::new(sp.clone(), Arc::new(RecordingSink::default()));

        // Without a published conversation a fork is refused, not silently fresh.
        let err = tool
            .invoke(serde_json::json!({
                "agents": [{ "name": "twin", "prompt": "try the other approach", "fork": true }]
            }))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("fork: true needs a conversation"),
            "{err}"
        );

        // The parent publishes its transcript as the call runs (the agent
        // does this itself right before a spawn_agents wave).
        let mut parent_agent = Agent::new(
            OxenClient::new(server.url(), "k", "claude-opus-4-8"),
            ToolRegistry::new(),
            store.clone(),
            parent.clone(),
            AgentConfig {
                system_prompt: Some(crate::prompt::default_system_prompt(false)),
                ..AgentConfig::default()
            },
        )
        .unwrap();
        parent_agent.set_fork_slot(sp.fork_slot());
        parent_agent
            .inject_exchange("we settled on PARENT-CONTEXT-MARKER", "noted")
            .unwrap();
        parent_agent.publish_fork_source(&[harness_llm::types::ToolCall {
            id: "c".into(),
            kind: "function".into(),
            function: harness_llm::types::FunctionCall {
                name: FLEET_TOOL.into(),
                arguments: r#"{"agents":[],"fork":true}"#.into(),
            },
        }]);

        let out = tool
            .invoke(serde_json::json!({
                "agents": [{ "name": "twin", "prompt": "try the other approach", "fork": true }]
            }))
            .await
            .unwrap();
        forked.assert_async().await;
        assert!(out.contains("continued from where you were"), "{out}");

        // The fork starts from one snapshot of the parent's transcript (not a
        // row per message), with a lane's system prompt; its own turn is the
        // only thing written as rows.
        let lanes = store.lanes_of(&parent).unwrap();
        assert_eq!(lanes.len(), 1);
        let (_, inherited) = store
            .context_snapshot::<Vec<harness_llm::types::ChatMessage>>(&lanes[0].id)
            .unwrap()
            .expect("the inherited context is one snapshot");
        let system = inherited[0].content_text().unwrap_or_default();
        assert!(
            !system.contains("update_trail"),
            "no trail mandate in a lane"
        );
        assert!(system.contains("You are a subagent"));
        assert_eq!(
            store.messages(&lanes[0].id).unwrap().len(),
            2,
            "its own user + reply"
        );
        assert!(inherited.iter().any(|m| m
            .content_text()
            .is_some_and(|c| c.contains("PARENT-CONTEXT-MARKER"))));
    }

    #[tokio::test]
    async fn a_session_cannot_pile_up_fleets_without_bound() {
        let sink = Arc::new(RecordingSink::default());
        let tool = FleetTool::new(spawner("http://127.0.0.1:1/api/ai"), sink);
        // Three fleets already in flight (as `wait: false` calls leave them).
        tool.spawner.live.store(MAX_LIVE_FLEETS, Ordering::SeqCst);
        let err = tool
            .invoke(serde_json::json!({
                "agents": [{ "name": "a", "prompt": "go" }],
                "wait": false
            }))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("already running"),
            "a fourth fleet is refused, not queued: {err}"
        );
        assert_eq!(tool.spawner.live.load(Ordering::SeqCst), MAX_LIVE_FLEETS);
    }

    #[tokio::test]
    async fn a_model_switch_reaches_future_subagents() {
        // Point the spawner at a fresh model; the next subagent must request it.
        let mut server = mockito::Server::new_async().await;
        let hit = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex(
                "\"model\":\"swapped-model\"".into(),
            ))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("ok"))
            .expect(1)
            .create_async()
            .await;
        let sp = spawner(&server.url());
        sp.set_model("swapped-model");
        let tool = FleetTool::new(sp, Arc::new(RecordingSink::default()));
        tool.invoke(serde_json::json!({ "agents": [{ "name": "a", "prompt": "go" }] }))
            .await
            .unwrap();
        hit.assert_async().await;
    }

    #[tokio::test]
    async fn spawn_agents_runs_the_fleet_and_labels_the_results() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("LOOK-LEFT".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("left says hi"))
            .create_async()
            .await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("LOOK-RIGHT".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("right says hi"))
            .create_async()
            .await;

        let sink = Arc::new(RecordingSink::default());
        let tool = FleetTool::new(spawner(&server.url()), sink.clone());
        let out = tool
            .invoke(serde_json::json!({
                "agents": [
                    { "name": "left", "prompt": "LOOK-LEFT please" },
                    { "name": "right", "prompt": "LOOK-RIGHT please" },
                ]
            }))
            .await
            .unwrap();

        assert!(out.contains("### left — done"), "{out}");
        assert!(out.contains("left says hi"), "{out}");
        assert!(out.contains("### right — done"), "{out}");
        assert!(out.contains("right says hi"), "{out}");

        // The sink saw the full bracket: started → completions → finished,
        // every call naming the same fleet.
        let calls = sink.calls.lock().unwrap();
        let fleet = calls
            .first()
            .and_then(|c| c.strip_prefix("started:"))
            .and_then(|rest| rest.split(':').next())
            .expect("a started call naming the fleet");
        assert!(fleet.starts_with("fleet-"), "{fleet}");
        assert_eq!(
            calls.first().unwrap(),
            &format!("started:{fleet}:left,right")
        );
        assert_eq!(calls.last().unwrap(), &format!("finished:{fleet}"));
        assert!(calls.contains(&"completed:left:true".to_string()));
        assert!(calls.contains(&"completed:right:true".to_string()));
    }

    #[tokio::test]
    async fn a_fleet_the_model_does_not_wait_for_reports_through_the_aside_queue() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("scout says hi"))
            .create_async()
            .await;
        let asides = harness_tools::Asides::default();
        let sink = Arc::new(RecordingSink::default());
        let tool = FleetTool::new(spawner(&server.url()), sink.clone()).with_asides(asides.clone());
        let out = tool
            .invoke(serde_json::json!({
                "agents": [{ "name": "scout", "prompt": "look" }],
                "wait": false
            }))
            .await
            .unwrap();
        assert!(out.contains("delivered to you automatically"), "{out}");
        // The report lands in the queue once the fleet finishes.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while asides.is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let delivered = asides.take_all();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].kind, "fleet");
        assert!(
            delivered[0].title.contains("scout"),
            "{}",
            delivered[0].title
        );
        assert!(
            delivered[0].body.contains("scout says hi"),
            "{}",
            delivered[0].body
        );
        // Without an aside queue, `wait: false` still waits.
        let waiting = FleetTool::new(spawner(&server.url()), sink);
        let out = waiting
            .invoke(serde_json::json!({
                "agents": [{ "name": "scout", "prompt": "look" }],
                "wait": false
            }))
            .await
            .unwrap();
        assert!(out.contains("scout says hi"), "{out}");
    }

    #[tokio::test]
    async fn spawn_agents_validates_its_arguments() {
        let sink = Arc::new(RecordingSink::default());
        let tool = FleetTool::new(spawner("http://127.0.0.1:1/api/ai"), sink.clone());

        let err = tool
            .invoke(serde_json::json!({ "agents": [] }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("at least one agent"));

        let too_many: Vec<_> = (0..7)
            .map(|i| serde_json::json!({ "name": format!("a{i}"), "prompt": "p" }))
            .collect();
        let err = tool
            .invoke(serde_json::json!({ "agents": too_many }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("at most 6"));
        // Rejected calls never touched the display.
        assert!(sink.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelling_the_turn_token_stops_the_fleet_and_notes_partial_results() {
        let sink = Arc::new(RecordingSink::default());
        let sp = spawner("http://127.0.0.1:1/api/ai");
        let turn_token = CancellationToken::new();
        sp.set_cancel(turn_token.clone());
        turn_token.cancel(); // the turn is already stopping

        let tool = FleetTool::new(sp, sink.clone());
        let out = tool
            .invoke(serde_json::json!({
                "agents": [{ "name": "a", "prompt": "go" }]
            }))
            .await
            .unwrap();
        assert!(out.contains("stopped before finishing"));
        assert!(sink
            .calls
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .starts_with("finished:fleet-"));
    }
}
