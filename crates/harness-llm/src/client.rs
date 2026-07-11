//! HTTP client for the Oxen.ai chat completions API.

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::auth;
use crate::stream::{AssembledMessage, SseDecoder, StreamAssembler, StreamEvent};
use crate::types::{ChatRequest, ChatResponse};
use crate::{chat_completions_url, LlmError};
use harness_core::DEFAULT_MODEL;

/// A client for one Oxen.ai inference endpoint and model.
#[derive(Debug, Clone)]
pub struct OxenClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OxenClient {
    /// Build a client against `base_url` using `api_key`, defaulting to `model`.
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// Build a client from configuration: the base URL is resolved from
    /// `OXEN_BASE_URL`/`OXEN_HOST` (falling back to the default Oxen.ai
    /// endpoint), the API key from the environment or the Oxen config (keyed by
    /// the base URL's host), and the model defaults to `claude-opus-4-8`.
    pub fn from_default_config() -> Result<Self, LlmError> {
        Self::connect(auth::resolve_base_url(), DEFAULT_MODEL)
    }

    /// Build a client against an explicit `base_url`, resolving the API key for
    /// that base URL's host.
    pub fn connect(
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, LlmError> {
        let base_url = base_url.into();
        let api_key = auth::resolve_api_key_for_base_url(&base_url)?;
        Ok(Self::new(base_url, api_key, model))
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// The API base URL this client targets.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    fn endpoint(&self) -> String {
        chat_completions_url(&self.base_url)
    }

    /// Send a non-streaming chat completion request.
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let resp = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(LlmError::Api {
                status: status.as_u16(),
                message: extract_api_error(&body),
            });
        }
        serde_json::from_str(&body).map_err(LlmError::from)
    }

    /// Send a streaming chat completion request, invoking `on_event` for each
    /// surfaced [`StreamEvent`] as it arrives, and returning the fully
    /// reassembled message.
    ///
    /// Cancellation is cooperative: when `cancel` fires, we stop reading and
    /// return whatever has assembled so far (often empty — e.g. a local server
    /// still chewing through a long prompt). Dropping the response on the way
    /// out closes the HTTP connection, which signals the upstream/`llama-server`
    /// to abort generation. A stop is therefore a normal early return, not an
    /// error, so callers settle the turn cleanly rather than surfacing a failure.
    pub async fn stream_chat<F>(
        &self,
        request: &ChatRequest,
        cancel: &CancellationToken,
        mut on_event: F,
    ) -> Result<AssembledMessage, LlmError>
    where
        F: FnMut(&StreamEvent),
    {
        let request = ChatRequest {
            stream: true,
            // Ask for a final usage chunk so we can calibrate the token estimate
            // against reality; ignored by endpoints that don't support it.
            stream_options: Some(crate::types::StreamOptions {
                include_usage: true,
            }),
            ..request.clone()
        };

        // Race the request against cancellation even before the response headers
        // land, so a stop is honored while we're still waiting to connect.
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(AssembledMessage::default()),
            resp = self
                .http
                .post(self.endpoint())
                .bearer_auth(&self.api_key)
                .json(&request)
                .send() => resp?,
        };

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await?;
            return Err(LlmError::Api {
                status: status.as_u16(),
                message: extract_api_error(&body),
            });
        }

        let mut decoder = SseDecoder::new();
        let mut assembler = StreamAssembler::new();
        let mut stream = resp.bytes_stream();

        let mut cancelled = false;
        loop {
            // The long wait lives here — a local server processing a large prompt
            // may not emit a byte for many seconds. `select!` lets a stop break
            // out of that pending read immediately instead of hanging on it.
            let bytes = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    cancelled = true;
                    break;
                }
                next = stream.next() => match next {
                    Some(bytes) => bytes?,
                    None => break,
                },
            };
            let text = String::from_utf8_lossy(&bytes);
            for payload in decoder.push(&text) {
                for event in assembler.accept(&payload) {
                    on_event(&event);
                }
                if assembler.is_done() {
                    break;
                }
            }
        }

        // A stream that ends without `[DONE]` or a finish reason was cut off —
        // typically an upstream timeout dropping the connection mid-reply.
        // Treating the fragment as a finished answer would end the turn on a
        // truncated reply (the agent would persist "I'll do X…" and stop), so
        // surface it as the transient failure it is and let the caller's retry
        // policy re-send the request. A user stop is different: return whatever
        // assembled, never an error.
        if !cancelled && !assembler.is_complete() {
            return Err(LlmError::Stream(
                "the connection closed before the reply finished".into(),
            ));
        }

        Ok(assembler.finish())
    }
}

