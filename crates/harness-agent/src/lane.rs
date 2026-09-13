//! A subagent lane once it has run: the typed result its parent reads
//! ([`SubagentResult`], also the record persisted beside the lane's
//! transcript), a structured-reply coercion for lanes asked to answer in
//! JSON, and the registry of lanes still running ([`AgentTree`]) that a host
//! steers and stops through.
//!
//! A lane's id is its session id: its transcript lives in the parent's
//! store under `parent_session`, so a finished lane can be read back
//! (`read_agent`) or resumed with a follow-up (`send_to_agent`) without
//! keeping any agent alive in memory.

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use harness_compress::CcrStore;
use harness_llm::LlmError;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::fleet::{LaneStop, SubagentOutcome, FLEET_RESULT_CHARS, LANE_RESULT_CHARS};
use crate::interject::Interjections;

/// How a lane ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneStatus {
    /// It finished its task on its own.
    Done,
    /// It was stopped early (a cancel, a clock) and what it had is kept.
    Partial,
    /// It errored; `summary` is the error.
    Failed,
}

/// Why a lane failed, coarse enough for the parent to act on without
/// parsing prose: an auth failure needs the user, a rate limit a retry, a
/// context overflow a smaller task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Auth,
    RateLimit,
    Provider,
    Context,
    Tool,
    Timeout,
    Other,
}

impl FailureKind {
    pub fn of(error: &AgentError) -> Self {
        match error {
            AgentError::Llm(LlmError::Api {
                status: 401 | 403, ..
            })
            | AgentError::Llm(LlmError::Auth(_)) => FailureKind::Auth,
            AgentError::Llm(LlmError::Api { status: 429, .. }) => FailureKind::RateLimit,
            AgentError::Llm(_) | AgentError::RetriesExhausted { .. } => FailureKind::Provider,
            AgentError::ContextWindowExceeded { .. } => FailureKind::Context,
            AgentError::Tool(_) => FailureKind::Tool,
            AgentError::TimedOut { .. } => FailureKind::Timeout,
            _ => FailureKind::Other,
        }
    }
}

/// What the parent reads about one lane, and what is persisted with the
/// lane's transcript (`LANE_STATE`). Short by construction: the summary is
/// the reply's head within the fleet's cap, and anything longer sits behind
/// an overflow handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    /// The lane's session id — what `send_to_agent` / `read_agent` take.
    pub id: String,
    pub fleet: String,
    pub label: String,
    pub status: LaneStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureKind>,
    /// Why a partial lane stopped, in words ("stopped after 600s …").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<String>,
    /// The reply (or error) within the cap.
    pub summary: String,
    /// Overflow-store hash of the whole reply when the cap cut it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// The parsed JSON reply, when the lane was asked for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<serde_json::Value>,
    /// Overflow-store hash of the lane's whole patch (isolated lanes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    pub tokens: usize,
    pub rounds: u32,
    /// Commands the lane's gate refused (a subagent cannot ask for
    /// approval); the parent or user runs them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied: Vec<String>,
}

/// The per-lane character budget for a fleet of `count` lanes: each lane's
/// cap, or its share of the whole document's when that is tighter.
pub fn lane_budget(count: usize) -> usize {
    LANE_RESULT_CHARS
        .min(FLEET_RESULT_CHARS / count.max(1))
        .max(1)
}

impl SubagentResult {
    /// Type one outcome, cutting its reply to `budget` characters and parking
    /// the whole reply in `spill` (when there is one) behind a handle.
    pub fn from_outcome(
        outcome: &SubagentOutcome,
        fleet: &str,
        budget: usize,
        spill: Option<&CcrStore>,
    ) -> Self {
        let (status, failure, summary, output) = match &outcome.result {
            Err(e) => (
                LaneStatus::Failed,
                Some(FailureKind::of(e)),
                e.to_string(),
                None,
            ),
            Ok(text) => {
                let text = text.trim();
                let status = if outcome.stopped.is_some() {
                    LaneStatus::Partial
                } else {
                    LaneStatus::Done
                };
                let (summary, output) = if text.chars().count() > budget {
                    let head: String = text.chars().take(budget).collect();
                    (head, spill.map(|store| store.put(text)))
                } else {
                    (text.to_string(), None)
                };
                (status, None, summary, output)
            }
        };
        Self {
            id: outcome.session.clone(),
            fleet: fleet.to_string(),
            label: outcome.label.clone(),
            status,
            failure,
            stop: outcome.stopped.map(|stop: LaneStop| stop.to_string()),
            summary,
            output,
            structured: outcome.structured.clone(),
            patch: None,
            tokens: outcome.tokens_used,
            rounds: outcome.rounds,
            denied: outcome.denied.clone(),
        }
    }

