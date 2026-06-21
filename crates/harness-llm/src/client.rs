//! HTTP client for the Oxen.ai chat completions API.

use futures_util::StreamExt;

use crate::auth;
use crate::stream::{AssembledMessage, SseDecoder, StreamAssembler, StreamEvent};
use crate::types::{ChatRequest, ChatResponse};
use crate::{chat_completions_url, LlmError};
use harness_core::{DEFAULT_BASE_URL, DEFAULT_MODEL};

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

    /// Build the default Oxen.ai client, resolving the API key from the
    /// environment or `liboxen` config and defaulting to `claude-opus-4-8`.
    pub fn from_default_config() -> Result<Self, LlmError> {
        let api_key = auth::resolve_default_api_key()?;
        Ok(Self::new(DEFAULT_BASE_URL, api_key, DEFAULT_MODEL))
    }

    pub fn model(&self) -> &str {
        &self.model
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
    pub async fn stream_chat<F>(
        &self,
        request: &ChatRequest,
        mut on_event: F,
    ) -> Result<AssembledMessage, LlmError>
    where
        F: FnMut(&StreamEvent),
    {
        let request = ChatRequest {
            stream: true,
            ..request.clone()
        };

        let resp = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?;

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

        while let Some(bytes) = stream.next().await {
            let bytes = bytes?;
            let text = String::from_utf8_lossy(&bytes);
            for payload in decoder.push(&text) {
                if let Some(event) = assembler.accept(&payload) {
                    on_event(&event);
                }
                if assembler.is_done() {
                    break;
                }
            }
        }

        Ok(assembler.finish())
    }
}

fn extract_api_error(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
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
            resp.message().unwrap().content.as_deref(),
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

    #[tokio::test]
    async fn stream_chat_collects_tokens_and_message() {
        let mut server = mockito::Server::new_async().await;
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello \"}}]}\n\n\
                   data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ox\"},\"finish_reason\":\"stop\"}]}\n\n\
                   data: [DONE]\n\n";
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let client = OxenClient::new(server.url(), "sk-test", "claude-opus-4-8");
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);

        let mut tokens = String::new();
        let assembled = client
            .stream_chat(&req, |event| {
                if let StreamEvent::Token(t) = event {
                    tokens.push_str(t);
                }
            })
            .await
            .unwrap();

        assert_eq!(tokens, "Hello ox");
        assert_eq!(assembled.content, "Hello ox");
        assert_eq!(assembled.finish_reason.as_deref(), Some("stop"));
    }
}
