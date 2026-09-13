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
//!
//! Two clocks bound a fleet ([`FleetLimits`]): a per-lane time limit and a
//! whole-fleet deadline. Either stops lanes cooperatively and keeps what they
//! produced — a stopped lane is a partial result, not a lost one — so a
//! wedged lane can never hold the parent turn open indefinitely.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use harness_compress::CcrStore;
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
/// Every call names its fleet: a `wait: false` fleet can overlap a later one
/// in the same session, and the host keeps their lanes (and their stop
/// buttons) apart by id. `finished` MUST be idempotent per fleet: the tool
/// calls it through a drop guard so a cancelled (dropped) turn still tears
/// the display down.
pub trait FleetSink: Send + Sync {
    /// A fleet is starting: lane labels in order, plus the token that cancels
    /// just this fleet (a child of the turn's token — hosts wire it to a key
    /// or button).
    fn started(&self, fleet: &str, labels: &[String], cancel: CancellationToken);
    /// One multiplexed progress event from `fleet`.
    fn event(&self, fleet: &str, event: &FleetEvent);
    /// `fleet` is done (or was abandoned); tear down its lanes display.
    fn finished(&self, fleet: &str);
}

/// Why a lane was stopped before its turn ended on its own. The lane's
/// partial reply is still its result; this says how much to trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneStop {
    /// The lane ran past [`FleetLimits::lane_timeout`].
    TimedOut(Duration),
    /// The whole fleet ran past [`FleetLimits::deadline`].
    Deadline(Duration),
    /// The fleet's token was cancelled (the user stopped the turn or the fleet).
    Cancelled,
}

impl std::fmt::Display for LaneStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaneStop::TimedOut(after) => {
                write!(f, "stopped after {}s (its time limit)", after.as_secs())
            }
            LaneStop::Deadline(after) => write!(
                f,
                "stopped after {}s (the fleet's deadline)",
                after.as_secs()
            ),
            LaneStop::Cancelled => write!(f, "stopped early (cancelled)"),
        }
    }
}

/// What one task produced: the subagent's final text (or the error that ended
/// it) plus what it cost.
#[derive(Debug)]
pub struct SubagentOutcome {
    pub label: String,
    pub result: Result<String, AgentError>,
    /// Estimated tokens this subagent spent (prompt + completion, all calls).
    pub tokens_used: usize,
    /// Set when the lane was stopped before finishing: `result` then holds
    /// whatever it had produced (possibly nothing), and the combined document
    /// says so beside it.
    pub stopped: Option<LaneStop>,
}

impl SubagentOutcome {
    /// Whether the lane finished its task: it produced a reply and nothing
    /// cut it short.
    pub fn ok(&self) -> bool {
        self.result.is_ok() && self.stopped.is_none()
    }
}

/// How long one lane may run before it is stopped, by default.
pub const DEFAULT_LANE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How long a whole fleet may run before every lane is stopped, by default.
pub const DEFAULT_FLEET_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// After a lane is told to stop, how long it gets to settle cooperatively
/// (finish the in-flight model call or tool) before its turn is abandoned.
const STOP_GRACE: Duration = Duration::from_secs(20);

/// What bounds a fleet run: how many lanes at once, and how long each lane
/// and the whole fleet may take.
#[derive(Debug, Clone, Copy)]
pub struct FleetLimits {
    /// Lanes running at once (clamped to ≥ 1).
    pub concurrency: usize,
    /// The longest one lane may run; past it the lane is stopped and its
    /// partial reply reported with [`LaneStop::TimedOut`].
    pub lane_timeout: Duration,
    /// The longest the whole fleet may run, queued lanes included; past it
    /// every lane is stopped with [`LaneStop::Deadline`].
    pub deadline: Duration,
}

impl FleetLimits {
    /// The default clocks with the given concurrency.
    pub fn with_concurrency(concurrency: usize) -> Self {
        Self {
            concurrency,
            lane_timeout: DEFAULT_LANE_TIMEOUT,
            deadline: DEFAULT_FLEET_DEADLINE,
        }
    }
}

