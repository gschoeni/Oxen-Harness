//! OpenAI-compatible chat completion wire types for the Oxen.ai API.
//!
//! These mirror the request/response shapes documented at
//! <https://docs.oxen.ai/examples/inference/chat_completions>, including tool
//! calling and the streaming `chat.completion.chunk` deltas.

use serde::{Deserialize, Serialize};

/// The body of a chat message: either plain text or an ordered list of content
/// parts (text interleaved with images/files).
///
/// Serializing text-only messages as a bare JSON string keeps the wire format
/// byte-identical to before multimodal support; only messages that actually
/// carry attachments serialize as the OpenAI-style content-part array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    /// The plain text of this content: the string for `Text`, or the
    /// concatenation of any text parts for `Parts` (attachments contribute no
    /// text). Used for display, budgeting, and persistence-derived previews.
    pub fn as_text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Parts(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Approximate character weight for token budgeting. Text contributes its
    /// length; image/file parts add a flat nominal cost, since the model charges
    /// attachments by image tiles, not by their (huge) data-URI length.
    ///
    /// The weights are deliberately conservative (high): a model charges a
    /// full-resolution image at well over a thousand tokens, so the old flat
    /// ~256-token estimate let a single screenshot silently blow the window.
    /// We can't know the exact tile count without decoding, so we over-budget a
    /// little rather than under-budget into an overflow. Values are in
    /// character-equivalents (callers divide by ~4 chars/token).
    pub fn budget_len(&self) -> usize {
        // ~1,600 tokens — an image at full tile cost.
        const IMAGE_CHARS: usize = 6_400;
        // ~3,000 tokens — a document (e.g. a multi-page PDF) costs more than one
        // image; conservative since real page counts vary widely.
        const FILE_CHARS: usize = 12_000;
        match self {
            MessageContent::Text(s) => s.len(),
            MessageContent::Parts(parts) => parts
                .iter()
                .map(|p| match p {
                    ContentPart::Text { text } => text.len(),
                    ContentPart::ImageUrl { .. } => IMAGE_CHARS,
                    ContentPart::File { .. } => FILE_CHARS,
                })
                .sum(),
        }
    }

    /// True if this is text-only (no image/file parts).
    pub fn is_text_only(&self) -> bool {
        match self {
            MessageContent::Text(_) => true,
            MessageContent::Parts(parts) => {
                parts.iter().all(|p| matches!(p, ContentPart::Text { .. }))
            }
        }
    }
}

impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        MessageContent::Text(s)
    }
}

impl From<&str> for MessageContent {
    fn from(s: &str) -> Self {
        MessageContent::Text(s.to_string())
    }
}

/// One part of a multimodal message body (OpenAI-compatible content parts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// A run of text.
    Text { text: String },
    /// An image, carried as a URL or `data:` URI.
    ImageUrl { image_url: ImageUrl },
    /// A document (e.g. a PDF), carried as a `data:` URI under `file_data`.
    File { file: FileData },
}

impl ContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        ContentPart::Text { text: text.into() }
    }

    /// An image part from a URL or `data:` URI.
    pub fn image(url: impl Into<String>) -> Self {
        ContentPart::ImageUrl {
            image_url: ImageUrl { url: url.into() },
        }
    }

    /// A file part (e.g. a PDF) from a filename and `data:` URI.
    pub fn file(filename: impl Into<String>, file_data: impl Into<String>) -> Self {
        ContentPart::File {
            file: FileData {
                filename: filename.into(),
                file_data: file_data.into(),
            },
        }
    }
}

/// The `image_url` payload of an image content part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ImageUrl {
    /// An `http(s)` URL or a `data:<mime>;base64,<...>` URI.
    pub url: String,
}

/// The `file` payload of a document content part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FileData {
    pub filename: String,
    /// A `data:<mime>;base64,<...>` URI carrying the document bytes.
    pub file_data: String,
}

/// A message in a chat transcript. Supports plain text, multimodal content
/// (text + images/files), assistant tool calls, and `tool` result messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ChatMessage {
    pub role: String,
    // `#[ts(optional)]` mirrors `skip_serializing_if`: these keys are omitted
    // (not sent as null) when None, so the generated TS uses `field?: T`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub content: Option<MessageContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
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
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// Build a `user` message from an ordered list of content parts (text +
    /// attachments). Empty parts collapse to no content.
    pub fn user_parts(parts: Vec<ContentPart>) -> Self {
        Self {
            role: "user".into(),
            content: (!parts.is_empty()).then_some(MessageContent::Parts(parts)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// The message body as plain text (text parts only), if any.
    pub fn content_text(&self) -> Option<String> {
        self.content.as_ref().map(MessageContent::as_text)
    }

    /// Build an assistant message that may carry text, tool calls, or both.
    ///
    /// Empty `content`/`tool_calls` are normalized to `None` so the serialized
    /// message stays minimal (the API treats absent and empty differently).
    pub fn assistant_with_tools(content: String, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content: (!content.is_empty()).then_some(MessageContent::Text(content)),
            tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
            tool_call_id: None,
            name: None,
        }
    }

    /// Build a `tool` result message answering a specific tool call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        }
    }
}

