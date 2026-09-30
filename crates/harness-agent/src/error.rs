//! The error type for the agent loop.

use harness_llm::{AttachmentError, LlmError};
use harness_store::HistoryError;
use harness_tools::ToolError;

/// Errors that can arise while running the agent loop.
///
/// Capability-crate errors ([`LlmError`], [`ToolError`], [`HistoryError`]) flow
/// up transparently via `#[from]`, so hosts can still match on them — e.g. the
/// CLI's auth handling matches `AgentError::Llm(LlmError::Api { status: 401, .. })`.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error("attachment IO failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Attachment(#[from] AttachmentError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("attachments total {size} bytes, over the {max}-byte per-turn limit")]
    AttachmentsTooLarge { size: usize, max: usize },
    #[error(
        "the subagent ran past its {}s time limit and did not stop when asked",
        after.as_secs()
    )]
    TimedOut { after: std::time::Duration },
    #[error(
        "the conversation grew past the model's context window \
         (~{used} prompt tokens, limit ~{window}); start a fresh session, \
         or switch to a model with a larger context window"
    )]
    ContextWindowExceeded { used: usize, window: usize },
    #[error(
        "the model endpoint failed {attempts} times in a row \
         ({model} at {endpoint}) — last error: {source}"
    )]
    RetriesExhausted {
        attempts: u32,
        model: String,
        endpoint: String,
        source: Box<LlmError>,
    },
}

impl AgentError {
    /// The model-endpoint failure behind this error, whether it surfaced
    /// directly or as the last of an exhausted retry run — so a host or log
    /// can reach the status and raw provider body without matching both
    /// variants.
    pub fn llm(&self) -> Option<&LlmError> {
        match self {
            AgentError::Llm(e) => Some(e),
            AgentError::RetriesExhausted { source, .. } => Some(source),
            _ => None,
        }
    }
}
