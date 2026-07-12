//! Run a fleet of subagents in parallel.
//!
//! A fleet takes N independent tasks (a label + a prompt each), runs each on
//! its own detached [`Agent::side_agent`] — full tool use, in-memory store,
//! nothing touches the caller's session — and multiplexes their progress into
//! one ordered event stream a host can render as live lanes. Concurrency is
//! capped by a semaphore, one task's failure never takes down the others, and
//! a single cancellation token stops the whole fleet cooperatively.
//!
//! Every subagent's events flow through one channel, so *per-agent* ordering
//! is preserved (an agent's `TaskStarted` always precedes its `Agent` events,
//! which precede its `TaskCompleted`), while different agents' events
//! interleave as they actually happen — exactly what a live multi-lane
//! display wants.

use std::sync::Arc;

use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::error::AgentError;
use crate::event::AgentEvent;

/// One unit of work for a fleet: what to call it and what to ask it.
#[derive(Debug, Clone)]
pub struct SubagentTask {
    /// Short display name ("diff-scan", "callers") used in events and lanes.
    pub label: String,
    /// The prompt the subagent runs as its single turn.
    pub prompt: String,
}

impl SubagentTask {
    pub fn new(label: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            prompt: prompt.into(),
        }
    }
}

/// Progress multiplexed from every subagent, tagged by task index (the
/// position in the `tasks` vec passed to [`run_fleet`]).
#[derive(Debug, Clone)]
pub enum FleetEvent {
    /// The task acquired a concurrency slot and its turn is now running.
    TaskStarted { index: usize, label: String },
    /// A streaming/tool event from one subagent's turn. Held in an [`Arc`] so
    /// the event is deep-cloned exactly once (crossing the task→drive-loop
    /// channel); every hop after — into a `ReviewEvent`, a host payload — is a
    /// refcount bump, not another copy of a token string.
    Agent {
        index: usize,
        event: Arc<AgentEvent>,
    },
    /// The task finished (its outcome is in [`run_fleet`]'s return value).
    /// `summary` is a short display string: the truncated final reply, or the
    /// error text when `ok` is false.
    TaskCompleted {
        index: usize,
        label: String,
        ok: bool,
        tokens_used: usize,
        summary: String,
    },
}

/// How a host renders a fleet that runs *inside* a turn (the `spawn_agents`
/// tool): bracketed start/finish around the multiplexed event stream, so the
/// host can build and tear down its lanes display. Mirrors the `CanvasSink` /
/// `QuestionAsker` pattern — the host injects an implementation at registry
/// build time.
///
/// `finished` MUST be idempotent: the tool calls it through a drop guard so a
/// cancelled (dropped) turn still tears the display down.
pub trait FleetSink: Send + Sync {
    /// A fleet is starting: lane labels in order, plus the token that cancels
    /// just this fleet (a child of the turn's token — hosts may wire it to a
    /// key or button).
    fn started(&self, labels: &[String], cancel: CancellationToken);
    /// One multiplexed progress event.
    fn event(&self, event: &FleetEvent);
    /// The fleet is done (or was abandoned); tear down the lanes display.
    fn finished(&self);
}

/// What one task produced: the subagent's final text (or the error that ended
/// it) plus what it cost.
#[derive(Debug)]
pub struct SubagentOutcome {
    pub label: String,
    pub result: Result<String, AgentError>,
    /// Estimated tokens this subagent spent (prompt + completion, all calls).
    pub tokens_used: usize,
}

impl SubagentOutcome {
    pub fn ok(&self) -> bool {
        self.result.is_ok()
    }
}