/// Most characters of one lane's reply that reach the parent when the fleet's
/// document is capped (see [`ResultCap`]).
pub const LANE_RESULT_CHARS: usize = 12_000;

/// Most characters the whole combined document may take when capped. The
/// same ceiling the aside delivery applies, so a fleet the model waited for
/// and one it didn't land in the parent's context at the same size.
pub const FLEET_RESULT_CHARS: usize = 48_000;

/// How [`combine_outcomes_capped`] bounds a fleet's document: each lane gets
/// `per_lane` characters, or its share of `total` when that is smaller, and
/// the rest of an over-long reply goes to `spill` under a `<<ccr:HASH>>`
/// marker the parent can dereference with `retrieve_original` — capped in
/// context, never lost.
#[derive(Clone, Copy)]
pub struct ResultCap<'a> {
    pub per_lane: usize,
    pub total: usize,
    pub spill: Option<&'a CcrStore>,
}

impl<'a> ResultCap<'a> {
    /// The defaults, spilling into `spill` when given.
    pub fn default_with(spill: Option<&'a CcrStore>) -> Self {
        Self {
            per_lane: LANE_RESULT_CHARS,
            total: FLEET_RESULT_CHARS,
            spill,
        }
    }
}

/// Concatenate fleet outcomes into one labeled document — `### {label}` headings
/// with each agent's trimmed reply (or an inline failure note), in agent order.
/// This is the shape a fleet's combined result takes wherever it's read as
/// model input: the `spawn_agents` tool return and the review pipeline's
/// fan-out step output both go through here, so the document shape can't drift
/// between them. `failure_noun` names what failed in the inline note ("agent",
/// "reviewer"). Replies are passed through whole; see
/// [`combine_outcomes_capped`] for the bounded form.
pub fn combine_outcomes(outcomes: &[SubagentOutcome], failure_noun: &str) -> String {
    render_outcomes(outcomes, failure_noun, None)
}

/// [`combine_outcomes`] with every reply bounded by `cap`. This is the form
/// for a document that lands in a parent's context: six lanes each pasting
/// a whole file would otherwise arrive as one unbounded tool message.
pub fn combine_outcomes_capped(
    outcomes: &[SubagentOutcome],
    failure_noun: &str,
    cap: ResultCap<'_>,
) -> String {
    render_outcomes(outcomes, failure_noun, Some(cap))
}

fn render_outcomes(
    outcomes: &[SubagentOutcome],
    failure_noun: &str,
    cap: Option<ResultCap<'_>>,
) -> String {
    let budget = cap.map(|c| c.per_lane.min(c.total / outcomes.len().max(1)).max(1));
    let mut out = String::new();
    for outcome in outcomes {
        out.push_str(&format!("### {}\n\n", outcome.label));
        if let Some(stop) = outcome.stopped {
            out.push_str(&format!(
                "(this {failure_noun} was {stop}; what follows may be partial)\n\n"
            ));
        }
        match &outcome.result {
            Ok(text) => match (budget, cap) {
                (Some(budget), Some(cap)) => out.push_str(&bounded(text.trim(), budget, cap.spill)),
                _ => out.push_str(text.trim()),
            },
            Err(e) => out.push_str(&format!("(this {failure_noun} failed: {e})")),
        }
        out.push_str("\n\n");
    }
    out.trim_end().to_string()
}

/// `text` cut to `budget` characters, with the whole reply parked in `spill`
/// (when there is one) behind a marker the parent can dereference.
fn bounded(text: &str, budget: usize, spill: Option<&CcrStore>) -> String {
    let len = text.chars().count();
    if len <= budget {
        return text.to_string();
    }
    let head: String = text.chars().take(budget).collect();
    match spill {
        Some(store) => {
            let hash = store.put(text);
            format!(
                "{head}\n… [reply cut at {budget} of {len} chars; the whole reply is {} — \
                 call retrieve_original with that hash if you need the rest]",
                harness_compress::ccr::marker(&hash, Some("full_reply"))
            )
        }
        None => format!("{head}\n… [reply cut at {budget} of {len} chars]"),
    }
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
    /// Build the agent for lane `index`. The index is passed so a spawner can
    /// give each lane its own workspace (see [`crate::worktree`]) and match
    /// the resulting changes back to the task that made them.
    fn spawn(&self, index: usize) -> Result<Agent, AgentError>;
}

