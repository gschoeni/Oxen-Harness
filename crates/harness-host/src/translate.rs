//! Translation from in-process types (`harness_agent::AgentEvent`,
//! `harness_permissions`, `harness_preview`, `harness_tools`) onto the
//! transport-neutral `harness_protocol` shapes — the single place the two
//! vocabularies meet, so hosts can't drift on the wire format.

use harness_agent::fleet::FleetEvent;
use harness_agent::AgentEvent;
use harness_protocol::ProtocolEvent;

/// One agent event as its protocol shape, tagged with its session.
///
/// `context_window` fills in the usage event (the core reports fill, the wire
/// reports fill *and* capacity). Returns `None` for events with no wire
/// representation (a non-canvas `ToolPending` — the UI reacts to `ToolStart`).
/// Token events translate too; hosts that batch tokens intercept them before
/// calling this.
pub fn agent_event(
    session: &str,
    context_window: usize,
    event: &AgentEvent,
) -> Option<ProtocolEvent> {
    let session = session.to_string();
    Some(match event {
        AgentEvent::Token(token) => ProtocolEvent::Token {
            session,
            token: token.clone(),
        },
        // The model started writing a canvas; the panel opens in a "writing"
        // state while content streams in as tool-delta events.
        AgentEvent::ToolPending { name } if name == harness_tools::CANVAS_TOOL => {
            ProtocolEvent::CanvasWriting { session }
        }
        AgentEvent::ToolPending { .. } => return None,
        AgentEvent::ToolDelta { name, delta } => ProtocolEvent::ToolDelta {
            session,
            name: name.clone(),
            delta: delta.clone(),
        },
        AgentEvent::ToolStart { name, arguments } => ProtocolEvent::Tool {
            session,
            phase: harness_protocol::ToolPhase::Start,
            name: name.clone(),
            detail: arguments.clone(),
        },
        AgentEvent::ToolEnd { name, result } => ProtocolEvent::Tool {
            session,
            phase: harness_protocol::ToolPhase::End,
            name: name.clone(),
            detail: result.clone(),
        },
        AgentEvent::Usage {
            tokens_used,
            context_tokens,
            prompt_tokens_used,
            completion_tokens_used,
        } => ProtocolEvent::Usage {
            session,
            tokens_used: *tokens_used,
            context_tokens: *context_tokens,
            context_window,
            prompt_tokens_used: *prompt_tokens_used,
            completion_tokens_used: *completion_tokens_used,
        },
        AgentEvent::Compacted { detail } => ProtocolEvent::Compacted {
            session,
            detail: detail.clone(),
        },
        AgentEvent::ApprovalPending { name, command } => ProtocolEvent::Approval {
            session,
            phase: harness_protocol::ApprovalPhase::Pending,
            name: name.clone(),
            command: command.clone(),
            decision: String::new(),
        },
        AgentEvent::ApprovalResolved {
            name,
            command,
            decision,
        } => ProtocolEvent::Approval {
            session,
            phase: harness_protocol::ApprovalPhase::Resolved,
            name: name.clone(),
            command: command.clone(),
            decision: decision.clone(),
        },
        AgentEvent::Retrying {
            attempt,
            max_attempts,
            delay_ms,
            error,
        } => ProtocolEvent::Retry {
            session,
            attempt: *attempt,
            max_attempts: *max_attempts,
            delay_ms: *delay_ms,
            error: error.clone(),
        },
        AgentEvent::Compression {
            mode,
            saved_tokens,
            total_saved_tokens,
            results_compressed,
        } => ProtocolEvent::Compression {
            session,
            mode: mode.clone(),
            saved_tokens: *saved_tokens,
            total_saved_tokens: *total_saved_tokens,
            results_compressed: *results_compressed,
        },
    })
}

