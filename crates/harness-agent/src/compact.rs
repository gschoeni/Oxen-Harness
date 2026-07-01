//! Context compaction: keeping a long conversation under the model's window
//! instead of hard-stopping when it fills.
//!
//! Two stages, applied in order by [`Agent::compact_to_fit`](crate::Agent):
//!
//! 1. **Prune stale tool results** ([`prune_tool_results`]) — replace the
//!    content of older `tool` messages (big shell dumps, file reads the model
//!    has already acted on) with a short stub. Cheap, no model call, and
//!    structure-preserving: the `tool` role and `tool_call_id` stay intact, so
//!    no assistant/tool pairing is broken.
//!
//! 2. **Summarize the oldest turns** ([`summary_cut_index`] + a model call) —
//!    if pruning didn't free enough, collapse the oldest whole turns into a
//!    single summary message. The cut always falls on a user-turn boundary so a
//!    `tool_result` is never separated from the assistant `tool_use` that
//!    spawned it (which the API would reject).
//!
//! Compaction mutates the agent's in-memory transcript only; the history store
//! keeps the full record, so nothing is lost on disk.

use harness_llm::types::{ChatMessage, MessageContent};

/// Replace the content of all but the most recent `keep_recent_tools` `tool`
/// messages with a short stub, returning the number of characters freed.
///
/// Recency is measured among **tool** messages, not all messages, so one big
/// stale tool result buried under later chat still gets pruned — that's the
/// common way a window fills. The latest tool output is kept verbatim (the
/// model is likely still working with it). Idempotent: an already-stubbed or
/// tiny message is left alone, so repeated calls don't churn.
pub fn prune_tool_results(messages: &mut [ChatMessage], keep_recent_tools: usize) -> usize {
    let tool_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == "tool")
        .map(|(i, _)| i)
        .collect();
    let protect_from = tool_indices.len().saturating_sub(keep_recent_tools);
    let mut freed = 0;
    for &i in &tool_indices[..protect_from] {
        let msg = &mut messages[i];
        let text = match &msg.content {
            Some(c) => c.as_text(),
            None => continue,
        };
        // Skip a message we've already stubbed, so repeated calls don't churn.
        if text.starts_with(ELIDED_PREFIX) {
            continue;
        }
        let len = text.len();
        let stub = elided_stub(len);
        if len <= stub.len() {
            continue; // tiny output — nothing worth reclaiming
        }
        freed += len - stub.len();
        msg.content = Some(MessageContent::Text(stub));
    }
    freed
}

const ELIDED_PREFIX: &str = "[earlier tool output elided";

fn elided_stub(original_len: usize) -> String {
    format!("{ELIDED_PREFIX} to free context — {original_len} chars]")
}

/// The exclusive end index of the oldest run of messages that can be summarized,
/// leaving the leading system prompt (if any) and the last `keep_recent_turns`
/// user turns untouched. Returns `None` when there aren't enough turns to be
/// worth summarizing.
///
/// A "turn" begins at a `user` message (role `"user"`); cutting on those
/// boundaries guarantees the summarized span never splits an assistant
/// `tool_use` from its `tool` result.
pub fn summary_cut_index(messages: &[ChatMessage], keep_recent_turns: usize) -> Option<usize> {
    let start = usize::from(messages.first().is_some_and(|m| m.role == "system"));
    let user_turns: Vec<usize> = messages
        .iter()
        .enumerate()
        .skip(start)
        .filter(|(_, m)| m.role == "user")
        .map(|(i, _)| i)
        .collect();
    // Need at least one turn to summarize beyond the ones we keep.
    if user_turns.len() <= keep_recent_turns {
        return None;
    }
    let cut = user_turns[user_turns.len() - keep_recent_turns];
    // Nothing summarizable before the cut (cut is at or before the prefix).
    (cut > start).then_some(cut)
}