impl<F> SpawnAgent for F
where
    F: Fn(usize) -> Result<Agent, AgentError>,
{
    fn spawn(&self, index: usize) -> Result<Agent, AgentError> {
        self(index)
    }
}

/// Run `tasks` in parallel, each on a fresh agent from `spawn`, within
/// `limits` (lanes at once, per-lane time, whole-fleet deadline), streaming
/// progress to `on_event`.
///
/// Returns one [`SubagentOutcome`] per task, in task order, when every task
/// has finished. A task that errors (or panics) yields an `Err` outcome; the
/// rest keep running. Cancelling `cancel` stops every in-flight turn
/// cooperatively — cancelled turns end with whatever partial text streamed,
/// marked [`LaneStop::Cancelled`]. A lane past its time limit, or the whole
/// fleet past its deadline, is stopped the same way and marked accordingly,
/// so a wedged lane never holds the caller open.
pub async fn run_fleet<S, F>(
    spawn: S,
    tasks: Vec<SubagentTask>,
    limits: FleetLimits,
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
    let slots = Arc::new(Semaphore::new(limits.concurrency.max(1)));
    // The fleet's own stop signal, a child of the caller's: the caller
    // cancelling stops the fleet, while the deadline stops only the fleet —
    // a review step past its clock must not cancel the whole review.
    let cancel = cancel.child_token();
    // Set when the fleet deadline fires, so a lane the deadline stopped reports
    // that rather than a plain cancellation.
    let past_deadline = Arc::new(AtomicBool::new(false));
    let mut join = JoinSet::new();

    for (index, task) in tasks.into_iter().enumerate() {
        // Build the subagent up front so construction errors surface here,
        // synchronously, instead of as a mid-flight task failure.
        let mut agent = spawn.spawn(index)?;
        // Each lane stops on its own token, a child of the fleet's: the fleet
        // stopping stops the lane, and the lane's clock can stop just the lane.
        let lane_cancel = cancel.child_token();
        agent.set_cancel_token(lane_cancel.clone());
        let tx = tx.clone();
        let slots = slots.clone();
        let fleet_cancel = cancel.clone();
        let past_deadline = past_deadline.clone();
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
            // Scoped so the turn's borrow of `agent` ends before its spend is
            // read below.
            let (result, timed_out) = {
                let turn = agent.run_turn(task.prompt, |event| {
                    // The one deep clone: from the borrowed callback event into
                    // an Arc that rides the channel and every downstream hop.
                    let _ = forward.try_send(Msg::Agent {
                        index,
                        event: Arc::new(event.clone()),
                    });
                });
                tokio::pin!(turn);
                tokio::select! {
                    result = &mut turn => (result, false),
                    _ = tokio::time::sleep(limits.lane_timeout) => {
                        lane_cancel.cancel();
                        // The turn checks its token between rounds and while
                        // streaming; give it that long to settle, then abandon
                        // it (dropping the future) rather than wait on a
                        // wedged tool.
                        match tokio::time::timeout(STOP_GRACE, &mut turn).await {
                            Ok(result) => (result, true),
                            Err(_) => (
                                Err(AgentError::TimedOut {
                                    after: limits.lane_timeout,
                                }),
                                true,
                            ),
                        }
                    }
                }
            };
            let stopped = if timed_out {
                Some(LaneStop::TimedOut(limits.lane_timeout))
            } else if past_deadline.load(Ordering::SeqCst) {
                Some(LaneStop::Deadline(limits.deadline))
            } else if fleet_cancel.is_cancelled() {
                Some(LaneStop::Cancelled)
            } else {
                None
            };
            let _ = tx
                .send(Msg::Done {
                    index,
                    outcome: SubagentOutcome {
                        label: task.label,
                        result,
                        tokens_used: agent.tokens_used(),
                        stopped,
                    },
                })
                .await;
        });
    }
    // Drop the original sender: the channel closes exactly when every task has
    // sent its `Done` (or died), which is the drive loop's exit condition.
    drop(tx);

    let mut outcomes: Vec<Option<SubagentOutcome>> = (0..count).map(|_| None).collect();
    let deadline = tokio::time::sleep(limits.deadline);
    tokio::pin!(deadline);
    let mut deadline_armed = true;
    loop {
        let msg = tokio::select! {
            msg = rx.recv() => match msg {
                Some(msg) => msg,
                None => break,
            },
            // The fleet's clock: past the deadline every lane — running or
            // still queued — is stopped, and the loop keeps draining so their
            // partial outcomes are collected like any other.
            _ = &mut deadline, if deadline_armed => {
                deadline_armed = false;
                past_deadline.store(true, Ordering::SeqCst);
                cancel.cancel();
                continue;
            }
        };
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
                stopped: None,
            })
        })
        .collect())
}

