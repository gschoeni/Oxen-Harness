//! Configuration for an [`Agent`](crate::Agent): model, prompt, context
//! budgeting, attachments, compression, and the retry policy.

use std::path::PathBuf;
use std::time::Duration;

use harness_compress::CompressionMode;
use harness_core::DEFAULT_MODEL;

use crate::prompt::default_system_prompt;

/// Backoff schedule for retrying model calls that fail transiently (provider
/// 5xx, rate limits, network blips — see [`harness_llm::LlmError::is_transient`]).
/// `max_attempts` counts the first try; the wait doubles from `base_delay`
/// after each failure (1s → 2s → 4s by default).
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_secs(1),
        }
    }
}

impl RetryPolicy {
    /// How long to wait after the `attempt`-th try failed (1-based), doubling
    /// each time: base, 2×base, 4×base, …
    pub(crate) fn delay_after(&self, attempt: u32) -> Duration {
        self.base_delay * 2u32.saturating_pow(attempt.saturating_sub(1))
    }
}

/// Configuration for an [`Agent`](crate::Agent).
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub system_prompt: Option<String>,
    /// Context window in tokens. `None` derives it from the model name; set it
    /// explicitly for locally-served models whose `llama-server` context is
    /// smaller than the model's theoretical maximum.
    pub context_window: Option<usize>,
    /// Tokens to keep free for the model's reply when budgeting the prompt.
    pub response_reserve: usize,
    /// Project root under which image/PDF attachments are stored on disk (so the
    /// transcript records a relative path, not inline base64). `None` keeps the
    /// legacy behavior of inlining attachments as data URIs.
    pub attachment_root: Option<PathBuf>,
    /// Context compression for outbound requests (see [`harness_compress`]):
    /// `Off` sends the transcript as-is, `Audit` measures would-be savings
    /// without changing anything, `On` compresses stale tool output and
    /// registers the `retrieve_original` tool so nothing is unrecoverable.
    pub compression: CompressionMode,
    /// How transient model-call failures are retried before the turn errors.
    pub retry: RetryPolicy,
    /// Where to append the developer error log (JSONL, one entry per retry
    /// attempt and per failed turn — see `crate::errlog`). `None` disables
    /// it. Writing is best-effort: log failures never affect the turn.
    pub error_log: Option<PathBuf>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            // Web search off by default: only callers that actually register the
            // tool should advertise it (see `default_system_prompt`).
            system_prompt: Some(default_system_prompt(false)),
            context_window: None,
            response_reserve: 4096,
            attachment_root: None,
            compression: CompressionMode::Off,
            retry: RetryPolicy::default(),
            error_log: None,
        }
    }
}
