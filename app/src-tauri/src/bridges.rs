//! The host↔agent bridges: agent-side capabilities that need the window —
//! `ask_user_question`, `canvas`, `spawn_agents` lanes — implemented as
//! webview event emitters, plus the inert `Null*` stand-ins the settings
//! registry uses when it only reads names/descriptions/schemas and must never
//! block on a real bridge.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use harness_agent::AgentEvent;
use harness_tools::{CanvasDoc, CanvasSink, Question, QuestionAnswer, QuestionAsker, ToolError};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::events::{
    CanvasPayload, FleetActivityPayload, FleetAgentPayload, FleetStartedPayload, QuestionPayload,
    SessionPayload,
};
use crate::state::Pending;

/// Bridges the agent's `ask_user_question` tool to the web UI: emits an
/// `agent://question` event and parks on a channel until `answer_question`
/// delivers the user's selection (or the channel is dropped → no answer).
pub(crate) struct TauriAsker {
    pub(crate) app: AppHandle,
    pub(crate) pending: Pending,
}

#[async_trait]
impl QuestionAsker for TauriAsker {
    async fn ask(&self, questions: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = format!("q{}", COUNTER.fetch_add(1, Ordering::Relaxed));

        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .insert(id.clone(), tx);

        self.app
            .emit(
                "agent://question",
                QuestionPayload {
                    id: id.clone(),
                    questions: questions.to_vec(),
                },
            )
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        match rx.await {
            Ok(answers) => Ok(Some(answers)),
            Err(_) => {
                self.pending
                    .lock()
                    .expect("pending mutex poisoned")
                    .remove(&id);
                Ok(None)
            }
        }
    }
}

/// Bridges the agent's `canvas` tool to the desktop side panel: emits an
/// `agent://canvas` event with the document. One sink per agent, so it carries
/// that agent's session id.
pub(crate) struct TauriCanvasSink {
    pub(crate) app: AppHandle,
    pub(crate) session: String,
}

#[async_trait]
impl CanvasSink for TauriCanvasSink {
    async fn show(&self, doc: &CanvasDoc) -> Result<Option<String>, ToolError> {
        self.app
            .emit(
                "agent://canvas",
                CanvasPayload {
                    session: self.session.clone(),
                    id: doc.id.clone(),
                    title: doc.title.clone(),
                    format: doc.format.clone(),
                    language: doc.language.clone(),
                    content: doc.content.clone(),
                },
            )
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        // The panel itself is the user-visible result; no extra note needed.
        Ok(None)
    }
}

/// Bridges a `spawn_agents` fleet (run by the model from inside a turn) to the
/// UI's lanes panel: one sink per agent, tagged with its session, emitting the
/// same `fleet://` events review fan-out steps use.
pub(crate) struct TauriFleetSink {
    pub(crate) app: AppHandle,
    pub(crate) session: String,
}

impl harness_agent::fleet::FleetSink for TauriFleetSink {
    fn started(&self, labels: &[String], _cancel: CancellationToken) {
        let _ = self.app.emit(
            "fleet://started",
            FleetStartedPayload {
                session: self.session.clone(),
                agents: labels.to_vec(),
                source: "turn",
            },
        );
    }

    fn event(&self, event: &harness_agent::fleet::FleetEvent) {
        emit_fleet_event(&self.app, &self.session, event);
    }

    fn finished(&self) {
        let _ = self.app.emit(
            "fleet://completed",
            SessionPayload {
                session: self.session.clone(),
            },
        );
    }
}

/// Emit one fleet lane event as the `fleet://` webview payloads the panel
/// consumes. The single translation from [`FleetEvent`] to the wire — shared
/// by [`TauriFleetSink`] (a `spawn_agents` fleet) and `run_code_review`'s
/// fan-out steps, which map their `ReviewEvent::Subagent*` into `FleetEvent`
/// and route here, so the two surfaces can't drift on the wire format.
///
/// [`FleetEvent`]: harness_agent::fleet::FleetEvent
pub(crate) fn emit_fleet_event(
    app: &AppHandle,
    session: &str,
    event: &harness_agent::fleet::FleetEvent,
) {
    use harness_agent::fleet::FleetEvent;
    match event {
        FleetEvent::TaskStarted { index, label } => {
            let _ = app.emit(
                "fleet://agent",
                FleetAgentPayload {
                    session: session.to_string(),
                    agent: *index,
                    name: label.clone(),
                    phase: "started",
                    tokens: 0,
                    summary: String::new(),
                },
            );
        }
        FleetEvent::Agent { index, event } => {
            if let Some((kind, text, tokens)) = activity_payload(event) {
                let _ = app.emit(
                    "fleet://agent-activity",
                    FleetActivityPayload {
                        session: session.to_string(),
                        agent: *index,
                        kind,
                        text,
                        tokens,
                    },
                );
            }
        }
        FleetEvent::TaskCompleted {
            index,
            label,
            ok,
            tokens_used,
            summary,
        } => {
            let _ = app.emit(
                "fleet://agent",
                FleetAgentPayload {
                    session: session.to_string(),
                    agent: *index,
                    name: label.clone(),
                    phase: if *ok { "done" } else { "failed" },
                    tokens: *tokens_used,
                    summary: summary.clone(),
                },
            );
        }
    }
}

/// The lane-activity slice of one subagent event, if it has one.
pub(crate) fn activity_payload(
    event: &AgentEvent,
) -> Option<(&'static str, String, Option<usize>)> {
    match event {
        AgentEvent::Token(t) => Some(("token", t.clone(), None)),
        AgentEvent::ToolStart { name, .. } => Some(("tool", name.clone(), None)),
        AgentEvent::Usage { tokens_used, .. } => {
            Some(("tokens", String::new(), Some(*tokens_used)))
        }
        _ => None,
    }
}

/// An inert question bridge for the settings registry — never invoked; only the
/// tool's name/description/schema are read.
pub(crate) struct NullAsker;

#[async_trait::async_trait]
impl harness_tools::QuestionAsker for NullAsker {
    async fn ask(
        &self,
        _: &[harness_tools::Question],
    ) -> Result<Option<Vec<harness_tools::QuestionAnswer>>, harness_tools::ToolError> {
        Ok(None)
    }
}

/// An inert canvas bridge for the settings registry — never invoked.
pub(crate) struct NullCanvasSink;

#[async_trait::async_trait]
impl harness_tools::CanvasSink for NullCanvasSink {
    async fn show(
        &self,
        _: &harness_tools::CanvasDoc,
    ) -> Result<Option<String>, harness_tools::ToolError> {
        Ok(None)
    }
}

/// Inert fleet sink for [`crate::state::settings_registry`]'s listing-only
/// `spawn_agents`.
pub(crate) struct NullFleetSink;

impl harness_agent::fleet::FleetSink for NullFleetSink {
    fn started(&self, _labels: &[String], _cancel: CancellationToken) {}
    fn event(&self, _event: &harness_agent::fleet::FleetEvent) {}
    fn finished(&self) {}
}