/// Concatenate fleet outcomes into one labeled document — `### {label}` headings
/// with each agent's trimmed reply (or an inline failure note), in agent order.
/// This is the shape a fleet's combined result takes wherever it's read as
/// model input: the `spawn_agents` tool return and the review pipeline's
/// fan-out step output both go through here, so the document shape can't drift
/// between them. `failure_noun` names what failed in the inline note ("agent",
/// "reviewer").
pub fn combine_outcomes(outcomes: &[SubagentOutcome], failure_noun: &str) -> String {
    let mut out = String::new();
    for outcome in outcomes {
        out.push_str(&format!("### {}\n\n", outcome.label));
        match &outcome.result {
            Ok(text) => out.push_str(text.trim()),
            Err(e) => out.push_str(&format!("(this {failure_noun} failed: {e})")),
        }
        out.push_str("\n\n");
    }
    out.trim_end().to_string()
}

/// Cap on the completion summary carried in [`FleetEvent::TaskCompleted`].
const SUMMARY_CHARS: usize = 120;

/// The channel message a subagent task sends; `Done` is always its last, so
/// per-task event order is preserved end to end.
enum Msg {
    Started {
        index: usize,
        label: String,
    },
    Agent {
        index: usize,
        event: Arc<AgentEvent>,
    },
    Done {
        index: usize,
        outcome: SubagentOutcome,
    },
}

/// How a fleet builds each subagent: any source of fresh, detached agents.
/// [`Agent::side_agent`] is the usual one (`|| agent.side_agent()`); the
/// `spawn_agents` tool uses a standalone [`FleetSpawner`](crate::fleet_tool::FleetSpawner)
/// so a fleet can run from inside a turn.
pub trait SpawnAgent {
    fn spawn(&self) -> Result<Agent, AgentError>;
}

impl<F> SpawnAgent for F
where
    F: Fn() -> Result<Agent, AgentError>,
{
    fn spawn(&self) -> Result<Agent, AgentError> {
        self()
    }
}

/// Run `tasks` in parallel, each on a fresh agent from `spawn`, at most
/// `concurrency` at a time (clamped to ≥ 1), streaming progress to `on_event`.
///
/// Returns one [`SubagentOutcome`] per task, in task order, when every task
/// has finished. A task that errors (or panics) yields an `Err` outcome; the
/// rest keep running. Cancelling `cancel` stops every in-flight turn
/// cooperatively — cancelled turns end with whatever partial text streamed, so
/// the caller decides (usually via `cancel.is_cancelled()`) whether the
/// results are usable.
pub async fn run_fleet<S, F>(
    spawn: S,
    tasks: Vec<SubagentTask>,
    concurrency: usize,
    cancel: CancellationToken,
    mut on_event: F,
) -> Result<Vec<SubagentOutcome>, AgentError>
where
    S: SpawnAgent,
    F: FnMut(&FleetEvent),
{
    let count = tasks.len();
    if count == 0 {
        return Ok(Vec::new());
    }

    // Progress is observational and may be coalesced by hosts; never let a slow
    // UI retain an unbounded token-event backlog. Start/done milestones await
    // capacity, while intermediate events are dropped when the lane is saturated.
    let (tx, mut rx) = mpsc::channel::<Msg>(256);
    let slots = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut join = JoinSet::new();

    for (index, task) in tasks.into_iter().enumerate() {
        // Build the subagent up front so construction errors surface here,
        // synchronously, instead of as a mid-flight task failure.
        let mut agent = spawn.spawn()?;
        agent.set_cancel_token(cancel.clone());
        let tx = tx.clone();
        let slots = slots.clone();
        join.spawn(async move {
            let _slot = match slots.acquire_owned().await {
                Ok(permit) => permit,
                // The semaphore is never closed today; if that ever changes,
                // bail (the reaper synthesizes a failed outcome) rather than
                // silently running uncapped.
                Err(_) => return,
            };
            let _ = tx
                .send(Msg::Started {
                    index,
                    label: task.label.clone(),
                })
                .await;
            let forward = tx.clone();
            let result = agent
                .run_turn(task.prompt, |event| {
                    // The one deep clone: from the borrowed callback event into
                    // an Arc that rides the channel and every downstream hop.
                    let _ = forward.try_send(Msg::Agent {
                        index,
                        event: Arc::new(event.clone()),
                    });
                })
                .await;
            let _ = tx
                .send(Msg::Done {
                    index,
                    outcome: SubagentOutcome {
                        label: task.label,
                        result,
                        tokens_used: agent.tokens_used(),
                    },
                })
                .await;
        });
    }
    // Drop the original sender: the channel closes exactly when every task has
    // sent its `Done` (or died), which is the drive loop's exit condition.
    drop(tx);

    let mut outcomes: Vec<Option<SubagentOutcome>> = (0..count).map(|_| None).collect();
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Started { index, label } => on_event(&FleetEvent::TaskStarted { index, label }),
            Msg::Agent { index, event } => on_event(&FleetEvent::Agent { index, event }),
            Msg::Done { index, outcome } => {
                on_event(&FleetEvent::TaskCompleted {
                    index,
                    label: outcome.label.clone(),
                    ok: outcome.ok(),
                    tokens_used: outcome.tokens_used,
                    summary: summarize(&outcome),
                });
                outcomes[index] = Some(outcome);
            }
        }
    }

    // Reap the join handles. A panicked task never sent `Done`; surface it as
    // that task's failed outcome below rather than poisoning the whole fleet.
    while let Some(reaped) = join.join_next().await {
        if let Err(e) = reaped {
            tracing::warn!("fleet subagent task died: {e}");
        }
    }

    Ok(outcomes
        .into_iter()
        .enumerate()
        .map(|(index, outcome)| {
            outcome.unwrap_or_else(|| SubagentOutcome {
                label: format!("agent {}", index + 1),
                result: Err(AgentError::Io(std::io::Error::other(
                    "the subagent task died before finishing",
                ))),
                tokens_used: 0,
            })
        })
        .collect())
}

