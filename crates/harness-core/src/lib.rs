//! Core domain types for oxen-harness.
//!
//! This crate holds the provider-agnostic message and role types that flow
//! through the agent loop, plus the defaults that pin the harness to Oxen.ai.

pub mod attach;
pub mod bounded;
pub mod fmt;
pub mod git;
pub mod json;
pub mod message;
pub mod text;

pub use message::{Message, Role};

/// The key a model call is priced and ledgered under: the model id on its
/// default tier, `{model}@{tier}` on a named service tier (e.g.
/// `gpt-6-1-sol@ultrafast`). Tiers bill at their own rates, so the usage
/// ledger records the tier in the model column and every pricing catalog
/// carries an entry per priced tier — existing spend reports then price tier
/// calls correctly without knowing tiers exist.
pub fn billing_key(model: &str, tier: Option<&str>) -> String {
    match tier.filter(|t| !t.is_empty()) {
        Some(tier) => format!("{model}@{tier}"),
        None => model.to_string(),
    }
}

/// Default model the harness talks to when none is configured.
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";

/// Base URL for the Oxen.ai OpenAI-compatible inference API.
///
/// The chat completions endpoint is `{DEFAULT_BASE_URL}/chat/completions`.
pub const DEFAULT_BASE_URL: &str = "https://hub.oxen.ai/api/ai";
