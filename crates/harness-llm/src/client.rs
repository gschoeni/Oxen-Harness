//! HTTP client for the Oxen.ai chat completions API.

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::auth;
use crate::stream::{AssembledMessage, SseDecoder, StreamAssembler, StreamEvent};
use crate::types::{ChatRequest, ChatResponse};
use crate::{chat_completions_url, LlmError};
use harness_core::DEFAULT_MODEL;

/// How long a stream is read past `[DONE]` for the server to end the
/// response (see [`OxenClient::stream_chat`]).
const DONE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// A client for one Oxen.ai inference endpoint and model.
#[derive(Debug, Clone)]
pub struct OxenClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    /// How long to read past `[DONE]`; [`DONE_GRACE`] outside tests.
    done_grace: std::time::Duration,
}

impl OxenClient {
    /// Build a client against `base_url` using `api_key`, defaulting to `model`.
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            http: harness_http::client(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            done_grace: DONE_GRACE,
        }
    }

    /// Shorten the post-`[DONE]` grace so a test of it doesn't sleep 2s.
    #[cfg(test)]
    fn with_done_grace(mut self, grace: std::time::Duration) -> Self {
        self.done_grace = grace;
        self
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
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request.to_bytes().map_err(LlmError::Encode)?)
            .send()
            .await?;

        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(LlmError::api_with_headers(status.as_u16(), &headers, &body));
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
        // Race the request against cancellation even before the response headers
        // land, so a stop is honored while we're still waiting to connect.
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(AssembledMessage::default()),
            resp = self
                .http
                .post(self.endpoint())
                .bearer_auth(&self.api_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request.to_bytes().map_err(LlmError::Encode)?)
                .send() => resp?,
        };

        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let body = resp.text().await?;
            return Err(LlmError::api_with_headers(status.as_u16(), &headers, &body));
        }

        let mut decoder = SseDecoder::new();
        let mut assembler = StreamAssembler::new();
        let mut stream = resp.bytes_stream();

        let mut cancelled = false;
        'read: loop {
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
            for payload in decoder.push(&bytes) {
                for event in assembler.accept(&payload) {
                    on_event(&event);
                }
                // `[DONE]` ends the reply; a keep-alive server may hold the
                // connection open after it, so stop parsing rather than wait.
                if assembler.is_done() {
                    break 'read;
                }
            }
        }

        // The reply is complete at `[DONE]`, but the server's work may not be:
        // a server that settles the request (billing, logging) after sending
        // the sentinel loses that work if the client hangs up on it. Read on
        // until the server ends the response, as the OpenAI SDKs do — bounded,
        // so a server that holds the connection open costs at most the grace.
        if assembler.is_done() && !cancelled {
            // Running out the grace is not a failure: the reply is whole, and
            // a server holding the connection open is owed no more than this.
            tokio::time::timeout(self.done_grace, async {
                loop {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return,
                        next = stream.next() => match next {
                            Some(Ok(_)) => {}
                            // The end of the body, or the server dropping the
                            // connection after the reply: either way, done.
                            Some(Err(_)) | None => return,
                        },
                    }
                }
            })
            .await
            .unwrap_or(());
        }

        // A user stop returns whatever assembled, never an error.
        if cancelled {
            return Ok(assembler.finish());
        }

        // A failure after the 200 was committed can't change the status, so
        // the server reports it in-band and then closes the stream normally.
        // The `[DONE]` that follows says the stream ended, not that the reply
        // did — the error decides, with the status it carried.
        if let Some(failure) = assembler.failure() {
            return Err(LlmError::api(failure.status, &failure.body));
        }

        // A stream that ends without `[DONE]` or a finish reason was cut off —
        // typically an upstream timeout dropping the connection mid-reply.
        // Treating the fragment as a finished answer would end the turn on a
        // truncated reply (the agent would persist "I'll do X…" and stop), so
        // surface it as the transient failure it is and let the caller's retry
        // policy re-send the request.
        if !assembler.is_complete() {
            return Err(LlmError::Stream(
                "the connection closed before the reply finished".into(),
            ));
        }

        Ok(assembler.finish())
    }
}