    /// A row for a tool-less leaf call (`map_agents` with `leaf`): no lane
    /// session, one round, the answer as the summary.
    pub fn leaf(label: String, text: String) -> Self {
        Self {
            id: String::new(),
            fleet: String::new(),
            label,
            status: LaneStatus::Done,
            failure: None,
            stop: None,
            summary: text.trim().to_string(),
            output: None,
            structured: None,
            patch: None,
            tokens: 0,
            rounds: 1,
            denied: Vec::new(),
        }
    }

    /// A failed row with no lane behind it.
    pub fn failed(label: String, error: String) -> Self {
        Self {
            id: String::new(),
            fleet: String::new(),
            label,
            status: LaneStatus::Failed,
            failure: Some(FailureKind::Provider),
            stop: None,
            summary: error,
            output: None,
            structured: None,
            patch: None,
            tokens: 0,
            rounds: 0,
            denied: Vec::new(),
        }
    }

    /// The summary as one short line, for lists and hubs.
    pub fn brief(&self) -> String {
        harness_core::text::ellipsize(&harness_core::text::collapse_ws(&self.summary), 200)
    }

    /// The one-line status the parent (and a hub) shows beside the label.
    pub fn status_line(&self) -> String {
        let spend = format!(
            "{} tok · {} round{}",
            harness_core::fmt::human_tokens(self.tokens),
            self.rounds,
            if self.rounds == 1 { "" } else { "s" }
        );
        match self.status {
            LaneStatus::Done => format!("done · {spend}"),
            LaneStatus::Partial => format!(
                "partial — {} · {spend}",
                self.stop.as_deref().unwrap_or("stopped early")
            ),
            LaneStatus::Failed => format!(
                "failed ({}) · {spend}",
                self.failure
                    .map(|k| format!("{k:?}").to_lowercase())
                    .unwrap_or_else(|| "other".into())
            ),
        }
    }

