//! Goal-driven, self-verifying agent loops for oxen-harness.
//!
//! A *prompt* hands the agent an instruction. A *loop* hands it a job, a way to
//! know when the job is done, and a rule for when to give up. This crate turns
//! the [`Agent`](harness_agent::Agent) into exactly that, running the cycle:
//!
//! ```text
//! DISCOVER → QUESTION → PLAN → EXECUTE → VERIFY → ITERATE
//! ```
//!
//! The three things that make it a real loop (rather than an agent agreeing
//! with itself on repeat):
//!
//! - **A gate** ([`Verify`]) that can *fail* the work — a real command (exit 0)
//!   or a strict, separate-checker rubric. This is the heart of the loop.
//! - **State** ([`LoopJournal`]) recording what's been tried and what failed,
//!   fed into each pass and persisted for resuming.
//! - **Stop conditions** ([`StopReason`]) — success *and* a hard limit
//!   (iterations + optional token budget) so it can't run all night for nothing.
//!
//! Loops are defined by a small, shareable [`LoopSpec`] and managed on disk by
//! [`LoopStore`]; a few [`builtins`] ship ready to run (notably `default`, the
//! "make the checks green" loop this project runs on itself).

pub mod builtins;
mod journal;
mod runner;
mod spec;
mod store;

pub use journal::{Attempt, LoopJournal, VerifyOutcome};
pub use runner::{LoopEvent, LoopRunner, StopReason};
pub use spec::{
    slug, LoopSpec, Verify, DEFAULT_MAX_ITERATIONS, DEFAULT_THRESHOLD, DEFAULT_VERIFY_TIMEOUT_MS,
    LOOP_SCHEMA_VERSION,
};
pub use store::{LoopStore, LoopSummary};

/// Errors that can arise while defining, storing, or running a loop.
#[derive(Debug, thiserror::Error)]
pub enum LoopError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("serializing loop: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("parsing loop: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("serializing run journal: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not determine home directory")]
    NoHome,
    #[error("no loop named `{0}`")]
    NotFound(String),
    #[error(transparent)]
    Agent(#[from] harness_agent::AgentError),
}