/// The short display summary for a finished task: how it was stopped, if it
/// was, then the truncated reply (or the error that ended it).
fn summarize(outcome: &SubagentOutcome) -> String {
    let body = match &outcome.result {
        Ok(text) => text.as_str(),
        Err(e) => return truncate(&e.to_string()),
    };
    match outcome.stopped {
        Some(stop) if body.trim().is_empty() => stop.to_string(),
        Some(stop) => truncate(&format!("{stop}; partial: {body}")),
        None => truncate(body),
    }
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
            |_| base.side_agent(),
            vec![
                SubagentTask::new("alpha", "do TASK-ALPHA"),
                SubagentTask::new("beta", "do TASK-BETA"),
            ],
            FleetLimits::with_concurrency(2),
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
            |_| base.side_agent(),
            vec![
                SubagentTask::new("first", "go"),
                SubagentTask::new("second", "go"),
            ],
            FleetLimits::with_concurrency(1),
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
            |_| base.side_agent(),
            vec![
                SubagentTask::new("good", "do TASK-GOOD"),
                SubagentTask::new("bad", "do TASK-BAD"),
            ],
            FleetLimits::with_concurrency(2),
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
            |_| base.side_agent(),
            vec![SubagentTask::new("a", "go"), SubagentTask::new("b", "go")],
            FleetLimits::with_concurrency(2),
            cancel.clone(),
            |_| {},
        )
        .await
        .unwrap();

        assert!(cancel.is_cancelled());
        for o in &outcomes {
            assert_eq!(o.result.as_deref().unwrap(), "");
            assert_eq!(o.stopped, Some(LaneStop::Cancelled));
            assert!(!o.ok(), "a stopped lane did not finish its task");
        }
    }

    /// An endpoint that streams a first token, then stalls for `stall` before
    /// finishing — the shape of a lane that will never converge in time.
    fn stalling_endpoint(server: &mut mockito::Server, stall: Duration) -> mockito::Mock {
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_chunked_body(move |w| {
                w.write_all(
                    b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n",
                )?;
                w.flush()?;
                std::thread::sleep(stall);
                w.write_all(sse_prose(" and the rest").as_bytes())
            })
            .create()
    }

    #[tokio::test]
    async fn a_lane_past_its_time_limit_is_stopped_with_what_it_had() {
        let mut server = mockito::Server::new_async().await;
        let _slow = stalling_endpoint(&mut server, Duration::from_secs(3));
        let base = base_agent(&server.url());
        let mut completed = Vec::new();
        let started = std::time::Instant::now();
        let outcomes = run_fleet(
            |_| base.side_agent(),
            vec![SubagentTask::new("slow", "go")],
            FleetLimits {
                concurrency: 1,
                lane_timeout: Duration::from_millis(300),
                deadline: DEFAULT_FLEET_DEADLINE,
            },
            CancellationToken::new(),
            |e| {
                if let FleetEvent::TaskCompleted { ok, summary, .. } = e {
                    completed.push((*ok, summary.clone()));
                }
            },
        )
        .await
        .unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the fleet must not wait for the stalled stream"
        );
        let lane = &outcomes[0];
        assert_eq!(
            lane.stopped,
            Some(LaneStop::TimedOut(Duration::from_millis(300)))
        );
        assert!(!lane.ok());
        // The token that streamed before the stall is kept, not discarded.
        assert_eq!(lane.result.as_deref().unwrap(), "partial");
        assert_eq!(completed.len(), 1);
        assert!(!completed[0].0);
        assert!(completed[0].1.contains("time limit"), "{}", completed[0].1);
    }

    #[tokio::test]
    async fn the_fleet_deadline_stops_running_and_queued_lanes_alike() {
        let mut server = mockito::Server::new_async().await;
        let _slow = stalling_endpoint(&mut server, Duration::from_secs(3));
        let base = base_agent(&server.url());
        let cancel = CancellationToken::new();
        let outcomes = run_fleet(
            |_| base.side_agent(),
            vec![SubagentTask::new("a", "go"), SubagentTask::new("b", "go")],
            FleetLimits {
                // One slot: `b` is still queued when the deadline lands.
                concurrency: 1,
                lane_timeout: DEFAULT_LANE_TIMEOUT,
                deadline: Duration::from_millis(300),
            },
            cancel.clone(),
            |_| {},
        )
        .await
        .unwrap();

        assert!(
            !cancel.is_cancelled(),
            "the deadline stops the fleet, never the caller's own token"
        );
        assert_eq!(
            outcomes[0].stopped,
            Some(LaneStop::Deadline(Duration::from_millis(300)))
        );
        assert_eq!(outcomes[0].result.as_deref().unwrap(), "partial");
        // The queued lane never got a slot before the deadline; it settles as
        // deadline-stopped with nothing, not as a lane that was never run.
        assert_eq!(
            outcomes[1].stopped,
            Some(LaneStop::Deadline(Duration::from_millis(300)))
        );
        assert_eq!(outcomes[1].result.as_deref().unwrap(), "");
    }

    fn outcome(label: &str, text: &str) -> SubagentOutcome {
        SubagentOutcome {
            label: label.into(),
            result: Ok(text.into()),
            tokens_used: 1,
            stopped: None,
        }
    }

    #[test]
    fn a_capped_document_cuts_long_replies_and_spills_them_whole() {
        let store = CcrStore::default();
        let long = "x".repeat(30_000);
        let outcomes = vec![outcome("short", "fine"), outcome("long", &long)];
        let doc = combine_outcomes_capped(
            &outcomes,
            "agent",
            ResultCap {
                per_lane: 12_000,
                total: 48_000,
                spill: Some(&store),
            },
        );
        assert!(doc.contains("### short\n\nfine"));
        assert!(doc.contains("reply cut at 12000 of 30000 chars"));
        assert!(
            doc.chars().count() < 13_000,
            "the document itself is bounded"
        );
        // The marker resolves to the whole reply.
        let hash = doc
            .split("<<ccr:")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .expect("a ccr marker");
        assert_eq!(store.get(hash).as_deref(), Some(long.as_str()));

        // The total cap divides among lanes when it is the tighter one, and a
        // stopped lane is flagged before its (partial) text.
        let mut many: Vec<SubagentOutcome> =
            (0..6).map(|i| outcome(&format!("l{i}"), &long)).collect();
        many[0].stopped = Some(LaneStop::TimedOut(Duration::from_secs(600)));
        let doc = combine_outcomes_capped(&many, "agent", ResultCap::default_with(None));
        assert!(doc.contains("reply cut at 8000 of 30000 chars"));
        assert!(
            doc.contains("was stopped after 600s (its time limit); what follows may be partial")
        );
        assert!(doc.chars().count() < FLEET_RESULT_CHARS + 2_000);

        // The uncapped form is untouched: the review pipeline reads whole replies.
        assert!(combine_outcomes(&outcomes, "reviewer").contains(&long));
    }

    #[tokio::test]
    async fn empty_task_list_returns_no_outcomes() {
        let base = base_agent("http://127.0.0.1:1/api/ai");
        let outcomes = run_fleet(
            |_| base.side_agent(),
            Vec::new(),
            FleetLimits::with_concurrency(4),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(outcomes.is_empty());
    }
}