/// Pull a human-readable reason out of an Oxen API error body, so callers show
/// "You have run out of credits." rather than a wall of raw JSON. Oxen's shape is
/// `{"error":{"type":..,"title":..},"status":..,"status_message":..}`, but other
/// services vary, so we try the friendliest fields in priority order and fall
/// back to the trimmed body when none are present.
fn extract_api_error(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.trim().to_string();
    };
    // Walk a key path to a non-empty string, if present.
    let at = |path: &[&str]| -> Option<String> {
        let mut cur = &v;
        for key in path {
            cur = cur.get(*key)?;
        }
        cur.as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    at(&["error", "title"])
        .or_else(|| at(&["error", "message"]))
        .or_else(|| at(&["error", "description"]))
        .or_else(|| at(&["error"])) // some APIs return `{"error": "message"}`
        .or_else(|| at(&["status_message"]))
        .or_else(|| at(&["message"]))
        .unwrap_or_else(|| body.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    #[tokio::test]
    async fn chat_parses_a_completion() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_header("authorization", "Bearer sk-test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"id":"c1","model":"claude-opus-4-8","choices":[
                    {"index":0,"finish_reason":"stop",
                     "message":{"role":"assistant","content":"Beauregard"}}]}"#,
            )
            .create_async()
            .await;

        let client = OxenClient::new(server.url(), "sk-test", "claude-opus-4-8");
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("name an ox")]);
        let resp = client.chat(&req).await.unwrap();

        assert_eq!(
            resp.message().unwrap().content_text().as_deref(),
            Some("Beauregard")
        );
        assert_eq!(resp.finish_reason(), Some("stop"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn chat_surfaces_api_errors() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(401)
            .with_body(r#"{"error":{"message":"Invalid API key"}}"#)
            .create_async()
            .await;

        let client = OxenClient::new(server.url(), "bad", "claude-opus-4-8");
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);
        let err = client.chat(&req).await.unwrap_err();
        match err {
            LlmError::Api { status, message } => {
                assert_eq!(status, 401);
                assert_eq!(message, "Invalid API key");
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[test]
    fn extract_api_error_prefers_human_fields_over_raw_json() {
        // Oxen's insufficient-credits shape → the human `error.title`.
        let body = r#"{"error":{"type":"insufficient_credits","title":"You have run out of credits."},"status":"error","status_message":"insufficient_credits"}"#;
        assert_eq!(extract_api_error(body), "You have run out of credits.");

        // OpenAI-style `error.message`.
        assert_eq!(
            extract_api_error(r#"{"error":{"message":"Invalid API key"}}"#),
            "Invalid API key"
        );

        // `error` as a bare string, and top-level `status_message` fallback.
        assert_eq!(extract_api_error(r#"{"error":"nope"}"#), "nope");
        assert_eq!(
            extract_api_error(r#"{"status_message":"rate_limited"}"#),
            "rate_limited"
        );

        // Non-JSON or shapeless bodies fall back to the trimmed text.
        assert_eq!(
            extract_api_error("  upstream timeout  "),
            "upstream timeout"
        );
    }

    #[tokio::test]
    async fn stream_chat_collects_tokens_and_message() {
        let mut server = mockito::Server::new_async().await;
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello \"}}]}\n\n\
                   data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ox\"},\"finish_reason\":\"stop\"}]}\n\n\
                   data: [DONE]\n\n";
        let mock = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "stream": true,
                "stream_options": { "include_usage": true }
            })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let client = OxenClient::new(server.url(), "sk-test", "claude-opus-4-8");
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);

        let mut tokens = String::new();
        let assembled = client
            .stream_chat(&req, &CancellationToken::new(), |event| {
                if let StreamEvent::Token(t) = event {
                    tokens.push_str(t);
                }
            })
            .await
            .unwrap();

        assert_eq!(tokens, "Hello ox");
        assert_eq!(assembled.content, "Hello ox");
        assert_eq!(assembled.finish_reason.as_deref(), Some("stop"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn stream_chat_errors_when_the_stream_is_cut_off_mid_reply() {
        // Tokens flow, then the body ends with no finish reason and no [DONE] —
        // an upstream timeout dropping the connection mid-reply. This must be
        // an error (and a retryable one), not a "successful" truncated answer.
        let mut server = mockito::Server::new_async().await;
        let sse =
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"I'll rewrite \"}}]}\n\n";
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let client = OxenClient::new(server.url(), "sk-test", "claude-opus-4-8");
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);
        let err = client
            .stream_chat(&req, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();

        assert!(matches!(err, LlmError::Stream(_)), "got: {err:?}");
        assert!(err.is_transient(), "a cut-off stream must be retryable");
    }

    #[tokio::test]
    async fn stream_chat_accepts_a_finished_reply_without_the_done_sentinel() {
        // A finish reason arrived, so the reply is complete even though the
        // server omitted the trailing `data: [DONE]`.
        let mut server = mockito::Server::new_async().await;
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello ox\"},\"finish_reason\":\"stop\"}]}\n\n";
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let client = OxenClient::new(server.url(), "sk-test", "claude-opus-4-8");
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);
        let assembled = client
            .stream_chat(&req, &CancellationToken::new(), |_| {})
            .await
            .unwrap();

        assert_eq!(assembled.content, "Hello ox");
        assert_eq!(assembled.finish_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn stream_chat_returns_empty_when_already_cancelled() {
        // An already-cancelled token short-circuits before any request is sent,
        // returning an empty message rather than erroring.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_body("data: [DONE]\n\n")
            .expect(0)
            .create_async()
            .await;

        let client = OxenClient::new(server.url(), "sk-test", "claude-opus-4-8");
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let assembled = client.stream_chat(&req, &cancel, |_| {}).await.unwrap();
        assert_eq!(assembled.content, "");
        mock.assert_async().await; // the request was never sent
    }
}
