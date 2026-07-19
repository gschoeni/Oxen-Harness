//! Token budgeting for the agent loop.
//!
//! Rather than capping the loop at a fixed number of iterations, the agent is
//! bounded by the model's *context window*: before each model call we estimate
//! how many prompt tokens the current transcript (plus tool definitions) would
//! use, and stop if that would overflow the window (minus a reserve for the
//! reply). Counts are estimated client-side with a simple chars/token heuristic
//! so this works for every endpoint — remote or local — without bundling a
//! model-specific tokenizer.

use harness_llm::types::ChatMessage;
use harness_llm::ToolCall;

/// Rough characters-per-token ratio for mixed English + code. Good enough for
/// budgeting; real tokenizers vary, so we stay conservative elsewhere.
const CHARS_PER_TOKEN: usize = 4;
/// Per-message structural overhead (role tags, delimiters) in tokens.
const PER_MESSAGE_OVERHEAD: usize = 4;
/// Context window assumed for an unrecognized model.
const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

/// Best-effort context window (in tokens) for a model, by name — the
/// **fallback** when no authoritative size is configured.
///
/// Hosts prefer accurate sources and pass them via `AgentConfig::context_window`:
/// the endpoint catalog's reported `context_length` (cached per model id in
/// `~/.oxen-harness/model-limits.json`) for cloud models, and the actual
/// `llama-server` context size for local ones (usually far smaller than the
/// model's theoretical maximum). This table only decides when neither is
/// known — conservative, well-known sizes, with unknown models falling back
/// to a reasonable default.
pub fn context_window_for(model: &str) -> usize {
    let m = model.to_ascii_lowercase();
    if m.contains("claude") || m.contains("fable") || m.contains("mythos") {
        // The 4.6+ Opus/Sonnet generation and Fable/Mythos ship a 1M window;
        // Haiku and older Claude families stay at 200K.
        let million = m.contains("fable")
            || m.contains("mythos")
            || m.contains("opus-4-6")
            || m.contains("opus-4-7")
            || m.contains("opus-4-8")
            || m.contains("sonnet-4-6");
        if million {
            1_000_000
        } else {
            200_000
        }
    } else if m.contains("gemini") {
        1_000_000
    } else if m.contains("gpt") || m.contains("o1") || m.contains("o3") || m.contains("o4") {
        128_000
    } else if m.contains("qwen") {
        32_768
    } else if m.contains("llama") || m.contains("mistral") || m.contains("gemma") {
        8_192
    } else {
        DEFAULT_CONTEXT_WINDOW
    }
}

/// Estimate the prompt tokens for a transcript plus its tool definitions.
pub fn estimate_prompt_tokens(messages: &[ChatMessage], tools: &[serde_json::Value]) -> usize {
    let mut chars = 0usize;
    for m in messages {
        chars += m.role.len();
        if let Some(c) = &m.content {
            chars += c.budget_len();
        }
        if let Some(calls) = &m.tool_calls {
            for c in calls {
                chars += c.function.name.len() + c.function.arguments.len() + c.id.len();
            }
        }
        if let Some(id) = &m.tool_call_id {
            chars += id.len();
        }
        if let Some(name) = &m.name {
            chars += name.len();
        }
    }
    let tool_chars: usize = tools.iter().map(|t| t.to_string().len()).sum();
    chars += tool_chars;

    chars / CHARS_PER_TOKEN + messages.len() * PER_MESSAGE_OVERHEAD
}

/// Tokens for a raw character count, on the same heuristic as the estimators
/// above (used to express compression savings in the meter's units).
pub fn estimate_tokens_for_chars(chars: usize) -> usize {
    chars / CHARS_PER_TOKEN
}

/// The full prompt size one call's reported usage describes, reconciling the
/// two counting styles seen in the wild (both verified against real
/// endpoints):
///
/// - **Inclusive** (OpenAI-style): `prompt_tokens` covers the whole prompt
///   and `cached_tokens` is the subset served from cache — the cache detail
///   fits inside the count, so the count *is* the full size.
/// - **Exclusive** (Anthropic-style pass-through): `prompt_tokens` covers
///   only the uncached remainder (a fully cached 4K prompt reports ~3), so
///   the full size is the sum of the parts.
///
/// Used to calibrate the client-side estimate; summing blindly would double
/// the inclusive style, and taking `prompt_tokens` alone would collapse the
/// exclusive one.
pub fn reported_full_prompt(usage: &harness_llm::types::Usage) -> usize {
    let prompt = usage.prompt_tokens as usize;
    let cache_detail = usage.cached_prompt_tokens() as usize + usage.cache_write_tokens() as usize;
    if cache_detail <= prompt {
        prompt
    } else {
        prompt + cache_detail
    }
}

