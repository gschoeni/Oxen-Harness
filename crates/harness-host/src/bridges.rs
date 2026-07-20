//! The host↔agent bridges, generic over [`EventSink`]: the agent-side
//! capabilities that need a client surface (`ask_user_question`, `canvas`,
//! `open_file`, `spawn_agents` lanes, permission approvals) implemented as
//! protocol-event emitters plus id-keyed oneshot round-trips — and the inert
//! `Null*` stand-ins a settings/listing registry uses when it only reads
//! names/descriptions/schemas and must never block on a real bridge.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use harness_agent::fleet::FleetEvent;
use harness_agent::AgentEvent;
use harness_preview::console::ConsoleLine;
use harness_preview::{PreviewLens, PreviewSink, PreviewStatus};
use harness_protocol::ProtocolEvent;
use harness_tools::{CanvasDoc, CanvasSink, Question, QuestionAnswer, QuestionAsker, ToolError};
use tokio_util::sync::CancellationToken;

use crate::{translate, EventSink, PendingApprovals, PendingQuestions};

/// Bridges the agent's `ask_user_question` tool to the client: emits an
/// `agent.question` event and parks until the host's answer-question command
/// delivers the user's selection (or the entry is dropped → no answer).
pub struct HostAsker {
    pub sink: Arc<dyn EventSink>,
    pub session: String,
    pub pending: PendingQuestions,
}

#[async_trait]
impl QuestionAsker for HostAsker {
    async fn ask(&self, questions: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
        let (id, rx) = self.pending.register("q");
        self.sink.emit(ProtocolEvent::Question {
            session: self.session.clone(),
            id: id.clone(),
            questions: questions.iter().map(translate::question).collect(),
        });
        match rx.await {
            Ok(answers) => Ok(Some(answers)),
            Err(_) => {
                self.pending.forget(&id);
                Ok(None)
            }
        }
    }
}

/// Bridges the permission gate's approval prompts to the client: emits an
/// `agent.approval_request` event and parks until the host's answer-approval
/// command delivers the decision. A dropped entry (chat evicted, client gone)
/// reads as "no interactive user" and the gate declines the command.
pub struct HostApprover {
    pub sink: Arc<dyn EventSink>,
    pub session: String,
    pub pending: PendingApprovals,
}

#[async_trait]
impl harness_permissions::CommandApprover for HostApprover {
    async fn approve(
        &self,
        request: &harness_permissions::ApprovalRequest,
    ) -> Result<Option<harness_permissions::ApprovalDecision>, String> {
        let (id, rx) = self.pending.register("a");
        self.sink.emit(ProtocolEvent::ApprovalRequest {
            session: self.session.clone(),
            id: id.clone(),
            kind: translate::approval_kind(request.kind),
            tool: request.tool.clone(),
            command: request.command.clone(),
            risk: request.risk.label().to_string(),
            reasons: request.reasons.clone(),
            grant_label: request.grant_label.clone(),
            offer_project_grant: request.offer_project_grant,
            offer_trash: request.offer_trash,
        });
        match rx.await {
            Ok(answer) => Ok(Some(translate::approval_decision(answer))),
            Err(_) => {
                self.pending.forget(&id);
                Ok(None)
            }
        }
    }
}

/// Bridges the agent's `canvas` tool to the client's side panel. One sink per
/// agent, so it carries that agent's session id.
pub struct HostCanvasSink {
    pub sink: Arc<dyn EventSink>,
    pub session: String,
}

#[async_trait]
impl CanvasSink for HostCanvasSink {
    async fn show(&self, doc: &CanvasDoc) -> Result<Option<String>, ToolError> {
        self.sink.emit(ProtocolEvent::Canvas {
            session: self.session.clone(),
            id: doc.id.clone(),
            title: doc.title.clone(),
            format: doc.format.clone(),
            language: doc.language.clone(),
            content: doc.content.clone(),
        });
        // The panel itself is the user-visible result; no extra note needed.
        Ok(None)
    }
}

/// Bridges the agent's `open_file` tool to the client's editor/viewer surface.
pub struct HostViewerSink {
    pub sink: Arc<dyn EventSink>,
    pub session: String,
}

#[async_trait]
impl harness_tools::ViewerSink for HostViewerSink {
    async fn open(&self, view: &harness_tools::FileView) -> Result<Option<String>, ToolError> {
        self.sink.emit(ProtocolEvent::OpenFile {
            session: self.session.clone(),
            paths: view.paths.clone(),
        });
        Ok(None)
    }
}

/// Coalesces the token slice of fleet lane events before they hit the wire:
/// N parallel agents each streaming tokens is N × dozens of events per second,
/// and every one is an IPC hop plus a client render. Tokens buffer per lane,
/// flushing at [`crate::service::STREAM_BATCH_BYTES`], at
/// [`crate::service::STREAM_BATCH_MAX_AGE`] (so a slow-trickling lane still
/// reads as alive), or before any other event from the same lane — within-lane
/// ordering is what matters; lanes are mutually independent. The lanes panel
/// renders a rolling tail by pure concatenation, so coalescing is lossless.
pub(crate) struct FleetActivityBatch {
    sink: Arc<dyn EventSink>,
    session: String,
    lanes: StdMutex<HashMap<usize, LaneRun>>,
}

