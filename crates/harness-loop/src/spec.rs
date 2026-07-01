//! The definition of a loop: a goal, a way to know when it's done, and a rule
//! for when to give up.
//!
//! A [`LoopSpec`] is the durable, shareable description of a job. It is
//! deliberately small — the heart of it is the [`Verify`] gate, because a loop
//! without a real check is just an agent agreeing with itself on repeat.

use serde::{Deserialize, Serialize};

/// Current loop file schema version. Bump on incompatible changes; files
/// written before versioning read back as this default.
pub const LOOP_SCHEMA_VERSION: u32 = 1;
fn default_loop_schema_version() -> u32 {
    LOOP_SCHEMA_VERSION
}

/// Default ceiling on iterations before a loop gives up and reports.
pub const DEFAULT_MAX_ITERATIONS: u32 = 8;
/// Default rubric pass threshold (each criterion must score at least this).
pub const DEFAULT_THRESHOLD: u8 = 8;
/// Default timeout for a verify command (5 minutes — builds/tests can be slow).
pub const DEFAULT_VERIFY_TIMEOUT_MS: u64 = 300_000;

fn default_max_iterations() -> u32 {
    DEFAULT_MAX_ITERATIONS
}
fn default_threshold() -> u8 {
    DEFAULT_THRESHOLD
}
fn default_timeout_ms() -> u64 {
    DEFAULT_VERIFY_TIMEOUT_MS
}

/// How a loop decides whether a pass succeeded — the gate that turns repetition
/// into progress.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Verify {
    /// Run a shell command in the workspace; **exit code 0 means pass**. This is
    /// the strongest gate — an objective check the model can't talk itself past.
    Command {
        command: String,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// A separate, strict checker scores the work 1–10 against each success
    /// criterion; the pass succeeds only if every score is at least `threshold`.
    /// Softer than a command (the checker is still an LLM), but useful when
    /// "done" can't be reduced to an exit code.
    Rubric {
        #[serde(default = "default_threshold")]
        threshold: u8,
    },
}

impl Default for Verify {
    fn default() -> Self {
        Verify::Rubric {
            threshold: DEFAULT_THRESHOLD,
        }
    }
}

impl Verify {
    /// True if this gate runs an objective command (vs. an LLM rubric).
    pub fn is_command(&self) -> bool {
        matches!(self, Verify::Command { .. })
    }

    /// A short human label for the gate.
    pub fn label(&self) -> String {
        match self {
            Verify::Command { command, .. } => format!("command: {command}"),
            Verify::Rubric { threshold } => format!("rubric (≥{threshold}/10 on every criterion)"),
        }
    }
}

/// A reusable, shareable loop definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopSpec {
    /// File-format version (see [`LOOP_SCHEMA_VERSION`]).
    #[serde(default = "default_loop_schema_version")]
    pub schema_version: u32,
    /// Human name (also the basis for the on-disk slug).
    pub name: String,
    /// One-line description for listings.
    #[serde(default)]
    pub description: String,
    /// The job: what should be true when the loop is done.
    pub goal: String,
    /// Concrete, strict criteria the work is checked against (no soft passes).
    #[serde(default)]
    pub success_criteria: Vec<String>,
    /// The gate. Defaults to a rubric when omitted.
    #[serde(default)]
    pub verify: Verify,
    /// Hard stop: give up and report after this many iterations.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// Optional hard stop on cumulative token spend (estimated by the agent).
    #[serde(default)]
    pub token_budget: Option<usize>,
}

impl LoopSpec {
    /// A minimal ad-hoc loop from a goal, using a rubric gate and defaults.
    pub fn from_goal(goal: impl Into<String>) -> Self {
        Self {
            schema_version: LOOP_SCHEMA_VERSION,
            name: "ad-hoc".to_string(),
            description: String::new(),
            goal: goal.into(),
            success_criteria: Vec::new(),
            verify: Verify::default(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            token_budget: None,
        }
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }
}

/// A normalized, filesystem-safe identifier derived from a loop's name.
///
/// Thin wrapper over [`harness_core::text::slug`] that pins the empty-name
/// fallback to `"loop"`.
pub fn slug(name: &str) -> String {
    harness_core::text::slug(name, "loop")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_in_when_omitted() {
        let spec: LoopSpec = toml::from_str(
            r#"
            name = "Tidy"
            goal = "make the repo tidy"
        "#,
        )
        .unwrap();
        assert_eq!(spec.max_iterations, DEFAULT_MAX_ITERATIONS);
        assert!(matches!(spec.verify, Verify::Rubric { threshold: 8 }));
        assert!(spec.success_criteria.is_empty());
        assert!(spec.token_budget.is_none());
    }

    #[test]
    fn command_verify_round_trips_through_toml() {
        let spec = LoopSpec {
            schema_version: LOOP_SCHEMA_VERSION,
            name: "Green tests".into(),
            description: "all tests pass".into(),
            goal: "make the test suite green".into(),
            success_criteria: vec!["cargo test passes".into()],
            verify: Verify::Command {
                command: "cargo test".into(),
                timeout_ms: 600_000,
            },
            max_iterations: 10,
            token_budget: Some(500_000),
        };
        let toml = spec.to_toml().unwrap();
        let back = LoopSpec::from_toml_str(&toml).unwrap();
        assert_eq!(spec, back);
        assert!(back.verify.is_command());
    }

    #[test]
    fn slugs_are_filesystem_safe() {
        assert_eq!(slug("Green Tests"), "green-tests");
        assert_eq!(slug("  Make  it!! green "), "make-it-green");
        assert_eq!(slug("***"), "loop");
    }
}
