//! Conversation history store for oxen-harness.
//!
//! Sessions and every message (including tool calls and tool results) are
//! persisted **verbatim** in SQLite so transcripts are complete enough to build
//! fine-tuning datasets from. The full JSON of each message is stored in a
//! `raw_json` column; role/content are extracted alongside for querying.
//!
//! [`HistoryStore::export_jsonl`] emits one JSON object per line — the on-disk
//! format used to build fine-tuning datasets.

pub mod store;

pub use store::{HistoryError, HistoryStore, SessionMeta};

use serde::Serialize;

/// Serialize any sequence of serializable items to JSONL (one per line).
pub fn to_jsonl<T: Serialize>(items: &[T]) -> Result<String, serde_json::Error> {
    let mut out = String::new();
    for item in items {
        out.push_str(&serde_json::to_string(item)?);
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