/// The short display summary for a finished task.
fn summarize(outcome: &SubagentOutcome) -> String {
    let text = match &outcome.result {
        Ok(text) => text.as_str(),
        Err(e) => return truncate(&e.to_string()),
    };
    truncate(text)
}

fn truncate(s: &str) -> String {
    harness_core::text::ellipsize(&harness_core::text::collapse_ws(s), SUMMARY_CHARS)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use harness_llm::OxenClient;
    use harness_store::HistoryStore;
    use harness_tools::ToolRegistry;

    use super::*;
    use crate::test_support::{sse_prose, test_session};
    use crate::AgentConfig;

    fn base_agent(url: &str) -> Agent {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(url, "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            ..AgentConfig::default()
        };
        Agent::new(client, ToolRegistry::new(), store, session, config).unwrap()
    }

    #[tokio::test]
    async fn fleet_runs_tasks_and_returns_outcomes_in_task_order() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("TASK-ALPHA".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("alpha done"))
            .create_async()
            .await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("TASK-BETA".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("beta done"))
            .create_async()
            .await;

        let base = base_agent(&server.url());
        let mut events = Vec::new();
        let outcomes = run_fleet(
            || base.side_agent(),
            vec![
                SubagentTask::new("alpha", "do TASK-ALPHA"),
                SubagentTask::new("beta", "do TASK-BETA"),
            ],
            2,
            CancellationToken::new(),
            |e| events.push(e.clone()),
        )
        .await
        .unwrap();

        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].label, "alpha");
        assert_eq!(outcomes[0].result.as_deref().unwrap(), "alpha done");
        assert_eq!(outcomes[1].result.as_deref().unwrap(), "beta done");
        assert!(outcomes.iter().all(|o| o.tokens_used > 0));

        // Two starts, two completions, and per-agent ordering held.
        let started: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, FleetEvent::TaskStarted { .. }))
            .collect();
        assert_eq!(started.len(), 2);
        for index in 0..2 {
            let positions: Vec<usize> = events
                .iter()
                .enumerate()
                .filter_map(|(i, e)| match e {
                    FleetEvent::TaskStarted { index: t, .. } if *t == index => Some(i),
                    FleetEvent::TaskCompleted { index: t, .. } if *t == index => Some(i),
                    _ => None,
                })
                .collect();
            assert_eq!(positions.len(), 2, "start + complete for task {index}");
            assert!(positions[0] < positions[1]);
        }
        // The base agent's own session saw none of it.
        assert!(base.messages().is_empty());
    }

    #[tokio::test]
    async fn concurrency_cap_of_one_serializes_the_fleet() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("done"))
            .expect(2)
            .create_async()
            .await;

        let base = base_agent(&server.url());
        let mut order = Vec::new();
        run_fleet(
            || base.side_agent(),
            vec![
                SubagentTask::new("first", "go"),
                SubagentTask::new("second", "go"),
            ],
            1,
            CancellationToken::new(),
            |e| match e {
                FleetEvent::TaskStarted { index, .. } => order.push(format!("start-{index}")),
                FleetEvent::TaskCompleted { index, .. } => order.push(format!("end-{index}")),
                _ => {}
            },
        )
        .await
        .unwrap();

        // With one slot, a task's start can never precede the prior completion.
        let starts: Vec<usize> = order
            .iter()
            .enumerate()
            .filter(|(_, s)| s.starts_with("start"))
            .map(|(i, _)| i)
            .collect();
        let ends: Vec<usize> = order
            .iter()
            .enumerate()
            .filter(|(_, s)| s.starts_with("end"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(starts.len(), 2);
        assert!(
            ends[0] < starts[1],
            "second start before first end: {order:?}"
        );
    }

    #[tokio::test]
    async fn one_failing_task_does_not_take_down_the_rest() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("TASK-GOOD".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("survived"))
            .create_async()
            .await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("TASK-BAD".into()))
            .with_status(401)
            .with_body(r#"{"error":{"message":"Invalid API key"}}"#)
            .create_async()
            .await;

        let base = base_agent(&server.url());
        let mut completed = Vec::new();
        let outcomes = run_fleet(
            || base.side_agent(),
            vec![
                SubagentTask::new("good", "do TASK-GOOD"),
                SubagentTask::new("bad", "do TASK-BAD"),
            ],
            2,
            CancellationToken::new(),
            |e| {
                if let FleetEvent::TaskCompleted { label, ok, .. } = e {
                    completed.push((label.clone(), *ok));
                }
            },
        )
        .await
        .unwrap();

        assert!(outcomes[0].ok());
        assert!(!outcomes[1].ok());
        assert!(outcomes[1]
            .result
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("401"));
        assert_eq!(completed.len(), 2);
        assert!(completed.contains(&("good".to_string(), true)));
        assert!(completed.contains(&("bad".to_string(), false)));
    }

    #[tokio::test]
    async fn a_pre_cancelled_fleet_settles_immediately_with_empty_turns() {
        // Unroutable endpoint: if cancellation didn't short-circuit before the
        // network call, the turns would hang/error on connect.
        let base = base_agent("http://127.0.0.1:1/api/ai");
        let cancel = CancellationToken::new();
        cancel.cancel();

        let outcomes = run_fleet(
            || base.side_agent(),
            vec![SubagentTask::new("a", "go"), SubagentTask::new("b", "go")],
            2,
            cancel.clone(),
            |_| {},
        )
        .await
        .unwrap();

        assert!(cancel.is_cancelled());
        for o in &outcomes {
            assert_eq!(o.result.as_deref().unwrap(), "");
        }
    }

    #[tokio::test]
    async fn empty_task_list_returns_no_outcomes() {
        let base = base_agent("http://127.0.0.1:1/api/ai");
        let outcomes = run_fleet(
            || base.side_agent(),
            Vec::new(),
            4,
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(outcomes.is_empty());
    }
}
