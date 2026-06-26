//! Streaming (SSE) decoding and chunk assembly.
//!
//! Oxen.ai streams `chat.completion.chunk` objects as server-sent events:
//! each event is a `data: {json}` line, terminated by `data: [DONE]`. This
//! module decodes that byte stream into payloads ([`SseDecoder`]) and
//! reassembles the deltas back into a single message ([`StreamAssembler`]),
//! including merging streamed tool-call fragments by index.

use std::collections::BTreeMap;

use crate::types::{ChatChunk, FunctionCall, ToolCall};

/// An event surfaced to a streaming consumer (e.g. the REPL) as it arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// An incremental piece of assistant text.
    Token(String),
    /// The model began emitting a tool call — surfaced as soon as the tool's
    /// name is known, before its (possibly long) arguments finish streaming, so
    /// the UI can react while a tool like `canvas` is still being written.
    ToolCallStart { name: String },
    /// An incremental piece of a tool call's arguments (the raw JSON fragment),
    /// tagged with the tool's name so a UI can stream the in-progress content —
    /// e.g. a file being written or a canvas document being authored.
    ToolCallDelta { name: String, arguments: String },
    /// The stream finished, with the model's finish reason if provided.
    Done { finish_reason: Option<String> },
}

/// The message reassembled from a full stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssembledMessage {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

impl AssembledMessage {
    pub fn wants_tools(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// Incremental decoder turning raw SSE bytes into `data:` payload strings.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buf: String,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of bytes; return any complete `data:` payloads found.
    ///
    /// Lines that are not `data:` lines (comments, `event:` lines, blanks) are
    /// ignored. The `data: ` prefix is stripped from returned payloads.
    pub fn push(&mut self, bytes: &str) -> Vec<String> {
        self.buf.push_str(bytes);
        let mut payloads = Vec::new();

        while let Some(newline) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=newline).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if let Some(rest) = line.strip_prefix("data:") {
                payloads.push(rest.trim().to_string());
            }
        }
        payloads
    }
}

/// Reassembles streamed chunks into a single [`AssembledMessage`].
#[derive(Debug, Default)]
pub struct StreamAssembler {
    content: String,
    finish_reason: Option<String>,
    tool_fragments: BTreeMap<u64, ToolFragment>,
    done: bool,
}

#[derive(Debug, Default)]
struct ToolFragment {
    id: String,
    name: String,
    arguments: String,
}

/// The outcome of merging one tool-call delta: the fragment's current name, the
/// name iff this delta first named it, and any arguments fragment it carried.
struct MergedDelta {
    name: String,
    started: Option<String>,
    arguments: Option<String>,
}

