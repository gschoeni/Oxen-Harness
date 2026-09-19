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
pub use attachment_store::{
    base64_len, hydrate_content, hydrate_content_bounded, hydrated_data_uri_len, AttachmentStore,
    MAX_OUTBOUND_ATTACHMENT_BYTES, MAX_OUTBOUND_ATTACHMENT_PARTS,
};
pub use auth::{base_url_from_host, host_from_base_url, resolve_base_url};
pub use client::OxenClient;
/// The retry schedule lives in its own leaf crate so tools can share it
/// without linking the client; it keeps its old path here.
pub use harness_http as retry;
pub use harness_http::{send_with_retry, Backoff, Transient};
pub use stream::{AssembledMessage, StreamEvent};
pub use types::{
    ChatMessage, ChatRequest, ChatResponse, ContentPart, FileData, FunctionCall, ImageUrl,
    MessageContent, ToolCall, ToolChoice, Usage,
};

use harness_core::DEFAULT_BASE_URL;

/// Errors returned by the LLM client.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("could not encode chat request: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// A non-2xx reply. `message` is the human-readable reason pulled out of
    /// the body (what a notice shows); `body` is the raw response, capped at
    /// [`MAX_ERROR_BODY`] bytes, kept so a UI can show exactly what the
    /// provider said when the friendly line is as vague as "the model
    /// provider returned an error". `retry_after` is the server's own wait
    /// hint (`Retry-After`), which the retry loop obeys over its schedule.
    #[error("Oxen API error ({status}): {message}")]
    Api {
        status: u16,
        message: String,
        body: String,
        retry_after: Option<std::time::Duration>,
    },
    #[error("auth error: {0}")]
    Auth(String),
    #[error("decode error: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("stream error: {0}")]
    Stream(String),
}

/// How much of an error response body an [`LlmError::Api`] keeps. Provider
/// error pages are normally a few hundred bytes; the cap only guards against a
/// misrouted HTML page or a proxy dumping the request back. Because that
/// dump can echo our own headers, the kept body also passes through
/// [`redact_credentials`] — it travels into notices, the error log and the
/// desktop's "copy the error report", none of which may carry a key.
pub const MAX_ERROR_BODY: usize = 8 * 1024;

/// Blank out credential values in text that came back from a server: a
/// `Bearer …` token wherever it appears, and the value after an
/// `authorization` / `x-api-key` / `api_key` field in either header
/// (`name: value`) or JSON (`"name": "value"`) form. Only the secret is
/// replaced, so the surrounding context stays readable.
pub fn redact_credentials(text: &str) -> String {
    const REDACTED: &str = "[redacted]";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((start, len)) = next_secret(rest) {
        out.push_str(&rest[..start]);
        out.push_str(REDACTED);
        rest = &rest[start + len..];
    }
    out.push_str(rest);
    out
}

/// Field names whose value is a credential when followed by `:` or `=`.
const CREDENTIAL_FIELDS: [&str; 5] = ["authorization", "x-api-key", "api_key", "api-key", "apikey"];

/// The byte range of the first credential value in `text`, if any.
fn next_secret(text: &str) -> Option<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let mut best: Option<(usize, usize)> = None;
    let mut consider = |start: usize, len: usize| {
        if len > 0 && best.is_none_or(|(s, _)| start < s) {
            best = Some((start, len));
        }
    };
    for (at, _) in lower.match_indices("bearer ") {
        let start = at + "bearer ".len();
        consider(start, token_len(&text[start..]));
    }
    for field in CREDENTIAL_FIELDS {
        for (at, _) in lower.match_indices(field) {
            let after = at + field.len();
            let Some(start) = value_start(&text[after..]).map(|offset| after + offset) else {
                continue;
            };
            // `Authorization: Bearer …` is covered by the bearer rule above,
            // which keeps the scheme word readable.
            if lower[start..].starts_with("bearer ") {
                continue;
            }
            consider(start, token_len(&text[start..]));
        }
    }
    best
}

/// Offset of the value after a field name: an optional closing quote, then
/// `:` or `=`, then optional spaces and an opening quote. `None` when no
/// separator follows — the word appeared in prose, not as a field.
fn value_start(tail: &str) -> Option<usize> {
    let bytes = tail.as_bytes();
    let mut i = 0;
    while i < bytes.len() && matches!(bytes[i], b'"' | b'\'' | b' ') {
        i += 1;
    }
    if !matches!(bytes.get(i), Some(b':' | b'=')) {
        return None;
    }
    i += 1;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'"' | b'\'') {
        i += 1;
    }
    Some(i)
}

/// How far a credential token extends: up to the first whitespace, quote,
/// or delimiter that ends a header, JSON, or query-string value.
fn token_len(text: &str) -> usize {
    text.find(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | ',' | '&' | '<' | '>' | '}' | ']')
    })
    .unwrap_or(text.len())
}

impl LlmError {
    /// Build an [`LlmError::Api`] from a failed response: the friendly reason
    /// is extracted for the display line and the raw body is kept (trimmed and
    /// capped) for the debugging view.
    pub fn api(status: u16, body: &str) -> Self {
        let body = redact_credentials(body.trim());
        LlmError::Api {
            status,
            message: client::extract_api_error(&body),
            body: harness_core::text::truncate_with_marker(
                &body,
                MAX_ERROR_BODY,
                "\n… [truncated]",
            ),
            retry_after: None,
        }
    }

