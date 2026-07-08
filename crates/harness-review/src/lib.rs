//! Configurable multi-step code review: an ordered list of prompt steps
//! (find → verify → report by default) run against a diff, each step's output
//! feeding the next, ending in machine-readable findings a fixing agent can
//! act on.
//!
//! - [`ReviewConfig`] — the durable, user-editable pipeline definition
//!   (`~/.oxen-harness/code-review.json`), with built-in defaults.
//! - [`ReviewTarget`] / [`target::resolve_target`] — what to review
//!   (uncommitted changes, or PR-style against a base branch) and the
//!   mechanically-computed diff that anchors every step.
//! - [`ReviewRunner`] — walks the steps, each on an isolated
//!   [`Agent::side_agent`](harness_agent::Agent::side_agent) so the verifier
//!   isn't anchored by the finder's context.
//! - [`ReviewReport`] / [`Finding`] — the lenient-parsed result, rendered as
//!   markdown and injected into the session so "fix 1 and 3" just works.

pub mod config;
pub mod findings;
pub mod prompts;
pub mod runner;
pub mod target;

pub use config::{
    ReviewConfig, ReviewStep, StepAgent, DEFAULT_MAX_PARALLEL, REVIEW_SCHEMA_VERSION,
};
pub use findings::{Finding, ReviewReport};
pub use prompts::default_steps;
pub use runner::{session_exchange, ReviewEvent, ReviewRunner};
pub use target::{resolve_target, ReviewInput, ReviewTarget};

/// Errors from resolving or running a review.
#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("nothing to review — the target has no changes")]
    NothingToReview,
    #[error("the review was stopped")]
    Cancelled,
    #[error("{0}")]
    Git(String),
    #[error(transparent)]
    Agent(#[from] harness_agent::AgentError),
    #[error(transparent)]
    Config(#[from] harness_config::ConfigError),
}
