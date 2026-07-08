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

/// Builds the detached agents a fleet runs on: the session agent's client,
/// tools, and config, captured at registry build time. The registry snapshot
/// is taken *before* `spawn_agents` registers, so subagents can't recurse.
pub struct FleetSpawner {
    client: OxenClient,
    tools: ToolRegistry,
    config: AgentConfig,
    /// The current turn's stop signal; hosts refresh it when they install a
    /// turn's token so cancelling the turn cancels any running fleet too.
    cancel: StdMutex<CancellationToken>,
}

impl FleetSpawner {
    pub fn new(client: OxenClient, tools: ToolRegistry, config: AgentConfig) -> Self {
        Self {
            client,
            tools,
            config,
            cancel: StdMutex::new(CancellationToken::new()),
        }
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

    /// One detached subagent: in-memory store, shared client/tools/config.
    fn build_agent(&self) -> Result<Agent, AgentError> {
        let store = Arc::new(HistoryStore::open_in_memory()?);
        let session = store.create_session(&SessionMeta {
            model: self.config.model.clone(),
            ..Default::default()
        })?;
        Agent::new(
            self.client.clone(),
            self.tools.clone(),
            store,
            session,
            self.config.clone(),
        )
    }
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
         over many vague ones. Subagents cannot spawn further agents."
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

        let cancel = self.spawner.run_token();
        self.sink.started(&labels, cancel.clone());
        let guard = SinkGuard(self.sink.clone());

        let sink = self.sink.clone();
        let spawner = self.spawner.clone();
        let outcomes = run_fleet(
            move || spawner.build_agent(),
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
        for outcome in outcomes {
            match &outcome.result {
                Ok(text) => {
                    out.push_str(&format!("### {}\n\n{}\n\n", outcome.label, text.trim()));
                }
                Err(e) => {
                    out.push_str(&format!(
                        "### {}\n\n(this agent failed: {e})\n\n",
                        outcome.label
                    ));
                }
            }
        }
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
