//! Every event a host streams to a client, as one internally-tagged enum.
//!
//! The `type` tag is dotted (`agent.token`, `fleet.started`); the desktop's
//! legacy Tauri channel names derive mechanically from it via [`ProtocolEvent::
//! channel`] (`agent.tool_delta` → `agent://tool-delta`). Field names and the
//! string values of the phase/kind enums are pinned to what the desktop
//! frontend already parses — see `tests/wire.rs` before changing anything.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dto::Question;

/// Which side of a tool invocation an `agent.tool` event marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolPhase {
    Start,
    End,
}

/// Whether an `agent.approval` thread marker is awaiting or carrying a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPhase {
    Pending,
    Resolved,
}

/// What kind of gated action an approval request covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Shell,
    FileEdit,
    GitCommit,
    TaskKill,
}

/// What launched a fleet: a code-review pipeline step or the model's
/// `spawn_agents` call inside a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FleetSource {
    Review,
    Turn,
}

/// One fleet lane's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FleetAgentPhase {
    Started,
    Done,
    Failed,
}

/// What a `fleet.agent_activity` event carries: streamed text to append, a
/// tool line to replace the lane's status with, or a token-counter update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FleetActivityKind {
    Token,
    Tool,
    Tokens,
}

/// A dev server's lifecycle phase (mirrors `harness_preview::PreviewPhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreviewPhase {
    Starting,
    Ready,
    Error,
    Stopped,
}

/// A local model's load phase while its server comes online.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LocalPhase {
    Starting,
    Loading,
    Ready,
    Error,
}

/// Everything a host streams to a client. Session-scoped variants carry the
/// session id so one stream can multiplex every chat (background chats
/// included); [`ProtocolEvent::session`] exposes it for routing/filtering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum ProtocolEvent {
    // --- Turn streaming ------------------------------------------------------
    /// Incremental assistant text.
    #[serde(rename = "agent.token")]
    Token { session: String, token: String },
    /// A tool call started (detail = its JSON arguments) or finished
    /// (detail = its display-truncated result).
    #[serde(rename = "agent.tool")]
    Tool {
        session: String,
        phase: ToolPhase,
        name: String,
        detail: String,
    },
    /// An incremental fragment of a tool call's JSON arguments, so a UI can
    /// stream in-progress content (a file being written, a canvas document).
    #[serde(rename = "agent.tool_delta")]
    ToolDelta {
        session: String,
        name: String,
        delta: String,
    },
    /// Live token usage, emitted around each model call within a turn.
    #[serde(rename = "agent.usage")]
    Usage {
        session: String,
        tokens_used: usize,
        context_tokens: usize,
        context_window: usize,
        prompt_tokens_used: usize,
        completion_tokens_used: usize,
    },
    /// The transcript was compacted to fit the context window.
    #[serde(rename = "agent.compacted")]
    Compacted { session: String, detail: String },
    /// A transient model-call failure is being retried with backoff.
    #[serde(rename = "agent.retry")]
    Retry {
        session: String,
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error: String,
    },
    /// Stale tool output was compressed (or its savings measured, in audit
    /// mode) before a model call.
    #[serde(rename = "agent.compression")]
    Compression {
        session: String,
        mode: String,
        saved_tokens: usize,
        total_saved_tokens: usize,
        results_compressed: usize,
    },

    // --- Turn lifecycle ------------------------------------------------------
    /// A turn began running. HTTP clients that fire-and-forget their turn POST
    /// use these lifecycle events to track the turn on the stream alone.
    #[serde(rename = "turn.started")]
    TurnStarted { session: String },
    /// The turn finished; `text` is the final assistant reply.
    #[serde(rename = "turn.completed")]
    TurnCompleted { session: String, text: String },
    /// The turn ended in an error (auth failure, exhausted retries, …).
    #[serde(rename = "turn.failed")]
    TurnFailed { session: String, error: String },

    // --- Host round-trips ----------------------------------------------------
    /// The model asked the user structured questions (`ask_user_question`);
    /// answer via the host's answer-question command with this `id`.
    #[serde(rename = "agent.question")]
    Question {
        session: String,
        id: String,
        questions: Vec<Question>,
    },
    /// A gated tool call wants a decision; answer via the host's
    /// answer-approval command with this `id`.
    #[serde(rename = "agent.approval_request")]
    ApprovalRequest {
        session: String,
        id: String,
        kind: ApprovalKind,
        tool: String,
        command: String,
        /// The gate's risk label ("safe", "caution", "destructive", …).
        risk: String,
        reasons: Vec<String>,
        /// Human-readable description of what "always allow" would grant.
        grant_label: String,
        offer_project_grant: bool,
        offer_trash: bool,
    },
    /// Thread marker for a gated call awaiting (`pending`, empty decision) or
    /// resolved from (`resolved`, decision label) the user's approval.
    #[serde(rename = "agent.approval")]
    Approval {
        session: String,
        phase: ApprovalPhase,
        name: String,
        command: String,
        decision: String,
    },

    // --- Canvas & files ------------------------------------------------------
    /// The model produced a canvas document for the side panel.
    #[serde(rename = "agent.canvas")]
    Canvas {
        session: String,
        id: String,
        title: String,
        format: String,
        language: Option<String>,
        content: String,
    },
    /// The model started writing a canvas; content streams via `tool_delta`.
    #[serde(rename = "agent.canvas_writing")]
    CanvasWriting { session: String },
    /// The `open_file` tool asked the UI to show workspace-relative files.
    #[serde(rename = "agent.open_file")]
    OpenFile { session: String, paths: Vec<String> },

    // --- Fleet lanes ----------------------------------------------------------
    /// A fleet of parallel subagents is spinning up; `agents` is the lane
    /// labels, in order.
    #[serde(rename = "fleet.started")]
    FleetStarted {
        session: String,
        agents: Vec<String>,
        source: FleetSource,
    },
    /// One lane changed state.
    #[serde(rename = "fleet.agent")]
    FleetAgent {
        session: String,
        agent: usize,
        name: String,
        phase: FleetAgentPhase,
        tokens: usize,
        /// The lane's truncated reply or error (set on done/failed).
        summary: String,
    },
    /// What one lane is doing right now.
    #[serde(rename = "fleet.agent_activity")]
    FleetActivity {
        session: String,
        agent: usize,
        kind: FleetActivityKind,
        text: String,
        tokens: Option<usize>,
    },
    /// Every lane settled.
    #[serde(rename = "fleet.completed")]
    FleetCompleted { session: String },

    // --- Code review -----------------------------------------------------------
    /// Which pipeline step a running code review is on (and, for a fan-out
    /// step, its parallel lanes).
    #[serde(rename = "review.progress")]
    ReviewProgress {
        session: String,
        step: String,
        index: usize,
        total: usize,
        agents: Vec<String>,
    },
    /// Streamed text from the current review step's agent.
    #[serde(rename = "review.token")]
    ReviewToken { session: String, token: String },
    /// A tool the current review step's agent invoked.
    #[serde(rename = "review.tool")]
    ReviewTool { session: String, name: String },

    // --- Live preview -----------------------------------------------------------
    /// The session's dev server changed lifecycle phase.
    #[serde(rename = "preview.status")]
    PreviewStatus {
        session: String,
        phase: PreviewPhase,
        name: String,
        command: String,
        url: Option<String>,
        port: Option<u16>,
        message: Option<String>,
    },
    /// The preview page hit a JavaScript error.
    #[serde(rename = "preview.console")]
    PreviewConsole { session: String, text: String },

    // --- Local models (app-wide, not session-scoped) ----------------------------
    /// A phase of bringing a local model online.
    #[serde(rename = "local.status")]
    LocalStatus { model: String, phase: LocalPhase },
    /// Model-download progress.
    #[serde(rename = "models.progress")]
    DownloadProgress {
        id: String,
        downloaded: u64,
        total: Option<u64>,
        fraction: Option<f64>,
    },
}

