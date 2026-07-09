//! Oxen.ai chat completions client for oxen-harness.
//!
//! Provides the OpenAI-compatible wire [`types`], API-key resolution via
//! [`auth`] (env var or the Oxen `auth_config.toml`), an HTTP
//! [`client::OxenClient`] with non-streaming and SSE [`stream`]ing calls, and
//! tool-calling support.

pub mod attachment;
pub mod attachment_store;
pub mod auth;
pub mod client;
pub mod stream;
pub mod types;

pub use attachment::{mime_for_extension, Attachment, AttachmentError, AttachmentKind};
pub use attachment_store::{hydrate_content, AttachmentStore};
pub use auth::{base_url_from_host, host_from_base_url, resolve_base_url};
pub use client::OxenClient;
pub use stream::{AssembledMessage, StreamEvent};
pub use types::{
    ChatMessage, ChatRequest, ChatResponse, ContentPart, FileData, FunctionCall, ImageUrl,
    MessageContent, ToolCall, ToolChoice, Usage,
};

use harness_core::DEFAULT_BASE_URL;

/// Errors returned by the LLM client.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Oxen API error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("auth error: {0}")]
    Auth(String),
    #[error("decode error: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("stream error: {0}")]
    Stream(String),
}

impl LlmError {
    /// Whether this failure is plausibly transient — a provider hiccup or a
    /// network blip — and therefore worth retrying with backoff. Transport
    /// errors (connect failures, timeouts, a dropped stream) and the statuses
    /// services return for temporary trouble (408 timeout, 429 rate limit,
    /// 5xx upstream errors, 529 overloaded) qualify. Auth (401), credits
    /// (402), and malformed requests don't: retrying can't fix them.
    pub fn is_transient(&self) -> bool {
        match self {
            LlmError::Http(e) => !e.is_builder(),
            LlmError::Api { status, .. } => matches!(*status, 408 | 429 | 500..=599),
            // A stream cut off mid-reply (upstream timeout, dropped connection)
            // is a network blip: re-sending the same request is safe because
            // nothing was persisted from the partial reply.
            LlmError::Stream(_) => true,
            _ => false,
        }
    }
}

/// Resolve the chat completions endpoint for a given API base URL.
pub fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

/// The default Oxen.ai chat completions endpoint.
pub fn default_chat_completions_url() -> String {
    chat_completions_url(DEFAULT_BASE_URL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_chat_completions_url_without_double_slash() {
        assert_eq!(
            chat_completions_url("https://hub.oxen.ai/api/ai/"),
            "https://hub.oxen.ai/api/ai/chat/completions"
        );
    }

    #[test]
    fn transient_errors_are_the_retryable_ones() {
        let api = |status| LlmError::Api {
            status,
            message: "boom".into(),
        };
        // Provider-side trouble and throttling are worth retrying…
        assert!(api(502).is_transient());
        assert!(api(500).is_transient());
        assert!(api(529).is_transient());
        assert!(api(429).is_transient());
        assert!(api(408).is_transient());
        // …as is a stream that died mid-reply (dropped connection)…
        assert!(LlmError::Stream("cut off".into()).is_transient());
        // …but auth, credits, and bad requests are not.
        assert!(!api(401).is_transient());
        assert!(!api(402).is_transient());
        assert!(!api(400).is_transient());
        assert!(!LlmError::Auth("no key".into()).is_transient());
    }

    #[test]
    fn default_endpoint_points_at_oxen() {
        assert_eq!(
            default_chat_completions_url(),
            "https://hub.oxen.ai/api/ai/chat/completions"
        );
    }
}
