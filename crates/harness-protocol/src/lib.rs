//! The transport-neutral wire protocol every oxen-harness UI speaks.
//!
//! One tagged [`ProtocolEvent`] enum carries everything a host streams to a
//! client (assistant tokens, tool activity, approval prompts, fleet lanes,
//! preview status), and the DTO structs carry command requests/responses. The
//! desktop webview, the HTTP server's SSE stream, and any third-party UI all
//! consume these exact shapes — the wire tests in `tests/wire.rs` are the
//! spec, and `schemars` derives make the protocol self-describing for
//! generating TypeScript types.
//!
//! This crate deliberately depends only on serde: converting from the richer
//! in-process types (`harness_agent::AgentEvent`, `harness_permissions::
//! ApprovalRequest`, …) is the host layer's job, so a protocol consumer never
//! drags in the agent stack.

mod dto;
mod event;

pub use dto::{
    ApprovalAnswer, Choice, InterjectRequest, InterjectResponse, LoopResult, Question,
    QuestionAnswer, ReviewResult, SessionInfo, SessionView, TurnRequest, TurnResponse,
};
pub use event::{
    ApprovalKind, ApprovalPhase, FleetActivityKind, FleetAgentPhase, FleetSource, LocalPhase,
    PreviewPhase, ProtocolEvent, ToolPhase,
};
