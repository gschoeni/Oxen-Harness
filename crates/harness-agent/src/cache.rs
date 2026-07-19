//! Prompt-cache shaping and diagnostics.
//!
//! Two halves, both provider-neutral:
//!
//! - **Shaping** ([`PromptCacheMode`]) decides which outbound messages get
//!   Anthropic-style `cache_control` breakpoints (see
//!   [`harness_llm::ChatRequest::with_cache_anchors`]): the last two
//!   content-bearing user/assistant messages, so each tool-loop round extends
//!   the previous round's cached prefix — system prompt and tool definitions
//!   included — instead of re-billing it. Endpoints that don't support the
//!   marker ignore it; whether it *worked* is read from the usage a call
//!   reports, not assumed.
//!
//! - **Diagnostics** ([`fingerprints`] / [`diff_prefix`]) fingerprint each
//!   outbound message so a cache miss is attributable: a request that only
//!   appends to the previous one should hit the provider's prefix cache, and
//!   when one doesn't extend cleanly, the first changed index says what
//!   invalidated it (an edited message, a compaction splice, a tool change).

use std::hash::{Hash, Hasher};

use harness_llm::types::ChatMessage;

/// Whether (and when) outbound requests carry prompt-cache breakpoints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PromptCacheMode {
    /// Never mark breakpoints.
    Off,
    /// Mark breakpoints only for model families known to honor
    /// Anthropic-style `cache_control` (Claude/Fable/Mythos names). The
    /// default: harmless where unsupported, a large input-cost discount where
    /// supported.
    #[default]
    Auto,
    /// Always mark breakpoints, whatever the model name.
    On,
}

impl PromptCacheMode {
    /// The message indices to anchor for a request to `model`: the last two
    /// content-bearing `user`/`assistant` messages, or nothing when the
    /// mode/model opts out.
    ///
    /// Why this shape (each verified against hub.oxen.ai's Anthropic proxy):
    /// a breakpoint caches the *entire* prefix up to it — system prompt and
    /// tool definitions included — so markers belong near the tip, and
    /// anchoring the previous tip too keeps the provider's backward cache
    /// lookup anchored when a round appends several messages. Roles matter:
    /// a parts-form `system` message is rejected outright, a marker on a
    /// `tool` message is accepted but produces no caching, and an assistant
    /// message without content has nowhere to carry the marker — so those
    /// three never get an anchor.
    pub fn anchors_for(&self, model: &str, messages: &[ChatMessage]) -> Vec<usize> {
        let enabled = match self {
            PromptCacheMode::Off => false,
            PromptCacheMode::On => true,
            PromptCacheMode::Auto => model_honors_cache_control(model),
        };
        if !enabled {
            return Vec::new();
        }
        let mut anchors: Vec<usize> = messages
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, m)| (m.role == "user" || m.role == "assistant") && m.content.is_some())
            .map(|(i, _)| i)
            .take(2)
            .collect();
        anchors.reverse();
        anchors
    }
}

/// Model families that honor Anthropic-style `cache_control` markers (directly
/// or through the OpenAI-compatible proxies this harness targets).
fn model_honors_cache_control(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    ["claude", "fable", "mythos", "opus", "sonnet", "haiku"]
        .iter()
        .any(|family| m.contains(family))
}

/// A stable per-message fingerprint of the outbound transcript, for prefix
/// diffing between consecutive requests. Hashes each message's serialized
/// form; stability is only needed within one process (the comparison never
/// persists).
pub fn fingerprints(messages: &[ChatMessage]) -> Vec<u64> {
    messages
        .iter()
        .map(|m| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            serde_json::to_string(m)
                .unwrap_or_default()
                .hash(&mut hasher);
            hasher.finish()
        })
        .collect()
}

/// One fingerprint over the whole tool-definition block (tool changes
/// invalidate the provider's cached prefix from the very top).
pub fn hash_tools(tools: &[serde_json::Value]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for tool in tools {
        tool.to_string().hash(&mut hasher);
    }
    hasher.finish()
}

/// How this request's message prefix relates to the previous request's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixDiff {
    /// No previous request to compare against.
    First,
    /// The request extends the previous one unchanged — the shape a provider
    /// prefix cache rewards. `shared` is the reused message count.
    AppendOnly { shared: usize },
    /// A message inside the previously-sent prefix changed at `at` —
    /// everything from there on is a cache miss.
    Diverged { at: usize },
}

