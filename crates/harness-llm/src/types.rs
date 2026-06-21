//! OpenAI-compatible chat completion wire types for the Oxen.ai API.
//!
//! These mirror the request/response shapes documented at
//! <https://docs.oxen.ai/examples/inference/chat_completions>, including tool
//! calling and the streaming `chat.completion.chunk` deltas.

use serde::{Deserialize, Serialize};

/// A message in a chat transcript. Supports plain text, assistant tool calls,
/// and `tool` result messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::text("system", content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::text("user", content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::text("assistant", content)
    }

    fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Build a `tool` result message answering a specific tool call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }
}

/// A tool call requested by the assistant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_type")]
    pub kind: String,
    pub function: FunctionCall,
}

fn default_tool_type() -> String {
    "function".to_string()
}

/// The function name + JSON-string arguments of a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments as a JSON string (per the OpenAI tool-calling format).
    pub arguments: String,
}

impl FunctionCall {
    /// Parse the `arguments` JSON string into a value.
    pub fn parsed_arguments(&self) -> Result<serde_json::Value, serde_json::Error> {
        if self.arguments.trim().is_empty() {
            Ok(serde_json::json!({}))
        } else {
            serde_json::from_str(&self.arguments)
        }
    }
}

/// How the model should choose tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// `"auto"`, `"none"`, or `"required"`.
    Mode(String),
    /// Force a specific function.
    Function(serde_json::Value),
}

/// A chat completion request body.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub stream: bool,
}

impl ChatRequest {
    /// A non-streaming request for `model` over `messages`, no tools.
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: false,
        }
    }

    pub fn with_tools(mut self, tools: Vec<serde_json::Value>) -> Self {
        self.tools = tools;
        self
    }

    pub fn streaming(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }
}

/// A full (non-streaming) chat completion response.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

impl ChatResponse {
    /// The first choice's message, if any.
    pub fn message(&self) -> Option<&ChatMessage> {
        self.choices.first().map(|c| &c.message)
    }

    /// The first choice's finish reason, if any.
    pub fn finish_reason(&self) -> Option<&str> {
        self.choices
            .first()
            .and_then(|c| c.finish_reason.as_deref())
    }

    /// True if the model is asking to call tools.
    pub fn wants_tools(&self) -> bool {
        self.finish_reason() == Some("tool_calls")
            || self
                .message()
                .and_then(|m| m.tool_calls.as_ref())
                .is_some_and(|t| !t.is_empty())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    #[serde(default)]
    pub index: u32,
    pub message: ChatMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

/// A single streaming chunk (`chat.completion.chunk`).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatChunk {
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChunkChoice {
    #[serde(default)]
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// The incremental delta carried by a streaming chunk.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_omits_empty_tools_and_none_fields() {
        let req = ChatRequest::new("claude-opus-4-8", vec![ChatMessage::user("hi")]);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-opus-4-8");
        assert_eq!(json["stream"], false);
        assert!(json.get("tools").is_none());
        assert!(json.get("temperature").is_none());
    }

    #[test]
    fn tool_result_message_serializes_with_tool_call_id() {
        let msg = ChatMessage::tool_result("call_1", "{\"ok\":true}");
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_1");
        assert!(json.get("tool_calls").is_none());
    }

    #[test]
    fn parses_tool_call_response_and_detects_intent() {
        let body = serde_json::json!({
            "id": "chatcmpl-1",
            "model": "gpt-5-4-2026-03-05",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": { "name": "get_weather", "arguments": "{\"city\":\"Paris\"}" }
                    }]
                }
            }]
        });
        let resp: ChatResponse = serde_json::from_value(body).unwrap();
        assert!(resp.wants_tools());
        let call = &resp.message().unwrap().tool_calls.as_ref().unwrap()[0];
        assert_eq!(call.function.name, "get_weather");
        assert_eq!(call.function.parsed_arguments().unwrap()["city"], "Paris");
    }

    #[test]
    fn parses_streaming_chunk_delta() {
        let line = r#"{"id":"x","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}"#;
        let chunk: ChatChunk = serde_json::from_str(line).unwrap();
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hello"));
    }
}