/// Flatten a slice of messages to plain text for the summarizer prompt. Each
/// line is `role: text`; attachment-only messages contribute just their role.
pub fn render_for_summary(messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        let text = m
            .content
            .as_ref()
            .map(MessageContent::as_text)
            .unwrap_or_default();
        out.push_str(&m.role);
        out.push_str(": ");
        out.push_str(&text);
        out.push('\n');
    }
    out
}

/// The marker prefixed to a summary message so it's distinguishable in the
/// transcript (and in the UI) from an ordinary user turn.
pub const SUMMARY_MARKER: &str = "[Earlier conversation summarized to free context]";

/// The instruction handed to the model when summarizing the elided span.
pub const SUMMARY_PROMPT: &str = "Summarize the following earlier portion of a coding \
    conversation so the work can continue without it. Preserve concrete facts the assistant \
    will still need: decisions made, files and symbols touched, what was tried, what worked or \
    failed, and any open threads. Be concise and factual — no preamble.";

#[cfg(test)]
mod tests {
    use super::*;
    use harness_llm::types::ChatMessage;

    fn tool_msg(id: &str, content: &str) -> ChatMessage {
        ChatMessage::tool_result(id.to_string(), content.to_string())
    }

    #[test]
    fn prune_stubs_old_tool_results_keeps_recent() {
        let big = "x".repeat(5000);
        // An old big tool result buried under later chat, plus a recent one.
        let mut msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("do it"),
            tool_msg("a", &big), // older tool result → pruned
            ChatMessage::assistant("ok"),
            ChatMessage::user("more"),
            tool_msg("b", &big), // most recent tool result → kept
        ];
        let freed = prune_tool_results(&mut msgs, 1);
        assert!(freed > 4000);
        assert!(msgs[2].content_text().unwrap().contains("elided"));
        assert_eq!(msgs[5].content_text().unwrap(), big); // recent kept verbatim
    }

    #[test]
    fn prune_is_idempotent_and_skips_small_output() {
        let mut msgs = vec![
            ChatMessage::user("q"),
            tool_msg("a", &"y".repeat(5000)),
            ChatMessage::user("q2"),
            tool_msg("b", "tiny"),
        ];
        let first = prune_tool_results(&mut msgs, 1); // keep last tool only
        assert!(first > 0);
        let second = prune_tool_results(&mut msgs, 1);
        assert_eq!(second, 0); // nothing left to reclaim
    }

    #[test]
    fn prune_ignores_non_tool_messages() {
        let mut msgs = vec![
            ChatMessage::user(&"u".repeat(5000)),
            ChatMessage::assistant(&"a".repeat(5000)),
        ];
        assert_eq!(prune_tool_results(&mut msgs, 0), 0);
    }

    #[test]
    fn cut_index_falls_on_a_user_boundary_and_keeps_recent_turns() {
        // system, [turn1: user/assistant], [turn2: user/tool/assistant], [turn3: user]
        let msgs = vec![
            ChatMessage::system("sys"),   // 0
            ChatMessage::user("t1"),      // 1  turn 1
            ChatMessage::assistant("a1"), // 2
            ChatMessage::user("t2"),      // 3  turn 2
            tool_msg("x", "out"),         // 4
            ChatMessage::assistant("a2"), // 5
            ChatMessage::user("t3"),      // 6  turn 3
        ];
        // Keep the last 1 turn → summarize up to the start of turn 3 (index 6).
        assert_eq!(summary_cut_index(&msgs, 1), Some(6));
        // Keep the last 2 turns → cut at start of turn 2 (index 3).
        assert_eq!(summary_cut_index(&msgs, 2), Some(3));
    }

    #[test]
    fn cut_index_none_when_too_few_turns() {
        let msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("t1"),
            ChatMessage::assistant("a1"),
        ];
        assert_eq!(summary_cut_index(&msgs, 3), None);
    }

    #[test]
    fn render_includes_roles_and_text() {
        let msgs = vec![ChatMessage::user("hello"), ChatMessage::assistant("hi")];
        let rendered = render_for_summary(&msgs);
        assert!(rendered.contains("user: hello"));
        assert!(rendered.contains("assistant: hi"));
    }
}
