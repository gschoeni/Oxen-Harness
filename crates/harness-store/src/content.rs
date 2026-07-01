//! Deriving displayable text from a message's `content` field.
//!
//! A message's content is stored verbatim, which for a user turn with
//! attachments is a multimodal array rather than a plain string. Both the
//! queryable `content` column (and the session title derived from it) and the
//! fine-tuning [`export`](crate::export) need the plain text out of that shape,
//! so the extraction lives here once.

use serde_json::Value;

/// The plain-text rendering of a message's `content`.
///
/// A plain string is used as-is. A multimodal `Parts` array — what a user
/// message with attachments serializes to — is flattened to its `text` parts
/// joined by newlines, so a session that opened with an image still titles by
/// the words the user typed instead of recording `NULL`. Image/file parts carry
/// no displayable text and are skipped. Returns `None` when there's no text
/// (e.g. an assistant turn that's only tool calls).
pub(crate) fn derive_content_text(content: Option<&Value>) -> Option<String> {
    match content {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(parts)) => {
            let text: Vec<&str> = parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            (!text.is_empty()).then(|| text.join("\n"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_string_is_used_verbatim() {
        assert_eq!(
            derive_content_text(Some(&json!("hello"))),
            Some("hello".to_string())
        );
    }

    #[test]
    fn multimodal_array_keeps_text_and_drops_images() {
        let content = json!([
            {"type": "text", "text": "what is this?"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,XXX"}},
            {"type": "text", "text": "be brief"},
        ]);
        assert_eq!(
            derive_content_text(Some(&content)),
            Some("what is this?\nbe brief".to_string())
        );
    }

    #[test]
    fn no_text_yields_none() {
        assert_eq!(derive_content_text(None), None);
        assert_eq!(derive_content_text(Some(&json!(null))), None);
        // An image-only array has no displayable text.
        let image_only = json!([{"type": "image_url", "image_url": {"url": "x"}}]);
        assert_eq!(derive_content_text(Some(&image_only)), None);
    }
}
