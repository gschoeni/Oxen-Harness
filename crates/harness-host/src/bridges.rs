//! The host↔agent bridges, generic over [`EventSink`]: the agent-side
//! capabilities that need a client surface (`ask_user_question`, `canvas`,
//! `open_file`, `spawn_agents` lanes, permission approvals) implemented as
//! protocol-event emitters plus id-keyed oneshot round-trips — and the inert
//! `Null*` stand-ins a settings/listing registry uses when it only reads
//! names/descriptions/schemas and must never block on a real bridge.

use std::sync::Arc;

use async_trait::async_trait;
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

/// Bridges a `spawn_agents` fleet (run by the model from inside a turn) to the
/// client's lanes panel, emitting the same `fleet.*` events review fan-out
/// steps use.
pub struct HostFleetSink {
    pub sink: Arc<dyn EventSink>,
    pub session: String,
    pub source: harness_protocol::FleetSource,
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
        if let Some(event) = translate::fleet_event(&self.session, event) {
            self.sink.emit(event);
        }
    }

    fn finished(&self) {
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
        Err("preview screenshots aren't supported on this host — verify with \
             dev_server_logs and preview_console instead"
            .to_string())
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