/// One lane's buffered tokens, stamped with when the run started.
struct LaneRun {
    text: String,
    since: std::time::Instant,
}

impl FleetActivityBatch {
    pub(crate) fn new(sink: Arc<dyn EventSink>, session: String) -> Self {
        Self {
            sink,
            session,
            lanes: StdMutex::new(HashMap::new()),
        }
    }

    /// Route one lane event: tokens coalesce, everything else flushes its lane
    /// and forwards through the usual translation.
    pub(crate) fn forward(&self, event: &FleetEvent) {
        match event {
            FleetEvent::Agent {
                index,
                event: agent_event,
            } => {
                if let AgentEvent::Token(t) = agent_event.as_ref() {
                    let ready = {
                        let mut lanes = self.lanes.lock().expect("fleet batch poisoned");
                        let run = lanes.entry(*index).or_insert_with(|| LaneRun {
                            text: String::new(),
                            since: std::time::Instant::now(),
                        });
                        run.text.push_str(t);
                        (run.text.len() >= crate::service::STREAM_BATCH_BYTES
                            || run.since.elapsed() >= crate::service::STREAM_BATCH_MAX_AGE)
                            .then(|| lanes.remove(index))
                            .flatten()
                    };
                    if let Some(run) = ready {
                        self.emit(*index, run.text);
                    }
                    return;
                }
                self.flush_lane(*index);
            }
            FleetEvent::TaskStarted { index, .. } | FleetEvent::TaskCompleted { index, .. } => {
                self.flush_lane(*index);
            }
        }
        if let Some(event) = translate::fleet_event(&self.session, event) {
            self.sink.emit(event);
        }
    }

    fn flush_lane(&self, index: usize) {
        let buffered = self
            .lanes
            .lock()
            .expect("fleet batch poisoned")
            .remove(&index);
        if let Some(run) = buffered {
            self.emit(index, run.text);
        }
    }

    pub(crate) fn flush_all(&self) {
        let lanes = std::mem::take(&mut *self.lanes.lock().expect("fleet batch poisoned"));
        for (index, run) in lanes {
            self.emit(index, run.text);
        }
    }

    fn emit(&self, index: usize, text: String) {
        if text.is_empty() {
            return;
        }
        // One FleetEvent→ProtocolEvent mapping: a coalesced run rides through
        // `translate` as a synthetic token event, exactly like the unbatched
        // path, so the two encodings can't drift on the wire format.
        let event = FleetEvent::Agent {
            index,
            event: Arc::new(AgentEvent::Token(text)),
        };
        if let Some(event) = translate::fleet_event(&self.session, &event) {
            self.sink.emit(event);
        }
    }
}

/// Bridges a `spawn_agents` fleet (run by the model from inside a turn) to the
/// client's lanes panel, emitting the same `fleet.*` events review fan-out
/// steps use. Lane tokens coalesce through a `FleetActivityBatch`.
pub struct HostFleetSink {
    sink: Arc<dyn EventSink>,
    session: String,
    source: harness_protocol::FleetSource,
    batch: FleetActivityBatch,
}

impl HostFleetSink {
    pub fn new(
        sink: Arc<dyn EventSink>,
        session: String,
        source: harness_protocol::FleetSource,
    ) -> Self {
        Self {
            batch: FleetActivityBatch::new(sink.clone(), session.clone()),
            sink,
            session,
            source,
        }
    }
}

impl harness_agent::fleet::FleetSink for HostFleetSink {
    fn started(&self, labels: &[String], _cancel: CancellationToken) {
        self.sink.emit(ProtocolEvent::FleetStarted {
            session: self.session.clone(),
            agents: labels.to_vec(),
            source: self.source,
        });
    }

    fn event(&self, event: &harness_agent::fleet::FleetEvent) {
        self.batch.forward(event);
    }

    fn finished(&self) {
        self.batch.flush_all();
        self.sink.emit(ProtocolEvent::FleetCompleted {
            session: self.session.clone(),
        });
    }
}

/// The default dev-server lifecycle sink: statuses go straight onto the
/// protocol stream. Hosts with native preview surfaces (the desktop's child
/// webview) wrap or replace it via [`crate::HostHooks`].
pub struct ProtocolPreviewSink {
    pub sink: Arc<dyn EventSink>,
    pub session: String,
}

impl PreviewSink for ProtocolPreviewSink {
    fn status(&self, status: &PreviewStatus) {
        self.sink
            .emit(translate::preview_status(&self.session, status));
    }
}

/// The default preview lens for hosts without a screenshot surface: the agent
/// is told to verify through logs/console instead.
pub struct NoScreenshotLens;