    /// This lane's block of the document the parent reads.
    pub fn render(&self) -> String {
        let mut out = format!(
            "### {} — {}\nagent id: {}\n\n",
            self.label,
            self.status_line(),
            self.id
        );
        match self.status {
            LaneStatus::Failed => out.push_str(&format!("(this agent failed: {})", self.summary)),
            _ => out.push_str(&self.summary),
        }
        if let Some(hash) = &self.output {
            out.push_str(&format!(
                "\n… [reply cut at {} chars; the whole reply is {} — call retrieve_original \
                 with that hash, or read_agent with this agent id, for the rest]",
                self.summary.chars().count(),
                harness_compress::ccr::marker(hash, Some("full_reply"))
            ));
        }
        if let Some(structured) = &self.structured {
            out.push_str(&format!("\n\n```json\n{structured}\n```"));
        }
        if !self.denied.is_empty() {
            out.push_str("\n\nCould not run (needs user approval): ");
            out.push_str(
                &self
                    .denied
                    .iter()
                    .map(|c| format!("`{c}`"))
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
        out
    }
}

/// The document a fleet hands the parent: every lane's block, in lane order.
pub fn render_results(results: &[SubagentResult]) -> String {
    results
        .iter()
        .map(SubagentResult::render)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// How many times a lane is re-asked for a well-formed JSON reply before
/// its prose is accepted as-is.
const STRUCTURED_RETRIES: usize = 2;

impl Agent {
    /// Coerce a lane's final reply into the JSON object `schema` asks for:
    /// the reply's first JSON object must carry every key in
    /// `schema.required`. A reply that doesn't is answered with a short
    /// corrective and the lane re-asked, up to [`STRUCTURED_RETRIES`] times;
    /// after that its prose stands (the parent sees no `structured`).
    pub(crate) async fn coerce_structured<F>(
        &mut self,
        mut text: String,
        schema: &serde_json::Value,
        mut on_event: F,
    ) -> Result<(String, Option<serde_json::Value>), AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        let required: Vec<&str> = schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|keys| keys.iter().filter_map(|k| k.as_str()).collect())
            .unwrap_or_default();
        for attempt in 0..=STRUCTURED_RETRIES {
            if let Some(object) = harness_core::json::first_object(&text) {
                let missing: Vec<&str> = required
                    .iter()
                    .copied()
                    .filter(|key| object.get(*key).is_none())
                    .collect();
                if missing.is_empty() {
                    return Ok((text, Some(object)));
                }
            }
            if attempt == STRUCTURED_RETRIES {
                break;
            }
            let keys = if required.is_empty() {
                String::new()
            } else {
                format!(" with the keys {}", required.join(", "))
            };
            text = self
                .run_turn(
                    format!(
                        "Your reply must be a single JSON object{keys}. Reply with ONLY that \
                         JSON object — no prose before or after it."
                    ),
                    &mut on_event,
                )
                .await?;
        }
        Ok((text, None))
    }

    /// The commands this agent's gate refused because a subagent cannot ask
    /// for approval — read out of the transcript, so the parent can run them
    /// or ask the user. Empty for an agent with an interactive gate.
    pub fn denied_commands(&self) -> Vec<String> {
        let mut calls: HashMap<&str, &harness_llm::types::ToolCall> = HashMap::new();
        for message in self.messages() {
            for call in message.tool_calls.iter().flatten() {
                calls.insert(call.id.as_str(), call);
            }
        }
        self.messages()
            .iter()
            .filter(|m| m.role == "tool")
            .filter(|m| {
                m.content_text()
                    .is_some_and(|text| text.starts_with(harness_permissions::SUBAGENT_DENIAL))
            })
            .filter_map(|m| m.tool_call_id.as_deref())
            .filter_map(|id| calls.get(id))
            .map(|call| {
                let command = call.function.parsed_arguments().ok().and_then(|args| {
                    args.get("command")
                        .and_then(|c| c.as_str())
                        .map(str::to_owned)
                });
                command.unwrap_or_else(|| {
                    format!("{} {}", call.function.name, call.function.arguments)
                })
            })
            .map(|c| harness_core::text::ellipsize(&harness_core::text::collapse_ws(&c), 200))
            .collect()
    }
}

/// One lane still running: enough to stop it or hand it a message.
pub struct LiveLane {
    pub id: String,
    pub label: String,
    pub fleet: String,
    pub started: Instant,
    pub cancel: CancellationToken,
    pub steer: Interjections,
}

/// What a host reads about a running lane.
#[derive(Debug, Clone, Serialize)]
pub struct LiveLaneInfo {
    pub id: String,
    pub label: String,
    pub fleet: String,
    pub elapsed_secs: u64,
}

/// The lanes of one session that are running right now, by lane id. Lanes
/// register when they are built and leave when their fleet settles; a
/// finished lane is just its persisted transcript and record.
#[derive(Default)]
pub struct AgentTree {
    live: StdMutex<HashMap<String, LiveLane>>,
}

impl AgentTree {
    pub fn register(&self, lane: LiveLane) {
        self.live
            .lock()
            .expect("agent tree poisoned")
            .insert(lane.id.clone(), lane);
    }

    /// Every lane of `fleet` has settled.
    pub fn finish_fleet(&self, fleet: &str) {
        self.live
            .lock()
            .expect("agent tree poisoned")
            .retain(|_, lane| lane.fleet != fleet);
    }