/// Estimate the tokens generated in an assembled reply (text + tool calls).
pub fn estimate_completion_tokens(content: &str, tool_calls: &[ToolCall]) -> usize {
    let mut chars = content.len();
    for c in tool_calls {
        chars += c.function.name.len() + c.function.arguments.len();
    }
    chars / CHARS_PER_TOKEN
}

/// Split one side-completion's usage into (prompt, completion) tokens:
/// provider-reported counts when the stream carried them, else
/// `fallback_prompt` plus an estimate of the generated reply. The one policy
/// for accounting one-shot calls ([`Agent::complete`], the prefire
/// summarizer), so speculative and synchronous spend can't drift.
///
/// [`Agent::complete`]: crate::Agent::complete
pub fn split_oneshot_usage(
    assembled: &harness_llm::stream::AssembledMessage,
    fallback_prompt: usize,
) -> (usize, usize) {
    match &assembled.usage {
        Some(usage) if usage.prompt_tokens + usage.completion_tokens > 0 => (
            usage.prompt_tokens as usize,
            usage.completion_tokens as usize,
        ),
        _ => (
            fallback_prompt,
            estimate_completion_tokens(&assembled.content, &assembled.tool_calls),
        ),
    }
}

/// The prompt-token budget for a context `window`, reserving room for the reply.
///
/// The reserve is clamped to half the window so small (e.g. local) windows still
/// leave room for a prompt.
pub fn prompt_budget(window: usize, response_reserve: usize) -> usize {
    window.saturating_sub(response_reserve.min(window / 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_windows_match_known_families() {
        // 4.6+ Opus/Sonnet and Fable ship a 1M window...
        assert_eq!(context_window_for("claude-opus-4-8"), 1_000_000);
        assert_eq!(context_window_for("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(context_window_for("claude-fable-5"), 1_000_000);
        // ...while Haiku and older Claude families stay at 200K.
        assert_eq!(context_window_for("claude-haiku-4-5"), 200_000);
        assert_eq!(context_window_for("claude-3-opus"), 200_000);
        assert_eq!(context_window_for("gpt-5-mini"), 128_000);
        assert_eq!(context_window_for("qwen3-8b"), 32_768);
        assert_eq!(context_window_for("llama-3-8b"), 8_192);
        // Unknown -> default.
        assert_eq!(context_window_for("some-new-model"), DEFAULT_CONTEXT_WINDOW);
    }

    #[test]
    fn prompt_token_estimate_grows_with_content() {
        let empty = estimate_prompt_tokens(&[], &[]);
        let small = estimate_prompt_tokens(&[ChatMessage::user("hi")], &[]);
        let big = estimate_prompt_tokens(&[ChatMessage::user("x".repeat(4000))], &[]);
        assert!(empty <= small);
        assert!(big > small);
        // ~4000 chars / 4 chars-per-token ≈ 1000 tokens (plus overhead).
        assert!(big >= 1000);
    }

    #[test]
    fn tool_definitions_count_toward_the_prompt() {
        let tools = vec![serde_json::json!({"name": "x".repeat(400)})];
        let without = estimate_prompt_tokens(&[ChatMessage::user("hi")], &[]);
        let with = estimate_prompt_tokens(&[ChatMessage::user("hi")], &tools);
        assert!(with > without);
    }

    #[test]
    fn reported_full_prompt_handles_both_counting_styles() {
        use harness_llm::types::{PromptTokensDetails, Usage};

        // Inclusive (OpenAI-style): cached is a subset of prompt_tokens — the
        // count already is the full size; summing would double it.
        let inclusive = Usage {
            prompt_tokens: 4346,
            prompt_tokens_details: Some(PromptTokensDetails {
                cached_tokens: 4343,
            }),
            ..Usage::default()
        };
        assert_eq!(reported_full_prompt(&inclusive), 4346);

        // Exclusive (Anthropic-style pass-through): prompt_tokens is only the
        // uncached remainder — the full size is the sum of the parts.
        let exclusive = Usage {
            prompt_tokens: 3,
            cache_read_input_tokens: Some(900),
            cache_creation_input_tokens: Some(80),
            ..Usage::default()
        };
        assert_eq!(reported_full_prompt(&exclusive), 983);

        // No cache detail at all: the plain count stands.
        let plain = Usage {
            prompt_tokens: 1200,
            ..Usage::default()
        };
        assert_eq!(reported_full_prompt(&plain), 1200);
    }

    #[test]
    fn budget_reserves_room_and_clamps_for_small_windows() {
        assert_eq!(prompt_budget(200_000, 4096), 195_904);
        // Reserve larger than half the window is clamped to half.
        assert_eq!(prompt_budget(8_192, 100_000), 4_096);
    }
}