impl StreamAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Process one decoded `data:` payload, returning the events it surfaces.
    ///
    /// A single chunk can yield more than one event — e.g. the chunk that first
    /// names a tool may also carry the opening of its arguments, surfacing both
    /// a [`StreamEvent::ToolCallStart`] and a [`StreamEvent::ToolCallDelta`].
    /// Returns an empty vec for payloads that carry no surfaced event.
    pub fn accept(&mut self, payload: &str) -> Vec<StreamEvent> {
        if payload == "[DONE]" {
            self.done = true;
            return vec![StreamEvent::Done {
                finish_reason: self.finish_reason.clone(),
            }];
        }

        let chunk: ChatChunk = match serde_json::from_str(payload) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let mut events = Vec::new();
        for choice in chunk.choices {
            if let Some(reason) = choice.finish_reason {
                self.finish_reason = Some(reason);
            }
            if let Some(text) = choice.delta.content {
                if !text.is_empty() {
                    self.content.push_str(&text);
                    events.push(StreamEvent::Token(text));
                }
            }
            if let Some(tool_deltas) = choice.delta.tool_calls {
                for delta in tool_deltas {
                    let merged = self.merge_tool_delta(&delta);
                    // Surface the tool name the first time we see it, so a long
                    // tool call (e.g. a canvas document) signals its start early.
                    if let Some(name) = merged.started {
                        events.push(StreamEvent::ToolCallStart { name });
                    }
                    // Surface each arguments fragment so a UI can stream the
                    // in-progress content (a file being written, a canvas doc).
                    if let Some(arguments) = merged.arguments {
                        events.push(StreamEvent::ToolCallDelta {
                            name: merged.name,
                            arguments,
                        });
                    }
                }
            }
        }
        events
    }

    /// Merge a tool-call delta into its fragment, reporting whether this delta
    /// first named the tool and any arguments fragment it carried.
    fn merge_tool_delta(&mut self, delta: &serde_json::Value) -> MergedDelta {
        let index = delta.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        let frag = self.tool_fragments.entry(index).or_default();
        if let Some(id) = delta.get("id").and_then(|v| v.as_str()) {
            frag.id = id.to_string();
        }
        let mut started = None;
        let mut arguments = None;
        if let Some(func) = delta.get("function") {
            if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                let was_unnamed = frag.name.is_empty();
                frag.name.push_str(name);
                if was_unnamed && !frag.name.is_empty() {
                    started = Some(frag.name.clone());
                }
            }
            if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                if !args.is_empty() {
                    frag.arguments.push_str(args);
                    arguments = Some(args.to_string());
                }
            }
        }
        MergedDelta {
            name: frag.name.clone(),
            started,
            arguments,
        }
    }

    /// Finalize the stream into the assembled message.
    pub fn finish(self) -> AssembledMessage {
        let tool_calls = self
            .tool_fragments
            .into_values()
            .map(|f| ToolCall {
                id: f.id,
                kind: "function".to_string(),
                function: FunctionCall {
                    name: f.name,
                    arguments: f.arguments,
                },
            })
            .collect();
        AssembledMessage {
            content: self.content,
            tool_calls,
            finish_reason: self.finish_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_extracts_data_payloads_across_boundaries() {
        let mut dec = SseDecoder::new();
        assert!(dec.push("data: hel").is_empty());
        let out = dec.push("lo\n\ndata: [DONE]\n");
        assert_eq!(out, vec!["hello".to_string(), "[DONE]".to_string()]);
    }

    #[test]
    fn assembles_streamed_content_tokens() {
        let mut asm = StreamAssembler::new();
        let chunk = |c: &str| {
            format!(
                r#"{{"choices":[{{"index":0,"delta":{{"content":"{c}"}},"finish_reason":null}}]}}"#
            )
        };
        assert_eq!(
            asm.accept(&chunk("Hello ")),
            vec![StreamEvent::Token("Hello ".into())]
        );
        assert_eq!(asm.accept(&chunk("ox")), vec![StreamEvent::Token("ox".into())]);
        let done = asm.accept("[DONE]");
        assert!(matches!(done.as_slice(), [StreamEvent::Done { .. }]));
        let msg = asm.finish();
        assert_eq!(msg.content, "Hello ox");
        assert!(!msg.wants_tools());
    }

    #[test]
    fn merges_streamed_tool_call_fragments_by_index() {
        let mut asm = StreamAssembler::new();
        asm.accept(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
        );
        asm.accept(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a.rs\"}"}}]}}]}"#,
        );
        asm.accept(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#);
        let msg = asm.finish();
        assert!(msg.wants_tools());
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].id, "call_1");
        assert_eq!(msg.tool_calls[0].function.name, "read_file");
        let args = msg.tool_calls[0].function.parsed_arguments().unwrap();
        assert_eq!(args["path"], "a.rs");
        assert_eq!(msg.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn surfaces_tool_name_and_argument_deltas_while_streaming() {
        let mut asm = StreamAssembler::new();
        // First chunk names the tool and opens its arguments.
        let first = asm.accept(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"write_file","arguments":"{\"pa"}}]}}]}"#,
        );
        assert_eq!(
            first,
            vec![
                StreamEvent::ToolCallStart { name: "write_file".into() },
                StreamEvent::ToolCallDelta { name: "write_file".into(), arguments: "{\"pa".into() },
            ]
        );
        // Subsequent chunks carry only argument fragments, tagged with the name.
        let second = asm.accept(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a.rs\"}"}}]}}]}"#,
        );
        assert_eq!(
            second,
            vec![StreamEvent::ToolCallDelta {
                name: "write_file".into(),
                arguments: "th\":\"a.rs\"}".into()
            }]
        );
    }
}