/// A tool call requested by the assistant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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

/// Per-stream options. Setting `include_usage` asks an OpenAI-compatible
/// endpoint to emit a final chunk carrying the real token [`Usage`], which we
/// use to calibrate the client-side estimate. Endpoints that don't support it
/// ignore the field, and we fall back to the estimate.
#[derive(Debug, Clone, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
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
            stream_options: None,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
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
    /// Real token usage, present only on the final chunk when the request set
    /// `stream_options.include_usage` (and the endpoint honors it).
    #[serde(default)]
    pub usage: Option<Usage>,
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

    // ---- Wire-format golden tests -----------------------------------------
    // Pin the exact JSON shapes the desktop UI and stored transcripts depend
    // on. The untagged `MessageContent` is the fragile one: text must serialize
    // as a bare string, attachments as a tagged-part array. If these change, the
    // hand-mirrored TS in app/src and any consumer parsing break — so a wire
    // change must update these (and regenerate bindings.ts) deliberately.

    #[test]
    fn text_message_serializes_content_as_a_bare_string() {
        let json = serde_json::to_value(ChatMessage::user("hi")).unwrap();
        assert_eq!(json, serde_json::json!({ "role": "user", "content": "hi" }));
    }

    #[test]
    fn multimodal_message_serializes_content_as_a_tagged_part_array() {
        let msg = ChatMessage::user_parts(vec![
            ContentPart::text("look"),
            ContentPart::image("data:image/png;base64,AAAA"),
            ContentPart::file("a.pdf", "data:application/pdf;base64,BBBB"),
        ]);
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "role": "user",
                "content": [
                    { "type": "text", "text": "look" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } },
                    { "type": "file", "file": { "filename": "a.pdf", "file_data": "data:application/pdf;base64,BBBB" } }
                ]
            })
        );
    }

    #[test]
    fn assistant_tool_call_uses_type_field_and_omits_empty_content() {
        let msg = ChatMessage::assistant_with_tools(
            String::new(),
            vec![ToolCall {
                id: "call_1".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: "read_file".into(),
                    arguments: "{\"path\":\"a.rs\"}".into(),
                },
            }],
        );
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["tool_calls"][0]["type"], "function");
        assert_eq!(json["tool_calls"][0]["function"]["name"], "read_file");
        // Empty assistant text is dropped, not sent as "".
        assert!(json.get("content").is_none());
    }

    #[test]
    fn tool_result_round_trips_with_tool_call_id() {
        let msg = ChatMessage::tool_result("call_1".to_string(), "42".to_string());
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_1");
        assert_eq!(json["content"], "42");
        // Re-parsing yields the same message (the wire format is lossless).
        let back: ChatMessage = serde_json::from_value(json).unwrap();
        assert_eq!(back, msg);
    }

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
    fn assistant_with_tools_normalizes_empty_fields() {
        // Text-only: empty tool calls are dropped.
        let text = ChatMessage::assistant_with_tools("hi".into(), vec![]);
        assert_eq!(text.content_text().as_deref(), Some("hi"));
        assert!(text.tool_calls.is_none());

        // Tool-call-only: empty content is dropped.
        let call = ToolCall {
            id: "c1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: "{}".into(),
            },
        };
        let tools = ChatMessage::assistant_with_tools(String::new(), vec![call]);
        assert!(tools.content.is_none());
        assert_eq!(tools.tool_calls.unwrap().len(), 1);
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
    fn text_message_content_serializes_as_a_bare_string() {
        // The wire format for text-only messages must stay a JSON string so the
        // representation is unchanged from before multimodal support.
        let msg = ChatMessage::user("hello ox");
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["content"], "hello ox");
        assert!(json["content"].is_string());
    }

    #[test]
    fn multimodal_user_message_serializes_as_content_parts() {
        let msg = ChatMessage::user_parts(vec![
            ContentPart::text("describe this"),
            ContentPart::image("data:image/png;base64,AAAA"),
        ]);
        let json = serde_json::to_value(&msg).unwrap();
        let parts = json["content"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "describe this");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AAAA");
    }

    #[test]
    fn file_content_part_serializes_with_filename_and_data() {
        let part = ContentPart::file("paper.pdf", "data:application/pdf;base64,JVBER");
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["type"], "file");
        assert_eq!(json["file"]["filename"], "paper.pdf");
        assert_eq!(
            json["file"]["file_data"],
            "data:application/pdf;base64,JVBER"
        );
    }

    #[test]
    fn content_round_trips_through_json_for_both_shapes() {
        for msg in [
            ChatMessage::user("plain text"),
            ChatMessage::user_parts(vec![
                ContentPart::text("look"),
                ContentPart::image("data:image/jpeg;base64,ZZ"),
            ]),
        ] {
            let json = serde_json::to_string(&msg).unwrap();
            let back: ChatMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(msg, back);
        }
    }

    #[test]
    fn attachment_budget_far_exceeds_text_and_distinguishes_kinds() {
        // A short data URI for an image must budget for its real tile cost, not
        // its (tiny) string length — otherwise a screenshot silently overflows.
        let img = MessageContent::Parts(vec![ContentPart::image("data:image/png;base64,AAAA")]);
        let pdf = MessageContent::Parts(vec![ContentPart::file(
            "a.pdf",
            "data:application/pdf;base64,AAAA",
        )]);
        // Both vastly exceed their literal length, and a file costs more than an image.
        assert!(img.budget_len() > 1_000);
        assert!(pdf.budget_len() > img.budget_len());
        // Text is still budgeted by its actual length.
        assert_eq!(MessageContent::Text("hello".into()).budget_len(), 5);
    }

    #[test]
    fn image_budget_is_fixed_regardless_of_data_uri_size() {
        // A multi-megabyte base64 image must not inflate the token estimate — it's
        // capped at the fixed tile cost. Otherwise a large screenshot would count
        // as millions of "chars" and falsely trip the context-window guard.
        let small = MessageContent::Parts(vec![ContentPart::image("data:image/png;base64,AAAA")]);
        let huge = MessageContent::Parts(vec![ContentPart::image(format!(
            "data:image/png;base64,{}",
            "A".repeat(5_000_000)
        ))]);
        assert_eq!(small.budget_len(), huge.budget_len());
    }

    #[test]
    fn content_text_extracts_text_parts_only() {
        let parts = MessageContent::Parts(vec![
            ContentPart::text("a"),
            ContentPart::image("data:image/png;base64,QQ"),
            ContentPart::text("b"),
        ]);
        assert_eq!(parts.as_text(), "ab");
        assert!(!parts.is_text_only());
        assert!(MessageContent::Text("x".into()).is_text_only());
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

/// TypeScript binding generation for the wire types, behind the `ts` feature.
///
/// `app/src/lib/types.ts` re-exports the generated `bindings.ts`, so the desktop
/// UI's view of these types is derived from the Rust source of truth rather than
/// hand-mirrored. Regenerate after changing a wire type with:
///
/// ```text
/// cargo test -p harness-llm --features ts -- --ignored generate_bindings
/// ```
///
/// `bindings_are_up_to_date` then guards against forgetting to (run it with the
/// `ts` feature in CI).
#[cfg(all(test, feature = "ts"))]
mod bindings {
    use super::*;
    use ts_rs::TS;

    const BINDINGS_PATH: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/src/lib/bindings.ts");

    /// The full generated `bindings.ts` contents: a header plus each wire type's
    /// TypeScript declaration.
    fn generated() -> String {
        let mut out = String::from(
            "// @generated by harness-llm — DO NOT EDIT.\n\
             // Regenerate: cargo test -p harness-llm --features ts -- --ignored generate_bindings\n\
             // The Rust wire types in crates/harness-llm/src/types.rs are the source of truth.\n\n",
        );
        for decl in [
            ChatMessage::decl(),
            MessageContent::decl(),
            ContentPart::decl(),
            ImageUrl::decl(),
            FileData::decl(),
            ToolCall::decl(),
            FunctionCall::decl(),
        ] {
            out.push_str("export ");
            out.push_str(&decl);
            out.push('\n');
        }
        out
    }

    #[test]
    #[ignore = "writes app/src/lib/bindings.ts; run explicitly to regenerate"]
    fn generate_bindings() {
        std::fs::write(BINDINGS_PATH, generated()).expect("writing bindings.ts");
    }

    #[test]
    fn bindings_are_up_to_date() {
        let on_disk = std::fs::read_to_string(BINDINGS_PATH).unwrap_or_default();
        assert_eq!(
            on_disk,
            generated(),
            "bindings.ts is out of date — regenerate with: \
             cargo test -p harness-llm --features ts -- --ignored generate_bindings"
        );
    }
}
