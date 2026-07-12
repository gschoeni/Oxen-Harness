//! Core domain types for oxen-harness.
//!
//! This crate holds the provider-agnostic message and role types that flow
//! through the agent loop, plus the defaults that pin the harness to Oxen.ai.

pub mod bounded;
pub mod fmt;
pub mod git;
pub mod json;
pub mod message;
pub mod text;

pub use message::{Message, Role};

/// Default model the harness talks to when none is configured.
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";

/// Base URL for the Oxen.ai OpenAI-compatible inference API.
///
/// The chat completions endpoint is `{DEFAULT_BASE_URL}/chat/completions`.
pub const DEFAULT_BASE_URL: &str = "https://hub.oxen.ai/api/ai";
