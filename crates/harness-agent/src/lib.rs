//! The agentic loop for oxen-harness.
//!
//! This crate sits above [`harness_llm`], [`harness_tools`], and
//! [`harness_store`] and wires them into the runtime loop:
//!
//! 1. Add the user's message to the transcript (and persist it).
//! 2. Call the model (streaming) with the available tool definitions.
//! 3. If the model requested tool calls, execute each tool, append the results
//!    as `tool` messages, and loop.
//! 4. Otherwise, return the assistant's final text.
//!
//! Every message (user, assistant, tool) is persisted verbatim to the history
//! store as it is produced.
//!
//! # Layout
//!
//! - [`Agent`] (in `agent/`) — construction, session lifecycle, and accessors;
//!   its `turn` child module holds the model/tool loop, and `compression` the
//!   outbound-request compression wiring.
//! - [`AgentConfig`] / [`RetryPolicy`] — per-agent configuration.
//! - [`AgentEvent`] — the stream of progress events a host renders live.
//! - [`AgentError`] — the crate error, wrapping the capability crates' errors.
//! - [`budget`] — token estimation and context-window budgeting.
//! - [`compact`] — pruning + summarization used when the transcript outgrows
//!   the window.
//! - [`fleet`] — run N subagents in parallel on detached side agents, with a
//!   concurrency cap and one multiplexed event stream.
//! - `prompt` — the default system prompt and the turn-corrective nudges
//!   (re-exported below).

mod agent;
mod config;
pub mod errlog;
mod error;
mod event;
mod interject;
mod prompt;

pub mod budget;
pub mod compact;
pub mod fleet;
pub mod fleet_tool;

#[cfg(test)]
mod test_support;

pub use agent::Agent;
pub use config::{AgentConfig, RetryPolicy};
pub use error::AgentError;
pub use event::AgentEvent;
pub use fleet_tool::{FleetSpawner, FleetTool, FLEET_TOOL};
pub use interject::Interjections;
pub use prompt::{
    default_system_prompt, environment_section, system_prompt_with, system_prompt_with_env,
    OptionalTools,
};
