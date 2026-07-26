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
use crate::fleet::{run_fleet, FleetSink, SubagentTask};

/// Stable identifier the model uses to call the fleet tool.
pub const FLEET_TOOL: &str = "spawn_agents";

/// Most agents one call may spawn (also stated in the model-facing schema).
pub const MAX_FLEET_AGENTS: usize = 6;

/// Default (and ceiling) for how many subagents run at once.
pub const DEFAULT_FLEET_PARALLEL: usize = 3;

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
    /// Per-lane checkouts for the fleet currently running, indexed by lane.
    /// Installed by the tool before `run_fleet` and cleared after.
    lanes: StdMutex<Vec<std::sync::Arc<crate::worktree::LaneWorktree>>>,
    /// The client + config subagents are built from, updated in lockstep with
    /// the live agent via [`FleetSpawner::set_client`] / [`set_model`].
    endpoint: StdMutex<Endpoint>,
    /// The current turn's stop signal; hosts refresh it when they install a
    /// turn's token so cancelling the turn cancels any running fleet too.
    cancel: StdMutex<CancellationToken>,
    /// Optional persistent aggregate ledger. Fleet transcripts remain in
    /// memory, but their provider usage belongs in the host's all-time totals.
    usage_store: Option<Arc<HistoryStore>>,
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
            lanes: StdMutex::new(Vec::new()),
            endpoint: StdMutex::new(Endpoint { client, config }),
            cancel: StdMutex::new(CancellationToken::new()),
            usage_store: None,
        }
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
    /// git repository (isolation is an upgrade, not a precondition).
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
        *self.lanes.lock().expect("fleet lanes poisoned") = lanes.clone();
        Some(lanes)
    }

    /// The project root, when the host told us about one.
    fn root(&self) -> Option<std::path::PathBuf> {
        self.workspace_root
            .lock()
            .expect("fleet workspace poisoned")
            .clone()
    }

    /// Release the current fleet's worktrees (removing any the caller didn't
    /// keep a handle to).
    fn close_lanes(&self) {
        self.lanes.lock().expect("fleet lanes poisoned").clear();
    }

    /// Attach the host's persistent usage ledger to future fleet agents.
    pub fn with_usage_store(mut self, store: Arc<HistoryStore>) -> Self {
        self.usage_store = Some(store);
        self
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

    /// One detached subagent: in-memory store, the current client/config, and
    /// the subagent-narrowed tool set (no recursion, no interactive tools).
    fn build_agent(&self, index: usize) -> Result<Agent, AgentError> {
        let (client, mut config) = {
            let endpoint = self.endpoint.lock().expect("fleet endpoint poisoned");
            (endpoint.client.clone(), endpoint.config.clone())
        };
        // Lanes can't share the host's single approval prompt (the same
        // deadlock reasoning as `subagent_tools` stripping `ask_user_question`):
        // their gate auto-denies and tells the lane to report the command back.
        config.permissions = config.permissions.map(|gate| Arc::new(gate.for_subagent()));
        // Lanes read and grep far more than they write, and there are N of
        // them: route them to the `smol` role when the user configured one.
        config.model = config
            .roles
            .resolve(crate::config::Role::Smol, &config.model)
            .to_string();
        // An isolated lane works in its own checkout, so its file and shell
        // tools must point there rather than at the shared project.
        let lane = self
            .lanes
            .lock()
            .expect("fleet lanes poisoned")
            .get(index)
            .cloned();
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
        let store = Arc::new(HistoryStore::open_in_memory()?);
        let session = store.create_session(&SessionMeta {
            model: config.model.clone(),
            ..Default::default()
        })?;
        let mut agent = Agent::new(
            client,
            crate::agent::subagent_tools(tools),
            store,
            session,
            config,
        )?;
        agent.disable_transcript_persistence();
        if let Some(usage_store) = &self.usage_store {
            agent.set_usage_store(usage_store.clone());
        }
        Ok(agent)
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
    // against the commands that lane actually started.
    let tasks = BackgroundTasks::in_temp();
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
/// than discovering the edits already merged.
fn patches_section(
    lanes: &[std::sync::Arc<crate::worktree::LaneWorktree>],
    labels: &[String],
) -> String {
    let mut out = String::from("\n\n## Changes from isolated agents\n\n");
    let mut any = false;
    for (index, lane) in lanes.iter().enumerate() {
        let label = labels.get(index).map(String::as_str).unwrap_or("agent");
        match crate::worktree::changes(lane) {
            Some(changes) => {
                any = true;
                out.push_str(&format!("### {label}\n\n{}\n\n```diff\n{}\n```\n\n", changes.summary,
                    harness_core::text::truncate_with_marker(
                        &changes.patch,
                        MAX_PATCH_CHARS,
                        "\n… [patch truncated — ask this agent for the rest, or redo the change yourself]",
                    ).trim_end()));
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
}

/// The `spawn_agents` tool. Register it *after* snapshotting the registry into
/// the [`FleetSpawner`], so subagents get every tool except this one.
pub struct FleetTool {
    spawner: Arc<FleetSpawner>,
    sink: Arc<dyn FleetSink>,
}

impl FleetTool {
    pub fn new(spawner: Arc<FleetSpawner>, sink: Arc<dyn FleetSink>) -> Self {
        Self { spawner, sink }
    }
}

/// Calls `finished` exactly once — on drop — so the host's lanes display is
/// torn down even when the turn's future is dropped mid-fleet (CLI Ctrl-C).
struct SinkGuard(Arc<dyn FleetSink>);

impl Drop for SinkGuard {
    fn drop(&mut self) {
        self.0.finished();
    }
}

/// Releases the spawner's owning references to isolated lane worktrees on
/// every exit path, including cancellation that drops the tool future.
struct LaneGuard(Arc<FleetSpawner>);

impl Drop for LaneGuard {
    fn drop(&mut self) {
        self.0.close_lanes();
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
         patch for you to review and apply, instead of several agents writing over each other."
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
        let concurrency = args
            .max_parallel
            .unwrap_or(DEFAULT_FLEET_PARALLEL)
            .clamp(1, MAX_FLEET_AGENTS);

        let labels: Vec<String> = args.agents.iter().map(|a| a.name.clone()).collect();
        let tasks: Vec<SubagentTask> = args
            .agents
            .into_iter()
            .map(|a| SubagentTask::new(a.name, a.prompt))
            .collect();

        // Editing lanes each get their own checkout. Falling back to the
        // shared workspace when git can't oblige keeps the fleet working, so
        // the note below says which way it went — silently sharing when the
        // model asked for isolation would be the dangerous outcome.
        let isolated = args.isolate_edits.unwrap_or(false);
        let lanes = isolated
            .then(|| self.spawner.open_lanes(labels.len()))
            .flatten();
        let lane_guard = lanes.as_ref().map(|_| LaneGuard(self.spawner.clone()));

        let cancel = self.spawner.run_token();
        self.sink.started(&labels, cancel.clone());
        let guard = SinkGuard(self.sink.clone());

        let sink = self.sink.clone();
        let spawner = self.spawner.clone();
        let outcomes = run_fleet(
            move |index| spawner.build_agent(index),
            tasks,
            concurrency,
            cancel.clone(),
            |event| sink.event(event),
        )
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;
        drop(guard); // normal teardown; the guard covers the abnormal paths

        let mut out = String::new();
        if cancel.is_cancelled() {
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
        out.push_str(&crate::fleet::combine_outcomes(&outcomes, "agent"));
        if let Some(lanes) = &lanes {
            out.push_str(&patches_section(lanes, &labels));
        }
        drop(lane_guard);
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
        fn started(&self, labels: &[String], _cancel: CancellationToken) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("started:{}", labels.join(",")));
        }
        fn event(&self, event: &FleetEvent) {
            if let FleetEvent::TaskCompleted { label, ok, .. } = event {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("completed:{label}:{ok}"));
            }
        }
        fn finished(&self) {
            self.calls.lock().unwrap().push("finished".into());
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
    fn subagents_cannot_recurse_or_ask() {
        use harness_tools::{AskUserTool, Question, QuestionAnswer, QuestionAsker, ToolError};

        struct NoopAsker;
        #[async_trait]
        impl QuestionAsker for NoopAsker {
            async fn ask(&self, _q: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
                Ok(None)
            }
        }

        // A registry carrying both host-owned singular tools, plus the fleet
        // tool itself — the shape a real session hands the spawner.
        let mut tools = ToolRegistry::new();
        tools.register_typed(AskUserTool::new(Arc::new(NoopAsker)));
        let sp = FleetSpawner::new(
            OxenClient::new("http://localhost/api/ai", "k", "m"),
            tools,
            AgentConfig {
                system_prompt: None,
                ..AgentConfig::default()
            },
        );
        let sub = sp.build_agent(0).unwrap();
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

        // Each lane's work comes back as its own patch.
        let section = patches_section(&lanes, &["alpha".into(), "beta".into()]);
        assert!(section.contains("### alpha"), "{section}");
        assert!(section.contains("from lane 0"), "{section}");
        assert!(section.contains("from lane 1"), "{section}");
        assert!(section.contains("NOT in the project"), "{section}");
        spawner.close_lanes();
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
    fn lane_guard_releases_worktrees_when_a_run_is_dropped() {
        let project = git_project();
        let spawner = Arc::new(
            FleetSpawner::new(
                OxenClient::new("http://localhost/api/ai", "k", "m"),
                ToolRegistry::new(),
                AgentConfig::default(),
            )
            .with_workspace(project.path()),
        );
        let lanes = spawner.open_lanes(1).expect("worktree");
        let path = lanes[0].path().to_path_buf();
        let guard = LaneGuard(spawner.clone());

        drop(lanes);
        assert!(
            path.exists(),
            "the spawner still owns the lane until the run guard drops"
        );
        drop(guard);

        assert!(!path.exists(), "dropping the run must release its lane");
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

        assert!(out.contains("### left\n\nleft says hi"));
        assert!(out.contains("### right\n\nright says hi"));

        // The sink saw the full bracket: started → completions → finished.
        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.first().unwrap(), "started:left,right");
        assert_eq!(calls.last().unwrap(), "finished");
        assert!(calls.contains(&"completed:left:true".to_string()));
        assert!(calls.contains(&"completed:right:true".to_string()));
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
        assert_eq!(sink.calls.lock().unwrap().last().unwrap(), "finished");
    }
}
