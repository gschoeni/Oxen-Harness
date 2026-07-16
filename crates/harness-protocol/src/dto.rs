//! Command request/response shapes shared by every host transport: the Tauri
//! invoke layer, the HTTP server's routes, and client SDKs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One selectable choice within a [`Question`]. Serde-compatible with
/// `harness_tools::Choice` (pinned by a wire test).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Choice {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// A structured question the model asked via `ask_user_question`.
/// Serde-compatible with `harness_tools::Question`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub header: String,
    pub options: Vec<Choice>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

/// The user's answer to one [`Question`]. Serde-compatible with
/// `harness_tools::QuestionAnswer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionAnswer {
    /// The question's `header`, echoed back for context.
    pub header: String,
    /// The question text, echoed back for context.
    pub question: String,
    /// The selected option label(s), or the user's free-text answer.
    pub selected: Vec<String>,
}

/// The user's reply to one approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalAnswer {
    /// "once" | "session" | "project" | "trash" | "bypass" | "deny".
    pub decision: String,
    /// The user's own words when denying (sent back to the model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A session's live vitals — what a UI needs to render its header/meters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInfo {
    pub model: String,
    pub workspace: String,
    pub session_id: String,
    /// Cumulative tokens used in this session.
    pub tokens_used: usize,
    /// Tokens the current transcript occupies (context-window fill).
    pub context_tokens: usize,
    /// The model's effective context window.
    pub context_window: usize,
    /// The context-compression mode this session's agent runs with
    /// ("off"/"audit"/"on").
    pub compression_mode: String,
}

/// A resumed session: its info plus the verbatim transcript to re-render.
/// When `running` is true the chat is mid-turn and couldn't be read;
/// `messages` is empty and the client keeps whatever it already streamed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionView {
    pub info: SessionInfo,
    pub messages: Vec<serde_json::Value>,
    pub running: bool,
}

/// A request to run one user turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnRequest {
    pub prompt: String,
    /// Paths of attachments readable by the host (dropped files on the
    /// desktop; upload-endpoint results over HTTP).
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// A completed turn's final assistant text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnResponse {
    pub text: String,
}

/// A message for the session's *running* turn (mid-turn steering): delivered
/// into the turn at its next safe point rather than queued for after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InterjectRequest {
    pub text: String,
}

/// Whether a running turn accepted the interjection. `accepted: false` means
/// no turn was in flight — send the text as an ordinary prompt instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InterjectResponse {
    pub accepted: bool,
}

/// What a code-review run resolved to. `status` is `"ok"`, `"nothing"` (the
/// target had no changes), or `"cancelled"`; on `"ok"` the user/assistant
/// pair is already persisted to the session, so the client appends it to the
/// thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewResult {
    pub status: String,
    pub user: String,
    pub assistant: String,
    pub findings: usize,
    /// Estimated tokens spent across every reviewer agent in the pipeline.
    pub tokens_used: usize,
}

/// What a verification-loop run resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoopResult {
    pub succeeded: bool,
    pub iterations: u32,
    pub summary: String,
}
