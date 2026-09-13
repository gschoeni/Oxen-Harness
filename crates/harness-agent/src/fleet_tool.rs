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
use crate::fleet::{run_fleet, FleetLimits, FleetSink, SpawnAgent, SubagentTask};
use crate::lane::{lane_budget, render_results, AgentTree, LiveLane, SubagentResult};

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
}

/// The mutable half of a [`FleetSpawner`]: what subagents inherit that can
/// change over a session's life.
struct Endpoint {
    client: OxenClient,
    config: AgentConfig,
}

impl FleetSpawner {
    pub fn new(client: OxenClient, tools: ToolRegistry, config: AgentConfig) -> Self {
        Self {
            tools,
            workspace_root: StdMutex::new(None),
            endpoint: StdMutex::new(Endpoint { client, config }),
            cancel: StdMutex::new(CancellationToken::new()),
            store: None,
            session: StdMutex::new(None),
            tree: Arc::default(),
            live: Arc::default(),
        }
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

    /// Refuse a new fleet when the session already has [`MAX_LIVE_FLEETS`]
    /// in flight.
    pub(crate) fn admit_fleet(&self) -> Result<(), ToolError> {
        if self.live.load(Ordering::SeqCst) >= MAX_LIVE_FLEETS {
            return Err(ToolError::Execution(format!(
                "{MAX_LIVE_FLEETS} fleets are already running in this session; their results \
                 arrive automatically — wait for them before starting another"
            )));
        }
        Ok(())
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
        let session = store.create_session(&meta)?;
        let mut agent = Agent::new(
            client,
            crate::agent::subagent_tools(tools),
            store,
            session,
            config,
        )?;
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
        let mut agent = Agent::resume_from_store(
            client,
            crate::agent::subagent_tools(tools),
            store,
            id.to_string(),
            config,
        )?;
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
        let guard = SinkGuard::open(
            sink.clone(),
            self.live.clone(),
            fleet,
            &labels,
            cancel.clone(),
        );
        let outcomes = run_fleet(spawn, tasks, limits, cancel.clone(), |event| {
            sink.event(fleet, event)
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
        let results = outcomes
            .iter()
            .map(|o| SubagentResult::from_outcome(o, fleet, budget, spill.map(Arc::as_ref)))
            .collect();
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
         (it keeps its context), read_agent reads its full reply."
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
        self.spawner.admit_fleet()?;
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
        let tasks: Vec<SubagentTask> = args
            .agents
            .into_iter()
            .map(|a| SubagentTask::new(a.name, a.prompt).with_schema(a.output_schema))
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
                        spawner.build_agent(
                            &labels[index],
                            &fleet,
                            lane_for_build.get(index).cloned(),
                            cancel,
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
        assert_eq!(store.messages(&id).unwrap().len(), 2, "user + assistant");
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