/// Classify `cur` against `prev` (see [`PrefixDiff`]). A shrunk transcript
/// (compaction splice) reports the divergence at the first changed index.
pub fn diff_prefix(prev: &[u64], cur: &[u64]) -> PrefixDiff {
    if prev.is_empty() {
        return PrefixDiff::First;
    }
    let shared = prev
        .iter()
        .zip(cur.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if shared == prev.len() && cur.len() >= prev.len() {
        PrefixDiff::AppendOnly { shared }
    } else {
        PrefixDiff::Diverged { at: shared }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_anchors_only_cache_control_families() {
        let auto = PromptCacheMode::Auto;
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u1"),
            ChatMessage::assistant("a1"),
            ChatMessage::user("u2"),
        ];
        // The last two content-bearing user/assistant messages carry the
        // markers — never the system prompt, which proxies reject in parts form.
        assert_eq!(auto.anchors_for("claude-fable-5", &msgs), vec![2, 3]);
        assert_eq!(
            auto.anchors_for("claude-opus-4-8", &msgs[..2]),
            vec![1],
            "a single eligible message gets the one anchor"
        );
        assert!(auto.anchors_for("gpt-5-mini", &msgs).is_empty());
        assert!(auto.anchors_for("qwen3-8b", &msgs).is_empty());
        assert!(auto.anchors_for("claude-fable-5", &[]).is_empty());
        assert!(
            auto.anchors_for("claude-fable-5", &msgs[..1]).is_empty(),
            "a system-only transcript has nothing to anchor"
        );

        assert_eq!(
            PromptCacheMode::On.anchors_for("gpt-5-mini", &msgs),
            vec![2, 3]
        );
        assert!(PromptCacheMode::Off
            .anchors_for("claude-fable-5", &msgs)
            .is_empty());
    }

    #[test]
    fn tool_results_and_contentless_assistant_turns_are_never_anchored() {
        // The realistic mid-turn shape: the newest messages are a pure tool
        // call and its result. Neither can carry a working marker (verified:
        // tool-role markers are ignored by the provider; a content-less
        // assistant message has nowhere to put one), so the anchors fall back
        // to the latest content-bearing user/assistant messages.
        let msgs = vec![
            ChatMessage::system("s"),
            ChatMessage::user("u1"),
            ChatMessage::assistant("working on it"),
            ChatMessage::assistant_with_tools(
                String::new(),
                vec![harness_llm::ToolCall {
                    id: "c1".into(),
                    kind: "function".into(),
                    function: harness_llm::FunctionCall {
                        name: "run_shell".into(),
                        arguments: "{}".into(),
                    },
                }],
            ),
            ChatMessage::tool_result("c1", "exit_code: 0"),
        ];
        assert_eq!(
            PromptCacheMode::Auto.anchors_for("claude-fable-5", &msgs),
            vec![1, 2]
        );
    }

    #[test]
    fn prefix_diff_classifies_append_edit_and_shrink() {
        let a = fingerprints(&[ChatMessage::system("s"), ChatMessage::user("u1")]);
        // Appending keeps the prefix.
        let b = fingerprints(&[
            ChatMessage::system("s"),
            ChatMessage::user("u1"),
            ChatMessage::assistant("a1"),
        ]);
        assert_eq!(diff_prefix(&a, &b), PrefixDiff::AppendOnly { shared: 2 });
        // Same transcript re-sent is still append-only (fully shared).
        assert_eq!(diff_prefix(&a, &a), PrefixDiff::AppendOnly { shared: 2 });

        // Editing an already-sent message diverges at its index.
        let edited = fingerprints(&[ChatMessage::system("s"), ChatMessage::user("EDITED")]);
        assert_eq!(diff_prefix(&a, &edited), PrefixDiff::Diverged { at: 1 });

        // A compaction splice (shorter, new content) diverges where it changed.
        let spliced = fingerprints(&[ChatMessage::system("s2")]);
        assert_eq!(diff_prefix(&a, &spliced), PrefixDiff::Diverged { at: 0 });

        // First request has nothing to compare against.
        assert_eq!(diff_prefix(&[], &a), PrefixDiff::First);
    }
}
