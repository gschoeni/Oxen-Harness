//! Normalizing verbatim transcripts into fine-tuning data.
//!
//! The store keeps every message exactly as it flowed through the agent, which
//! is richer than a training set wants: multimodal content arrays, streamed
//! tool-call scaffolding, empty assistant turns. These pure functions reshape a
//! session's transcript into the clean chat-completions form Oxen.ai expects,
//! independent of how the messages were stored — so the transform is testable on
//! its own and [`HistoryStore`](crate::HistoryStore) just supplies the messages.

use serde_json::{Map, Value};

use crate::content::derive_content_text;

/// Assemble a session's verbatim messages into one fine-tuning conversation, or
/// `None` when it lacks a real user↔assistant exchange worth a training row
/// (so exports never contain blank or one-sided conversations).
pub(crate) fn conversation_from_messages(
    messages: &[Value],
    include_tools: bool,
) -> Option<Vec<Value>> {
    let conversation: Vec<Value> = messages
        .iter()
        .filter_map(|m| sanitize_for_finetuning(m, include_tools))
        .collect();

    let has_role = |role| {
        conversation
            .iter()
            .any(|m| m.get("role").and_then(|r| r.as_str()) == Some(role))
    };
    (has_role("user") && has_role("assistant")).then_some(conversation)
}

/// Normalize one persisted transcript message into a clean chat-completions
/// message for fine-tuning, or `None` if it should be dropped.
///
/// - `content` is flattened to a plain string (text parts joined; images dropped).
/// - When `include_tools` is false: `tool` messages are dropped and assistant
///   `tool_calls` are stripped.
/// - When `include_tools` is true: assistant `tool_calls` and a tool message's
///   `tool_call_id` are carried through verbatim.
fn sanitize_for_finetuning(message: &Value, include_tools: bool) -> Option<Value> {
    let role = message.get("role").and_then(|r| r.as_str())?;

    if role == "tool" && !include_tools {
        return None;
    }

    let content = derive_content_text(message.get("content")).unwrap_or_default();
    let tool_calls = message.get("tool_calls").filter(|v| !v.is_null());

    // A message with neither text nor (kept) tool calls carries nothing to train on.
    let keeps_tool_calls = include_tools && tool_calls.is_some();
    if content.is_empty() && !keeps_tool_calls && role != "tool" {
        return None;
    }

    let mut obj = Map::new();
    obj.insert("role".into(), Value::String(role.to_string()));
    obj.insert("content".into(), Value::String(content));
    if keeps_tool_calls {
        if let Some(tc) = tool_calls {
            obj.insert("tool_calls".into(), tc.clone());
        }
    }
    if include_tools {
        if let Some(id) = message.get("tool_call_id").filter(|v| !v.is_null()) {
            obj.insert("tool_call_id".into(), id.clone());
        }
    }
    Some(Value::Object(obj))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drops_tool_traffic_when_tools_excluded() {
        let assistant_tool_call = json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read", "arguments": "{}"}}]
        });
        let tool_result = json!({"role": "tool", "tool_call_id": "c1", "content": "body"});

        // Excluded: the tool result is gone and the empty tool-call-only assistant
        // turn carries no trainable content.
        assert_eq!(sanitize_for_finetuning(&tool_result, false), None);
        assert_eq!(sanitize_for_finetuning(&assistant_tool_call, false), None);

        // Included: both are kept, tool_calls and tool_call_id carried verbatim.
        let kept = sanitize_for_finetuning(&assistant_tool_call, true).unwrap();
        assert!(kept.get("tool_calls").is_some());
        let kept_result = sanitize_for_finetuning(&tool_result, true).unwrap();
        assert_eq!(kept_result["tool_call_id"], "c1");
    }

    #[test]
    fn flattens_multimodal_and_drops_image_bytes() {
        let msg = json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "what is in this image?"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,SECRET"}}
            ]
        });
        let out = sanitize_for_finetuning(&msg, true).unwrap();
        assert_eq!(out["content"], "what is in this image?");
        assert!(!out.to_string().contains("SECRET"));
    }

    #[test]
    fn conversation_needs_both_a_user_and_an_assistant_turn() {
        let system_only = [json!({"role": "system", "content": "be helpful"})];
        assert!(conversation_from_messages(&system_only, false).is_none());

        let exchange = [
            json!({"role": "system", "content": "be helpful"}),
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "content": "hello"}),
        ];
        let conv = conversation_from_messages(&exchange, false).unwrap();
        assert_eq!(conv.len(), 3);
    }
}