#[async_trait]
impl PreviewLens for NoScreenshotLens {
    async fn screenshot(&self) -> Result<Vec<u8>, String> {
        Err(
            "preview screenshots aren't supported on this host — verify with \
             dev_server_logs and preview_console instead"
                .to_string(),
        )
    }
    fn console_tail(&self, _n: usize) -> Vec<ConsoleLine> {
        Vec::new()
    }
}

// --- Inert stand-ins for listing-only registries ----------------------------

/// An inert question bridge — never invoked; only the tool's
/// name/description/schema are read.
pub struct NullAsker;

#[async_trait]
impl QuestionAsker for NullAsker {
    async fn ask(&self, _: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
        Ok(None)
    }
}

/// An inert canvas bridge — never invoked.
pub struct NullCanvasSink;

#[async_trait]
impl CanvasSink for NullCanvasSink {
    async fn show(&self, _: &CanvasDoc) -> Result<Option<String>, ToolError> {
        Ok(None)
    }
}

/// An inert viewer bridge — never invoked.
pub struct NullViewerSink;

#[async_trait]
impl harness_tools::ViewerSink for NullViewerSink {
    async fn open(&self, _: &harness_tools::FileView) -> Result<Option<String>, ToolError> {
        Ok(None)
    }
}

/// An inert fleet sink — never invoked.
pub struct NullFleetSink;

impl harness_agent::fleet::FleetSink for NullFleetSink {
    fn started(&self, _labels: &[String], _cancel: CancellationToken) {}
    fn event(&self, _event: &harness_agent::fleet::FleetEvent) {}
    fn finished(&self) {}
}

/// An inert preview sink — never invoked.
pub struct NullPreviewSink;

impl PreviewSink for NullPreviewSink {
    fn status(&self, _status: &PreviewStatus) {}
}

/// An inert preview lens — never invoked.
pub struct NullPreviewLens;

#[async_trait]
impl PreviewLens for NullPreviewLens {
    async fn screenshot(&self) -> Result<Vec<u8>, String> {
        Err("not available".into())
    }
    fn console_tail(&self, _n: usize) -> Vec<ConsoleLine> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSink(StdMutex<Vec<ProtocolEvent>>);

    impl EventSink for RecordingSink {
        fn emit(&self, event: ProtocolEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn lane_token(index: usize, text: &str) -> FleetEvent {
        FleetEvent::Agent {
            index,
            event: Arc::new(AgentEvent::Token(text.to_string())),
        }
    }

    #[test]
    fn fleet_batch_coalesces_tokens_per_lane_and_flushes_on_lane_events() {
        let sink = Arc::new(RecordingSink::default());
        let batch = FleetActivityBatch::new(sink.clone(), "s1".into());

        // Interleaved lanes buffer independently — no cross-lane flushing.
        batch.forward(&lane_token(0, "alpha "));
        batch.forward(&lane_token(1, "beta "));
        batch.forward(&lane_token(0, "one"));
        batch.forward(&lane_token(1, "two"));
        assert!(sink.0.lock().unwrap().is_empty());

        // A lane's completion flushes that lane's tokens first, in order.
        batch.forward(&FleetEvent::TaskCompleted {
            index: 0,
            label: "a".into(),
            ok: true,
            tokens_used: 10,
            summary: "done".into(),
        });
        {
            let events = sink.0.lock().unwrap();
            match &events[..] {
                [ProtocolEvent::FleetActivity { agent, text, .. }, ProtocolEvent::FleetAgent { agent: done, .. }] =>
                {
                    assert_eq!((*agent, text.as_str()), (0, "alpha one"));
                    assert_eq!(*done, 0);
                }
                other => panic!("expected lane-0 flush then completion, got {other:?}"),
            }
        }

        // The other lane's buffer survives until the final flush.
        batch.flush_all();
        let events = sink.0.lock().unwrap();
        match events.last() {
            Some(ProtocolEvent::FleetActivity { agent, text, .. }) => {
                assert_eq!((*agent, text.as_str()), (1, "beta two"));
            }
            other => panic!("expected lane-1 tail, got {other:?}"),
        }
    }

    #[test]
    fn fleet_batch_flushes_an_aged_lane_so_slow_lanes_stay_live() {
        let sink = Arc::new(RecordingSink::default());
        let batch = FleetActivityBatch::new(sink.clone(), "s1".into());

        batch.forward(&lane_token(0, "slow "));
        std::thread::sleep(
            crate::service::STREAM_BATCH_MAX_AGE + std::time::Duration::from_millis(10),
        );
        // Under the byte threshold, but the run is old — the next token ships
        // it so a trickling lane's ticker keeps moving.
        batch.forward(&lane_token(0, "drip"));
        let events = sink.0.lock().unwrap();
        match &events[..] {
            [ProtocolEvent::FleetActivity { agent, text, .. }] => {
                assert_eq!((*agent, text.as_str()), (0, "slow drip"));
            }
            other => panic!("expected the aged lane to flush, got {other:?}"),
        }
    }
}