/// One fleet lane event as its protocol shape. `None` for lane agent events
/// with no lane-activity slice (deltas, compaction notices, …).
pub fn fleet_event(session: &str, event: &FleetEvent) -> Option<ProtocolEvent> {
    let session = session.to_string();
    Some(match event {
        FleetEvent::TaskStarted { index, label } => ProtocolEvent::FleetAgent {
            session,
            agent: *index,
            name: label.clone(),
            phase: harness_protocol::FleetAgentPhase::Started,
            tokens: 0,
            summary: String::new(),
        },
        FleetEvent::Agent { index, event } => {
            let (kind, text, tokens) = match event.as_ref() {
                AgentEvent::Token(t) => {
                    (harness_protocol::FleetActivityKind::Token, t.clone(), None)
                }
                AgentEvent::ToolStart { name, .. } => (
                    harness_protocol::FleetActivityKind::Tool,
                    name.clone(),
                    None,
                ),
                AgentEvent::Usage { tokens_used, .. } => (
                    harness_protocol::FleetActivityKind::Tokens,
                    String::new(),
                    Some(*tokens_used),
                ),
                _ => return None,
            };
            ProtocolEvent::FleetActivity {
                session,
                agent: *index,
                kind,
                text,
                tokens,
            }
        }
        FleetEvent::TaskCompleted {
            index,
            label,
            ok,
            tokens_used,
            summary,
        } => ProtocolEvent::FleetAgent {
            session,
            agent: *index,
            name: label.clone(),
            phase: if *ok {
                harness_protocol::FleetAgentPhase::Done
            } else {
                harness_protocol::FleetAgentPhase::Failed
            },
            tokens: *tokens_used,
            summary: summary.clone(),
        },
    })
}

/// A model-facing question as its protocol shape.
pub fn question(q: &harness_tools::Question) -> harness_protocol::Question {
    harness_protocol::Question {
        question: q.question.clone(),
        header: q.header.clone(),
        options: q
            .options
            .iter()
            .map(|c| harness_protocol::Choice {
                label: c.label.clone(),
                description: c.description.clone(),
            })
            .collect(),
        multi_select: q.multi_select,
    }
}

/// A client's answer as the tool-facing shape.
pub fn question_answer(a: harness_protocol::QuestionAnswer) -> harness_tools::QuestionAnswer {
    harness_tools::QuestionAnswer {
        header: a.header,
        question: a.question,
        selected: a.selected,
    }
}

/// A client's approval keyword as the gate's decision. Unknown keywords read
/// as a denial (with the client's message when present).
pub fn approval_decision(
    answer: harness_protocol::ApprovalAnswer,
) -> harness_permissions::ApprovalDecision {
    use harness_permissions::ApprovalDecision;
    match answer.decision.as_str() {
        "once" => ApprovalDecision::AllowOnce,
        "session" => ApprovalDecision::AllowSession,
        "project" => ApprovalDecision::AllowProject,
        "trash" => ApprovalDecision::MoveToTrash,
        "bypass" => ApprovalDecision::AllowAllBypass,
        _ => match answer.message {
            Some(msg) if !msg.trim().is_empty() => ApprovalDecision::DenyWithMessage(msg),
            _ => ApprovalDecision::Deny,
        },
    }
}

/// The gate's approval kind as its protocol shape.
pub fn approval_kind(kind: harness_permissions::ApprovalKind) -> harness_protocol::ApprovalKind {
    match kind {
        harness_permissions::ApprovalKind::Shell => harness_protocol::ApprovalKind::Shell,
        harness_permissions::ApprovalKind::FileEdit => harness_protocol::ApprovalKind::FileEdit,
        harness_permissions::ApprovalKind::GitCommit => harness_protocol::ApprovalKind::GitCommit,
        harness_permissions::ApprovalKind::TaskKill => harness_protocol::ApprovalKind::TaskKill,
    }
}

/// A dev server's lifecycle phase as its protocol shape.
pub fn preview_phase(phase: harness_preview::PreviewPhase) -> harness_protocol::PreviewPhase {
    match phase {
        harness_preview::PreviewPhase::Starting => harness_protocol::PreviewPhase::Starting,
        harness_preview::PreviewPhase::Ready => harness_protocol::PreviewPhase::Ready,
        harness_preview::PreviewPhase::Error => harness_protocol::PreviewPhase::Error,
        harness_preview::PreviewPhase::Stopped => harness_protocol::PreviewPhase::Stopped,
    }
}

/// A dev-server status snapshot as its protocol event.
pub fn preview_status(
    session: &str,
    status: &harness_preview::PreviewStatus,
) -> ProtocolEvent {
    ProtocolEvent::PreviewStatus {
        session: session.to_string(),
        phase: preview_phase(status.phase),
        name: status.name.clone(),
        command: status.command.clone(),
        url: status.url.clone(),
        port: status.port,
        message: status.message.clone(),
    }
}

/// A local model load phase as its protocol shape.
pub fn local_phase(phase: harness_local::LoadPhase) -> harness_protocol::LocalPhase {
    match phase {
        harness_local::LoadPhase::Starting => harness_protocol::LocalPhase::Starting,
        harness_local::LoadPhase::LoadingModel => harness_protocol::LocalPhase::Loading,
        harness_local::LoadPhase::Ready => harness_protocol::LocalPhase::Ready,
    }
}
