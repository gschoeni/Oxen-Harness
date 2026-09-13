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
    /// Models to fall back to, in order, once `max_attempts` on the current
    /// model are spent and the failure still looks transient. A provider
    /// having a bad day shouldn't end the turn when another model is healthy;
    /// the switch is per-call, so the session model is unchanged.
    pub fallback_models: Vec<String>,
}

/// The longest a single retry wait may be, whatever the configured base and
/// attempt count multiply out to. Unbounded exponential backoff has a history
/// of surprising people (a doubling schedule a few misconfigured attempts deep
/// sleeps for hours); past a minute, waiting longer doesn't make a provider
/// recover sooner.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_secs(1),
            fallback_models: Vec::new(),
        }
    }
}

impl RetryPolicy {
    /// How long to wait after the `attempt`-th try failed (1-based), doubling
    /// each time — base, 2×base, 4×base, … — clamped to [`MAX_RETRY_DELAY`].
    pub(crate) fn delay_after(&self, attempt: u32) -> Duration {
        self.base_delay
            .saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)))
            .min(MAX_RETRY_DELAY)
    }
}

/// Which model does which kind of work.
///
/// One session model doing everything is the expensive default: a fleet lane
/// grepping for call sites, a review pass, and a compaction summary do not
/// need the model that writes the code. Each role falls back to the session
/// model when unset, so an unconfigured harness behaves exactly as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The session model — normal turns.
    Default,
    /// Cheap and fast, for bulk mechanical work: fleet lanes, review steps.
    Smol,
    /// Compaction summaries, which re-read a whole elided span.
    Summary,
}

/// Per-role model overrides. Empty by default: no routing at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelRoles {
    pub smol: Option<String>,
    pub summary: Option<String>,
}

impl ModelRoles {
    /// The model for `role`, falling back to `session_model` when that role
    /// has no override.
    pub fn resolve<'a>(&'a self, role: Role, session_model: &'a str) -> &'a str {
        let configured = match role {
            Role::Default => None,
            Role::Smol => self.smol.as_deref(),
            Role::Summary => self.summary.as_deref(),
        };
        configured.unwrap_or(session_model)
    }
}

/// A hard ceiling on what one session may spend, in tokens (prompt +
/// completion, provider-reported where available). Token-denominated rather
/// than dollars so it works for unpriced endpoints too; hosts with a pricing
/// catalog can convert a dollar cap into tokens at the session's rates.
#[derive(Debug, Clone, Copy)]
pub struct SessionBudget {
    /// Cumulative tokens after which the session refuses further model calls.
    pub max_session_tokens: usize,
    /// Percentage of the ceiling at which a warning is logged (soft line).
    pub warn_at_percent: u8,
}

impl SessionBudget {
    /// A budget with the default 80% warning line.
    pub fn new(max_session_tokens: usize) -> Self {
        Self {
            max_session_tokens,
            warn_at_percent: 80,
        }
    }

    /// The token count at which the soft warning fires.
    pub(crate) fn warn_threshold(&self) -> usize {
        self.max_session_tokens / 100 * usize::from(self.warn_at_percent.min(100))
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
    /// The model's maximum reply size in tokens, when known (reported by the
    /// endpoint's model catalog). Caps the per-request `max_tokens` so the
    /// harness never asks a model for more output than it can produce.
    /// `None` leaves `response_reserve` as the cap.
    pub max_output_tokens: Option<usize>,
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
    /// Prompt-cache breakpoint shaping for outbound requests (see
    /// [`crate::cache`]). Defaults to `Auto`: marked only for model families
    /// known to honor `cache_control`, ignored harmlessly elsewhere.
    pub prompt_cache: crate::cache::PromptCacheMode,
    /// Per-role model overrides (see [`ModelRoles`]). Unset roles use the
    /// session model — correct but expensive: a compaction summary re-reads a
    /// whole elided span, and a fleet lane greps, neither of which needs the
    /// model that writes the code.
    pub roles: ModelRoles,
    /// Where to append the developer request log (JSONL, one entry per model
    /// call — prompt size, cache-prefix diff, latency, retries, and the
    /// provider's reported usage including cached tokens). `None` disables it.
    /// Best-effort like the error log; never affects the turn.
    pub request_log: Option<PathBuf>,
    /// Hard session spend ceiling (see [`SessionBudget`]). `None` (the
    /// default) enforces nothing. When set, a turn that would exceed it stops
    /// gracefully with an explanation instead of silently running on.
    pub budget: Option<SessionBudget>,
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
    /// A cap on model rounds per turn (see [`RoundBudget`]). `None` (the
    /// default for the interactive session) runs until the model stops on
    /// its own; subagents get one so a lane that never converges can't run
    /// away with the fleet.
    pub round_budget: Option<RoundBudget>,
    /// How far down the tree this agent sits: 0 is the session's own agent,
    /// 1 a lane it spawned, 2 a lane of that lane. Set by
    /// [`AgentConfig::for_subagent`].
    pub depth: u8,
    /// The deepest level allowed. A lane at `depth < max_depth` may spawn
    /// its own fleet; one at the cap is a leaf with no spawn tools. Two is
    /// root → orchestrating lanes → leaves; deeper mostly buys cost variance.
    pub max_depth: u8,
    /// The wallet every lane of a root turn shares (see [`crate::tree`]).
    /// `None` on a root that never spawns; the fleet spawner creates one
    /// when the host didn't.
    pub tree: Option<Arc<crate::tree::TreeBudget>>,
}

/// A soft cap on how many model rounds one turn may take.
///
/// At `wrap_up_at` rounds the model is told to finish the current step and
/// report; at `stop_at` the turn ends with whatever it has, the same way
/// the loop guard ends an unproductive turn. Both are counts of model
/// calls, which is what a runaway lane actually spends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundBudget {
    pub wrap_up_at: u32,
    pub stop_at: u32,
}

