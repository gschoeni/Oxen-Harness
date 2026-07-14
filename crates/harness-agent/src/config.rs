//! Configuration for an [`Agent`](crate::Agent): model, prompt, context
//! budgeting, attachments, compression, and the retry policy.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use harness_compress::CompressionMode;
use harness_core::DEFAULT_MODEL;
use harness_permissions::PermissionGate;

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
    /// Maximum approximate text retained in the active model context. Verbatim
    /// history remains on disk; older turns are compacted before this grows
    /// without bound even when the provider advertises a very large window.
    pub max_resident_context_chars: usize,
    /// Project root under which image/PDF attachments are stored on disk (so the
    /// transcript records a relative path, not inline base64). `None` keeps the
    /// legacy behavior of inlining attachments as data URIs.
    pub attachment_root: Option<PathBuf>,
    /// Durable project PDFs/images attached automatically to the first user
    /// prompt in a new chat. Text project context stays on disk and is read on
    /// demand with `read_file`.
    pub initial_attachments: Vec<PathBuf>,
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
    /// The permission gate consulted before every tool call (classification,
    /// approval prompts, circuit breakers — see `harness-permissions`).
    /// `None` runs tools ungated. Subagents automatically get the gate's
    /// non-interactive [`for_subagent`] form (see `subagent_tools`' reasoning).
    ///
    /// [`for_subagent`]: harness_permissions::PermissionGate::for_subagent
    pub permissions: Option<Arc<PermissionGate>>,
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
            max_resident_context_chars: 1_000_000,
            attachment_root: None,
            initial_attachments: Vec::new(),
            compression: CompressionMode::Off,
            retry: RetryPolicy::default(),
            error_log: None,
            permissions: None,
        }
    }
}
