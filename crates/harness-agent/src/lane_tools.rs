//! `send_to_agent` and `read_agent` — the model's handles on lanes it has
//! already run. Both take the agent id a `spawn_agents` result carries (the
//! lane's session id).
//!
//! - `send_to_agent` brings a finished lane back with its whole transcript
//!   and runs one more turn on it: a follow-up question, a correction, "now
//!   do the same for the other module". Rendered on the host's lanes display
//!   like any fleet.
//! - `read_agent` reads a lane's full final reply (the parent only saw it
//!   within the fleet's cap), optionally a line range or a grep of it.

use std::sync::Arc;

use async_trait::async_trait;
use harness_tools::{CallContext, Concurrency, ToolError, TypedTool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::fleet::{FleetLimits, FleetSink, SubagentTask, LANE_RESULT_CHARS};
use crate::fleet_tool::{next_fleet_id, FleetSpawner};
use crate::lane::{render_results, SubagentResult};

pub const SEND_TO_AGENT_TOOL: &str = "send_to_agent";
pub const READ_AGENT_TOOL: &str = "read_agent";

/// Arguments for `send_to_agent`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendToAgentArgs {
    /// The agent id from an earlier spawn_agents result.
    pub agent: String,
    /// The follow-up. The agent keeps everything it read and did before, so
    /// refer to that freely; state what you want back.
    pub message: String,
    /// Optional: make the reply a JSON object with these `required` keys.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
}

/// Continue a finished lane with one more turn.
pub struct SendToAgentTool {
    spawner: Arc<FleetSpawner>,
    sink: Arc<dyn FleetSink>,
}

impl SendToAgentTool {
    pub fn new(spawner: Arc<FleetSpawner>, sink: Arc<dyn FleetSink>) -> Self {
        Self { spawner, sink }
    }
}

#[async_trait]
impl TypedTool for SendToAgentTool {
    const NAME: &'static str = SEND_TO_AGENT_TOOL;

    type Args = SendToAgentArgs;

    fn description(&self) -> &str {
        "Send a follow-up to an agent you spawned earlier (spawn_agents gives each result an \
         agent id). The agent resumes with its full context — everything it read, ran, and \
         concluded — so use it to ask for more detail, correct its course, or extend its task, \
         instead of re-explaining from scratch to a new agent. Returns its reply like a \
         spawn_agents result."
    }

    /// Resumes an agent that may edit and run commands: alone in its wave,
    /// mutating to the gate.
    fn concurrency(&self) -> Concurrency {
        Concurrency::Exclusive
    }

    async fn run(&self, args: SendToAgentArgs, _call: &CallContext) -> Result<String, ToolError> {
        let _admission = self.spawner.admit_fleet(1)?;
        let id = args.agent.trim().to_string();
        let label = self
            .spawner
            .lane_record(&id)
            .map(|r| r.label)
            .unwrap_or_else(|| "agent".into());
        let fleet = next_fleet_id();
        let (results, cancelled) =
            self.spawner
                .run_lanes(
                    &self.sink,
                    &fleet,
                    vec![SubagentTask::new(label.clone(), args.message)
                        .with_schema(args.output_schema)],
                    FleetLimits::with_concurrency(1),
                    {
                        let spawner = self.spawner.clone();
                        let fleet = fleet.clone();
                        move |_: usize, cancel| spawner.resume_lane(&id, &label, &fleet, cancel)
                    },
                )
                .await?;
        let mut out = String::new();
        if cancelled {
            out.push_str(
                "NOTE: the agent was stopped before finishing; its reply may be partial.\n\n",
            );
        }
        out.push_str(&render_results(&results));
        Ok(out)
    }
}

/// Arguments for `read_agent`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadAgentArgs {
    /// The agent id from a spawn_agents or send_to_agent result.
    pub agent: String,
    /// Optional line range of the reply, e.g. "40-80", "120-", "-30".
    #[serde(default)]
    pub lines: Option<String>,
    /// Optional case-insensitive text to search the reply for; returns the
    /// matching lines with a little context.
    #[serde(default)]
    pub grep: Option<String>,
}

/// Read a lane's full final reply.
pub struct ReadAgentTool {
    spawner: Arc<FleetSpawner>,
}

impl ReadAgentTool {
    pub fn new(spawner: Arc<FleetSpawner>) -> Self {
        Self { spawner }
    }
}

#[async_trait]
impl TypedTool for ReadAgentTool {
    const NAME: &'static str = READ_AGENT_TOOL;

    type Args = ReadAgentArgs;

    fn description(&self) -> &str {
        "Read the full final reply of an agent you spawned (spawn_agents results are cut to a \
         summary; this is the rest). Give a line range or a grep to read just the part you \
         need. Read-only; it does not run the agent."
    }

    async fn run(&self, args: ReadAgentArgs, _call: &CallContext) -> Result<String, ToolError> {
        let id = args.agent.trim();
        let store = self
            .spawner
            .owned_lane_store(id)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let text = store
            .last_assistant_text(id)
            .map_err(|e| ToolError::Execution(e.to_string()))?
            .unwrap_or_default();
        if text.trim().is_empty() {
            return Ok("(this agent has not replied yet)".into());
        }
        if let Some(needle) = args
            .grep
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            let hits = harness_core::text::grep_lines(&text, needle, 2, 60);
            return Ok(if hits.is_empty() {
                format!("no line of the reply contains {needle:?}")
            } else {
                hits
            });
        }
        if let Some(spec) = args.lines.as_deref() {
            let (from, to) = harness_core::text::parse_line_range(spec).ok_or_else(|| {
                ToolError::InvalidArguments(format!(
                    "lines must look like \"40-80\", \"120-\" or \"-30\" (got {spec:?})"
                ))
            })?;
            return Ok(harness_core::text::slice_lines(&text, from, to));
        }
        let total = text.lines().count();
        Ok(harness_core::text::truncate_with_marker(
            &text,
            LANE_RESULT_CHARS,
            &format!(
                "\n… [reply cut at {LANE_RESULT_CHARS} chars of {total} lines; use lines or grep \
                 for the rest]"
            ),
        ))
    }
}

impl FleetSpawner {
    /// The persisted record of one of this session's lanes, if it has one.
    pub(crate) fn lane_record(&self, id: &str) -> Option<SubagentResult> {
        let store = self.owned_lane_store(id).ok()?;
        store
            .session_state::<SubagentResult>(id, harness_store::LANE_STATE)
            .ok()
            .flatten()
    }
}
