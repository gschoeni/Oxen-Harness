//! Importing conversations from other coding tools' local logs.
//!
//! Each importer reads a tool's own on-disk history — Claude Code's per-project
//! JSONL transcripts, Cursor's SQLite state database — and normalizes every
//! conversation into the same OpenAI-style message shape native chats persist
//! (`role` / `content`, assistant `tool_calls`, `tool` results with a
//! `tool_call_id`). Extended-thinking text is carried on an extra `reasoning`
//! field, which the fine-tuning export passes through when present.
//!
//! Importers are pure readers over paths the caller supplies; persisting (and
//! deduping re-scans) is [`HistoryStore::import_conversations`](crate::HistoryStore::import_conversations).

pub mod claude_code;
pub mod cursor;

use serde::Serialize;
use serde_json::Value;

/// Source name for conversations imported from Claude Code.
pub const SOURCE_CLAUDE_CODE: &str = "claude-code";
/// Source name for conversations imported from Cursor.
pub const SOURCE_CURSOR: &str = "cursor";

/// One conversation read out of an external tool's logs, normalized to the
/// store's native message shape and ready to persist.
#[derive(Debug, Clone)]
pub struct ImportedConversation {
    /// The source tool's own conversation id (session uuid / composer id) —
    /// the dedup key for re-scans.
    pub source_ref: String,
    /// The working directory the conversation ran in, when the source records
    /// one (empty otherwise).
    pub workspace: String,
    /// The model that answered, when the source records one.
    pub model: String,
    /// Conversation creation time, epoch seconds (best effort).
    pub created_at: i64,
    /// The normalized transcript, in order.
    pub messages: Vec<Value>,
}

/// What an import pass did: new conversations, ones re-imported because they
/// grew at the source, and unchanged ones left alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
}

/// True when the conversation holds a real user↔assistant exchange — the same
/// bar `list_sessions` and the fine-tuning export apply, so imports never
/// create untitled or untrainable sessions.
fn has_exchange(messages: &[Value]) -> bool {
    let has = |role: &str| {
        messages.iter().any(|m| {
            m.get("role").and_then(|r| r.as_str()) == Some(role)
                && crate::content::derive_content_text(m.get("content"))
                    .is_some_and(|t| !t.trim().is_empty())
        })
    };
    has("user") && (has("assistant") || messages.iter().any(|m| m.get("tool_calls").is_some()))
}

/// Flatten a value that is either a plain string or an array of typed blocks
/// (`{"type":"text","text":…}`-style) into its text. Foreign tools nest tool
/// results this way; non-text blocks are skipped.
fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                p.get("text")
                    .or_else(|| p.get("content"))
                    .and_then(|t| t.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Parse an ISO-8601 UTC timestamp (`2026-07-18T18:05:51.041Z`) to epoch
/// seconds, without pulling in a date crate for one format.
fn parse_iso_secs(s: &str) -> Option<i64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let time = time.trim_end_matches('Z');
    let mut t = time.split(':');
    let (h, min): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let sec: i64 = t
        .next()
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Some(days_from_civil(y, m, day) * 86_400 + h * 3_600 + min * 60 + sec)
}

/// Days since the Unix epoch for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_iso_timestamps_to_epoch_seconds() {
        // Cross-checked against Cursor's own millisecond timestamps for the
        // same wall-clock dates.
        assert_eq!(parse_iso_secs("2026-07-18T18:05:51.041Z"), Some(1784397951));
        assert_eq!(parse_iso_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_secs("not a date"), None);
    }

    #[test]
    fn an_exchange_needs_real_user_and_assistant_turns() {
        assert!(!has_exchange(&[json!({"role":"user","content":"hi"})]));
        assert!(has_exchange(&[
            json!({"role":"user","content":"hi"}),
            json!({"role":"assistant","content":"hello"}),
        ]));
        // Whitespace-only turns don't count.
        assert!(!has_exchange(&[
            json!({"role":"user","content":"  "}),
            json!({"role":"assistant","content":"hello"}),
        ]));
    }
}