    /// Stop one lane; `false` when it isn't running (any more).
    pub fn cancel(&self, id: &str) -> bool {
        match self.live.lock().expect("agent tree poisoned").get(id) {
            Some(lane) => {
                lane.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Hand a running lane a message for its next round; `false` when it
    /// isn't running.
    pub fn interject(&self, id: &str, text: impl Into<String>) -> bool {
        match self.live.lock().expect("agent tree poisoned").get(id) {
            Some(lane) => {
                lane.steer.push(text);
                true
            }
            None => false,
        }
    }

    pub fn live(&self) -> Vec<LiveLaneInfo> {
        let mut lanes: Vec<LiveLaneInfo> = self
            .live
            .lock()
            .expect("agent tree poisoned")
            .values()
            .map(|lane| LiveLaneInfo {
                id: lane.id.clone(),
                label: lane.label.clone(),
                fleet: lane.fleet.clone(),
                elapsed_secs: lane.started.elapsed().as_secs(),
            })
            .collect();
        lanes.sort_by(|a, b| b.elapsed_secs.cmp(&a.elapsed_secs).then(a.id.cmp(&b.id)));
        lanes
    }

    pub fn is_live(&self, id: &str) -> bool {
        self.live
            .lock()
            .expect("agent tree poisoned")
            .contains_key(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(text: Result<&str, AgentError>) -> SubagentOutcome {
        SubagentOutcome {
            label: "scan".into(),
            session: "lane-1".into(),
            result: text.map(str::to_owned),
            structured: None,
            tokens_used: 12_300,
            rounds: 3,
            stopped: None,
            denied: Vec::new(),
        }
    }

    #[test]
    fn a_result_types_the_outcome_and_parks_a_long_reply() {
        let store = CcrStore::default();
        let long = "y".repeat(500);
        let done = SubagentResult::from_outcome(&outcome(Ok(&long)), "fleet-1", 100, Some(&store));
        assert_eq!(done.status, LaneStatus::Done);
        assert_eq!(done.summary.len(), 100);
        let hash = done.output.clone().expect("the whole reply is parked");
        assert_eq!(store.get(&hash).as_deref(), Some(long.as_str()));
        let rendered = done.render();
        assert!(rendered.starts_with("### scan — done · 12.3k tok · 3 rounds\nagent id: lane-1"));
        assert!(rendered.contains(&format!("<<ccr:{hash} full_reply>>")));

        let mut stopped = outcome(Ok("half"));
        stopped.stopped = Some(LaneStop::Cancelled);
        stopped.denied = vec!["rm -rf build".into()];
        let partial = SubagentResult::from_outcome(&stopped, "fleet-1", 100, None);
        assert_eq!(partial.status, LaneStatus::Partial);
        assert!(partial
            .render()
            .contains("partial — stopped early (cancelled)"));
        assert!(partial
            .render()
            .contains("Could not run (needs user approval): `rm -rf build`"));

        let failed = SubagentResult::from_outcome(
            &outcome(Err(AgentError::Llm(LlmError::Api {
                status: 429,
                message: "slow down".into(),
            }))),
            "fleet-1",
            100,
            None,
        );
        assert_eq!(failed.status, LaneStatus::Failed);
        assert_eq!(failed.failure, Some(FailureKind::RateLimit));
        assert!(failed.render().contains("failed (ratelimit)"));
        assert!(failed.render().contains("slow down"));

        // The persisted form round-trips.
        let json = serde_json::to_string(&partial).unwrap();
        let back: SubagentResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.denied, partial.denied);
        assert_eq!(back.status, LaneStatus::Partial);
    }

    #[test]
    fn lane_budgets_share_the_document_cap() {
        assert_eq!(lane_budget(1), LANE_RESULT_CHARS);
        assert_eq!(lane_budget(6), FLEET_RESULT_CHARS / 6);
        assert_eq!(lane_budget(0), LANE_RESULT_CHARS);
    }

    #[test]
    fn the_tree_stops_and_steers_only_running_lanes() {
        let tree = AgentTree::default();
        let cancel = CancellationToken::new();
        let steer = Interjections::default();
        tree.register(LiveLane {
            id: "l1".into(),
            label: "scan".into(),
            fleet: "f1".into(),
            started: Instant::now(),
            cancel: cancel.clone(),
            steer: steer.clone(),
        });
        assert!(tree.interject("l1", "look at src/ too"));
        assert_eq!(steer.pending(), 1);
        assert!(tree.cancel("l1"));
        assert!(cancel.is_cancelled());
        assert_eq!(tree.live().len(), 1);
        tree.finish_fleet("f1");
        assert!(!tree.cancel("l1"));
        assert!(!tree.interject("l1", "too late"));
        assert!(tree.live().is_empty());
    }
}
