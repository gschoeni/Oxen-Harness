//! The error type for the agent loop.

use harness_llm::LlmError;
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
    Json(#[from] serde_json::Error),
    #[error("attachments total {size} bytes, over the {max}-byte per-turn limit")]
    AttachmentsTooLarge { size: usize, max: usize },
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
        source: LlmError,
    },
}