impl ProtocolEvent {
    /// The wire tag for this event (`agent.token`, `fleet.started`, …) — the
    /// same string the `type` field serializes to.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Token { .. } => "agent.token",
            Self::Tool { .. } => "agent.tool",
            Self::ToolDelta { .. } => "agent.tool_delta",
            Self::Usage { .. } => "agent.usage",
            Self::Compacted { .. } => "agent.compacted",
            Self::Retry { .. } => "agent.retry",
            Self::Compression { .. } => "agent.compression",
            Self::TurnStarted { .. } => "turn.started",
            Self::TurnCompleted { .. } => "turn.completed",
            Self::TurnFailed { .. } => "turn.failed",
            Self::Question { .. } => "agent.question",
            Self::ApprovalRequest { .. } => "agent.approval_request",
            Self::Approval { .. } => "agent.approval",
            Self::Canvas { .. } => "agent.canvas",
            Self::CanvasWriting { .. } => "agent.canvas_writing",
            Self::OpenFile { .. } => "agent.open_file",
            Self::FleetStarted { .. } => "fleet.started",
            Self::FleetAgent { .. } => "fleet.agent",
            Self::FleetActivity { .. } => "fleet.agent_activity",
            Self::FleetCompleted { .. } => "fleet.completed",
            Self::ReviewProgress { .. } => "review.progress",
            Self::ReviewToken { .. } => "review.token",
            Self::ReviewTool { .. } => "review.tool",
            Self::PreviewStatus { .. } => "preview.status",
            Self::PreviewConsole { .. } => "preview.console",
            Self::LocalStatus { .. } => "local.status",
            Self::DownloadProgress { .. } => "models.progress",
        }
    }

    /// The desktop's legacy Tauri channel for this event: the tag's dot
    /// becomes `://` and underscores become hyphens (`agent.tool_delta` →
    /// `agent://tool-delta`), matching what `app/src/lib/agentEvents.ts`
    /// already listens on.
    pub fn channel(&self) -> String {
        self.kind().replacen('.', "://", 1).replace('_', "-")
    }

    /// The session this event belongs to, or `None` for app-wide events
    /// (local model status, download progress).
    pub fn session(&self) -> Option<&str> {
        match self {
            Self::Token { session, .. }
            | Self::Tool { session, .. }
            | Self::ToolDelta { session, .. }
            | Self::Usage { session, .. }
            | Self::Compacted { session, .. }
            | Self::Retry { session, .. }
            | Self::Compression { session, .. }
            | Self::TurnStarted { session }
            | Self::TurnCompleted { session, .. }
            | Self::TurnFailed { session, .. }
            | Self::Question { session, .. }
            | Self::ApprovalRequest { session, .. }
            | Self::Approval { session, .. }
            | Self::Canvas { session, .. }
            | Self::CanvasWriting { session }
            | Self::OpenFile { session, .. }
            | Self::FleetStarted { session, .. }
            | Self::FleetAgent { session, .. }
            | Self::FleetActivity { session, .. }
            | Self::FleetCompleted { session }
            | Self::ReviewProgress { session, .. }
            | Self::ReviewToken { session, .. }
            | Self::ReviewTool { session, .. }
            | Self::PreviewStatus { session, .. }
            | Self::PreviewConsole { session, .. } => Some(session),
            Self::LocalStatus { .. } | Self::DownloadProgress { .. } => None,
        }
    }
}