    /// [`LlmError::api`] that also keeps the server's `Retry-After` hint from
    /// the failed response's headers (see [`retry::retry_after_from_headers`]).
    pub fn api_with_headers(status: u16, headers: &reqwest::header::HeaderMap, body: &str) -> Self {
        match Self::api(status, body) {
            LlmError::Api {
                status,
                message,
                body,
                ..
            } => LlmError::Api {
                status,
                message,
                body,
                retry_after: retry::retry_after_from_headers(headers),
            },
            other => other,
        }
    }

    /// The server's wait hint, when the failed response carried one.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            LlmError::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// The HTTP status behind this error, when there was a response at all.
    pub fn status(&self) -> Option<u16> {
        match self {
            LlmError::Api { status, .. } => Some(*status),
            LlmError::Http(e) => e.status().map(|s| s.as_u16()),
            _ => None,
        }
    }

    /// The raw material behind the one-line message, for a debugging view:
    /// the provider's response body for an API error, or the full error chain
    /// for a transport failure (a bare "error sending request" hides the
    /// connect/timeout cause underneath). `None` when the display line already
    /// says everything there is to say.
    pub fn detail(&self) -> Option<String> {
        match self {
            LlmError::Api { body, .. } => (!body.is_empty()).then(|| body.clone()),
            LlmError::Http(e) => {
                let mut chain = vec![e.to_string()];
                let mut source = std::error::Error::source(e);
                while let Some(s) = source {
                    chain.push(s.to_string());
                    source = s.source();
                }
                Some(chain.join("\n  caused by: "))
            }
            _ => None,
        }
    }

    /// Whether this failure is plausibly transient — a provider hiccup or a
    /// network blip — and therefore worth retrying with backoff. Transport
    /// errors (connect failures, timeouts, a dropped stream) and the statuses
    /// services return for temporary trouble (408 timeout, 429 rate limit,
    /// 5xx upstream errors, 529 overloaded) qualify. Auth (401), credits
    /// (402), and malformed requests don't: retrying can't fix them.
    pub fn is_transient(&self) -> bool {
        self.transient_kind().is_some()
    }

    /// Which kind of transient failure this is — a rate limit waits longer
    /// and gets more attempts than a blip (see [`retry::Backoff`]) — or
    /// `None` when retrying can't help.
    pub fn transient_kind(&self) -> Option<Transient> {
        match self {
            LlmError::Http(e) => (!e.is_builder()).then_some(Transient::Blip),
            LlmError::Api { status, .. } => retry::transient_status(*status),
            // A stream cut off mid-reply (upstream timeout, dropped connection)
            // is a network blip: re-sending the same request is safe because
            // nothing was persisted from the partial reply.
            LlmError::Stream(_) => Some(Transient::Blip),
            _ => None,
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
    fn kept_error_bodies_never_carry_credentials() {
        // A proxy that dumps the request back echoes our own headers; the
        // body then reaches notices, errors.jsonl and the desktop modal.
        let echoed = "502 upstream\nAuthorization: Bearer sk-oxen-SECRET\nx-api-key: hf_SECRET2\n\
                      {\"api_key\": \"SECRET3\", \"model\": \"claude-opus-4-8\"}\n\
                      api-key=SECRET4&model=x";
        let err = LlmError::api(502, echoed);
        let body = err.detail().expect("an API error keeps its body");
        for secret in ["SECRET", "SECRET2", "SECRET3", "SECRET4"] {
            assert!(!body.contains(secret), "{secret} leaked: {body}");
        }
        // The surrounding context stays readable: scheme, field names,
        // and the non-secret fields.
        assert!(body.contains("Authorization: Bearer [redacted]"), "{body}");
        assert!(body.contains("x-api-key: [redacted]"), "{body}");
        assert!(body.contains("\"api_key\": \"[redacted]\""), "{body}");
        assert!(body.contains("\"model\": \"claude-opus-4-8\""), "{body}");
        assert!(body.contains("api-key=[redacted]&model=x"), "{body}");
        // Prose that merely mentions a field is left alone.
        assert_eq!(
            redact_credentials("set your api key in the authorization settings"),
            "set your api key in the authorization settings"
        );
        assert_eq!(redact_credentials("Bearer "), "Bearer ");
    }

    #[test]
    fn transient_errors_are_the_retryable_ones() {
        let api = |status| LlmError::api(status, "boom");
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
    fn api_errors_keep_the_raw_body_behind_the_friendly_line() {
        let body = r#"{"error":{"type":"upstream","title":"The model provider returned an error."},"status":502}"#;
        let err = LlmError::api(502, body);
        // The display line is the friendly reason…
        assert_eq!(
            err.to_string(),
            "Oxen API error (502): The model provider returned an error."
        );
        // …while the status and the untouched body are there for a detail view.
        assert_eq!(err.status(), Some(502));
        assert_eq!(err.detail().as_deref(), Some(body));
    }

    #[test]
    fn api_error_bodies_are_capped_and_marked() {
        let huge = "x".repeat(MAX_ERROR_BODY + 100);
        let err = LlmError::api(500, &huge);
        let detail = err.detail().unwrap();
        assert!(
            detail.ends_with("… [truncated]"),
            "{}",
            &detail[detail.len() - 40..]
        );
        assert!(detail.chars().count() < MAX_ERROR_BODY + 20);
        // An empty body has nothing to show.
        assert_eq!(LlmError::api(502, "   ").detail(), None);
    }

    #[test]
    fn stream_errors_have_no_detail_beyond_their_message() {
        let err = LlmError::Stream("the connection closed before the reply finished".into());
        assert_eq!(err.status(), None);
        assert_eq!(err.detail(), None);
    }

    #[test]
    fn default_endpoint_points_at_oxen() {
        assert_eq!(
            default_chat_completions_url(),
            "https://hub.oxen.ai/api/ai/chat/completions"
        );
    }
}