impl RoundBudget {
    /// The budget every side agent (fleet lane, review step) runs under.
    pub const SUBAGENT: RoundBudget = RoundBudget {
        wrap_up_at: 40,
        stop_at: 60,
    };
}

impl AgentConfig {
    /// The configuration a detached subagent — a `side_agent`, a fleet lane —
    /// runs under, derived from its parent's. One builder for every subagent
    /// path, so the two can't drift on policy:
    ///
    /// - the model is routed through the `smol` role (lanes read and grep far
    ///   more than they write, and there are N of them);
    /// - the project's binary context is not re-attached (the parent's first
    ///   prompt already carried it; N lanes re-uploading every PDF is pure
    ///   cost);
    /// - the permission gate is demoted to its non-interactive form (a lane
    ///   can't drive the host's single approval prompt);
    /// - a round budget is installed, so a lane that never converges is
    ///   stopped instead of spending the fleet's whole allowance;
    /// - the system prompt loses the trail mandate, since `subagent_tools`
    ///   removes the tool it mandates, and gains the lane or leaf appendix
    ///   for its depth (see [`crate::prompt::subagent_appendix`]);
    /// - it sits one level deeper in the tree and shares the tree budget.
    pub fn for_subagent(&self) -> AgentConfig {
        let mut config = self.clone();
        config.model = config.roles.resolve(Role::Smol, &config.model).to_owned();
        config.initial_attachments.clear();
        config.permissions = config.permissions.map(|gate| Arc::new(gate.for_subagent()));
        config.round_budget = Some(RoundBudget::SUBAGENT);
        config.depth = self.depth.saturating_add(1);
        let appendix = crate::prompt::subagent_appendix(config.depth, config.max_depth);
        config.system_prompt = config.system_prompt.map(|p| {
            let mut prompt = crate::prompt::strip_trail_sections(&p);
            prompt.push_str(appendix);
            prompt
        });
        config
    }

    /// Whether an agent at this depth may spawn lanes of its own.
    pub fn may_spawn(&self) -> bool {
        self.depth < self.max_depth
    }

    /// The reply-size cap actually used for requests and prompt budgeting:
    /// the configured reserve, clamped down to the model's reported maximum
    /// output when the catalog knows it (a cap above what the model can
    /// produce would either error or silently mislead the budget).
    pub fn effective_response_reserve(&self) -> usize {
        match self.max_output_tokens {
            Some(max) if max > 0 => self.response_reserve.min(max),
            _ => self.response_reserve,
        }
    }
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
            max_output_tokens: None,
            max_resident_context_chars: 1_000_000,
            attachment_root: None,
            initial_attachments: Vec::new(),
            compression: CompressionMode::Off,
            prompt_cache: crate::cache::PromptCacheMode::default(),
            roles: ModelRoles::default(),
            request_log: None,
            budget: None,
            retry: RetryPolicy::default(),
            error_log: None,
            permissions: None,
            round_budget: None,
            depth: 0,
            max_depth: 2,
            tree: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_role_uses_the_session_model() {
        let roles = ModelRoles::default();
        assert_eq!(roles.resolve(Role::Smol, "opus"), "opus");
        assert_eq!(roles.resolve(Role::Summary, "opus"), "opus");
    }

    #[test]
    fn a_configured_role_routes_away_from_the_session_model() {
        let roles = ModelRoles {
            smol: Some("haiku".into()),
            summary: None,
        };
        assert_eq!(roles.resolve(Role::Smol, "opus"), "haiku");
        // Roles are independent: configuring one doesn't route the others.
        assert_eq!(roles.resolve(Role::Summary, "opus"), "opus");
        assert_eq!(roles.resolve(Role::Default, "opus"), "opus");
    }

    #[test]
    fn retry_delay_doubles_then_clamps() {
        let policy = RetryPolicy::default(); // 1s base
        assert_eq!(policy.delay_after(1), Duration::from_secs(1));
        assert_eq!(policy.delay_after(2), Duration::from_secs(2));
        assert_eq!(policy.delay_after(3), Duration::from_secs(4));
        // A deep (or misconfigured) attempt count must never produce an
        // hours-long sleep — the clamp holds even where 2^n overflows.
        assert_eq!(policy.delay_after(10), Duration::from_secs(60));
        assert_eq!(policy.delay_after(u32::MAX), Duration::from_secs(60));
    }
}
