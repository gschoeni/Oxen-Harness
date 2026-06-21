//! Oxen.ai chat completions client for oxen-harness.
//!
//! Phase 1 fills this in with the OpenAI-compatible request/response types,
//! `liboxen`-based auth resolution, SSE token streaming, and tool calling.

use harness_core::DEFAULT_BASE_URL;

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
    fn default_endpoint_points_at_oxen() {
        assert_eq!(
            default_chat_completions_url(),
            "https://hub.oxen.ai/api/ai/chat/completions"
        );
    }
}
