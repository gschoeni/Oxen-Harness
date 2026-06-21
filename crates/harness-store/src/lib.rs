//! Conversation history store for oxen-harness.
//!
//! Phase 3 fills this in with a SQLite-backed store that records sessions,
//! messages, and tool calls verbatim, plus a JSONL exporter that emits
//! fine-tune-ready transcripts.

use harness_core::Message;

/// Serialize a transcript to JSONL (one JSON message object per line).
///
/// This is the on-disk export format used to build fine-tuning datasets.
pub fn to_jsonl(messages: &[Message]) -> Result<String, serde_json::Error> {
    let mut out = String::new();
    for msg in messages {
        out.push_str(&serde_json::to_string(msg)?);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::{Message, Role};

    #[test]
    fn exports_one_json_object_per_line() {
        let transcript = vec![
            Message::system("You are a helpful coding agent."),
            Message::user("What is a great name for an ox?"),
        ];
        let jsonl = to_jsonl(&transcript).unwrap();
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: Message = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.role, Role::System);
    }
}