/// Pull a human-readable reason out of an Oxen API error body, so callers show
/// "You have run out of credits." rather than a wall of raw JSON. Oxen's shape is
/// `{"error":{"type":..,"title":..,"detail":..},"status":..,"status_message":..}`,
/// where `title` names the category and `detail` carries what actually went
/// wrong — for a relayed provider failure the title is always "The model
/// provider returned an error." and only the detail says why, so the detail
/// wins when both are present. Other services vary, so the friendliest fields
/// are tried in priority order, falling back to the trimmed body when none are
/// present. The result is flattened to one line: it is a notice, not a report.
pub(crate) fn extract_api_error(body: &str) -> String {
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
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
    };
    at(&["error", "detail"])
        .or_else(|| at(&["error", "title"]))
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
            // The hub tells harness traffic apart by this header.
            .match_header("user-agent", harness_http::user_agent().as_str())
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
            LlmError::Api {
                status,
                message,
                body,
                retry_after,
            } => {
                assert_eq!(status, 401);
                assert_eq!(message, "Invalid API key");
                // The raw body rides along for the debugging view.
                assert_eq!(body, r#"{"error":{"message":"Invalid API key"}}"#);
                assert_eq!(retry_after, None);
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_rate_limit_keeps_the_servers_retry_after() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(429)
            .with_header("retry-after", "17")
            .with_body(r#"{"error":{"title":"The model provider returned an error."}}"#)
            .create_async()
            .await;

        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);
        let err = client.chat(&req).await.unwrap_err();
        assert_eq!(err.status(), Some(429));
        assert_eq!(err.transient_kind(), Some(crate::Transient::RateLimited));
        assert_eq!(err.retry_after(), Some(std::time::Duration::from_secs(17)));
        // The streaming path reads the same header.
        let cancel = tokio_util::sync::CancellationToken::new();
        let err = client.stream_chat(&req, &cancel, |_| {}).await.unwrap_err();
        assert_eq!(err.retry_after(), Some(std::time::Duration::from_secs(17)));
    }

    #[test]
    fn extract_api_error_prefers_human_fields_over_raw_json() {
        // Oxen's insufficient-credits shape → the human `error.title`.
        let body = r#"{"error":{"type":"insufficient_credits","title":"You have run out of credits."},"status":"error","status_message":"insufficient_credits"}"#;
        assert_eq!(extract_api_error(body), "You have run out of credits.");

        // A relayed provider failure: the title is the same generic sentence
        // every time, so the detail (what the provider said) is the line shown,
        // squashed onto one line.
        let body = r#"{"error":{"type":"model_response_error","title":"The model provider returned an error.","detail":"Invalid 'input[3].output':\n  string too long"},"status":"error"}"#;
        assert_eq!(
            extract_api_error(body),
            "Invalid 'input[3].output': string too long"
        );

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
        let req =
            ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]).streaming(true);

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
    async fn stream_chat_returns_at_done_without_waiting_for_the_connection_to_close() {
        // A keep-alive server may hold the connection open after `[DONE]`.
        // The sentinel ends the reply, so the client must stop reading there.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 64 * 1024];
            let _ = sock.read(&mut buf).await.unwrap();
            let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ox\"}}]}\n\n\
                        data: [DONE]\n\n";
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                        transfer-encoding: chunked\r\n\r\n";
            let chunk = format!("{:x}\r\n{body}\r\n", body.len());
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(chunk.as_bytes()).await.unwrap();
            // Never send the terminating chunk: hold the socket open.
            std::future::pending::<()>().await;
        });

        let grace = std::time::Duration::from_millis(100);
        let client = OxenClient::new(url, "sk-test", "claude-opus-4-8").with_done_grace(grace);
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);
        let assembled = tokio::time::timeout(
            grace + std::time::Duration::from_secs(2),
            client.stream_chat(&req, &CancellationToken::new(), |_| {}),
        )
        .await
        .expect("stream_chat waited past the grace for a held-open connection")
        .unwrap();
        server.abort();

        assert_eq!(assembled.content, "ox");
    }

    #[tokio::test]
    async fn stream_chat_stays_until_the_server_ends_the_response_after_done() {
        // The hub settles a request (bills it) after sending `[DONE]`; a client
        // that hangs up at the sentinel cancels that. The client must keep the
        // connection until the server finishes the response.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 64 * 1024];
            let _ = sock.read(&mut buf).await.unwrap();
            let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ox\"},\"finish_reason\":\"stop\"}]}\n\n\
                        data: [DONE]\n\n";
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                        transfer-encoding: chunked\r\n\r\n";
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(format!("{:x}\r\n{body}\r\n", body.len()).as_bytes())
                .await
                .unwrap();
            // The server's post-reply work, then the end of the response.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let sent = sock.write_all(b"0\r\n\r\n").await.is_ok();
            let _ = finished_tx.send(sent);
            std::future::pending::<()>().await;
        });

        let client = OxenClient::new(url, "sk-test", "claude-opus-4-8");
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);
        let started = std::time::Instant::now();
        let assembled = client
            .stream_chat(&req, &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(assembled.content, "ox");
        assert!(
            finished_rx.await.unwrap(),
            "the server finished its response"
        );
        assert!(
            elapsed >= std::time::Duration::from_millis(300),
            "returned before the server ended the response: {elapsed:?}"
        );
        assert!(
            elapsed < DONE_GRACE,
            "waited out the grace instead: {elapsed:?}"
        );
        server.abort();
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
    async fn stream_chat_errors_when_the_server_reports_a_failure_mid_stream() {
        // The hub can't change the status once the 200 is on the wire, so a
        // provider failure mid-reply arrives as an error frame followed by a
        // normal `[DONE]`. Reading that as a finished answer ended a turn on
        // "I'm writing the file now." with the write never made.
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Writing it now.\"}}]}\n\n",
            "data: {\"error\":{\"type\":\"model_response_error\",",
            "\"title\":\"The model provider returned an error.\",",
            "\"detail\":\":timeout\",\"code\":502}}\n\n",
            "data: [DONE]\n\n",
        );
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

        match &err {
            LlmError::Api {
                status, message, ..
            } => {
                assert_eq!(*status, 502);
                assert_eq!(message, ":timeout");
            }
            other => panic!("expected an API error, got: {other:?}"),
        }
        assert!(err.is_transient(), "an upstream timeout must be retryable");
    }

    #[tokio::test]
    async fn a_mid_stream_error_keeps_its_own_status() {
        // The frame's status decides whether a retry can help: a 400 relayed
        // mid-stream must fail the call, not be re-sent four times.
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"error\":{\"detail\":\"prompt is too long\",\"code\":400}}\n\n",
            "data: [DONE]\n\n",
        );
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

        assert!(
            matches!(err, LlmError::Api { status: 400, .. }),
            "got: {err:?}"
        );
        assert!(!err.is_transient());
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
