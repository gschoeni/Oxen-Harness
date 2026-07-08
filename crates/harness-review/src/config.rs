//! The durable definition of the review pipeline: an ordered list of named
//! steps, each one or more user-editable prompt templates.
//!
//! The defaults encode the pipeline the strongest production reviewers
//! (Claude Code's `/code-review`, Codex's `/review`) converged on: a
//! recall-biased **find** pass that is told not to self-censor — fanned out
//! across three parallel reviewers, each with a narrow lens — an adversarial
//! **verify** pass that tries to refute each candidate with quoted evidence,
//! and a **report** pass that drops refuted candidates, dedups, ranks, caps,
//! and emits machine-readable findings. Users can edit any prompt, reorder,
//! add, or remove steps (and split any step into parallel agents) from
//! Settings — the runner just walks the list, feeding each step's combined
//! output to the next.

use serde::{Deserialize, Serialize};

use crate::prompts::default_steps;
use crate::ReviewError;

/// Current `code-review.json` schema version. Bump on incompatible changes.
/// v2 added per-step parallel `agents` and `max_parallel`.
pub const REVIEW_SCHEMA_VERSION: u32 = 2;

/// Default cap on findings in the final report.
pub const DEFAULT_MAX_FINDINGS: usize = 10;

/// Default cap on subagents running at once within a fan-out step.
pub const DEFAULT_MAX_PARALLEL: usize = 3;

/// Hard cap on the diff text substituted into a step prompt. Bigger changes
/// are truncated with a note — the step agents have git and file tools, so
/// they can read the rest themselves.
pub const DIFF_CHAR_BUDGET: usize = 60_000;

fn default_max_findings() -> usize {
    DEFAULT_MAX_FINDINGS
}
fn default_max_parallel() -> usize {
    DEFAULT_MAX_PARALLEL
}

/// One parallel reviewer within a fan-out step: a display name and its prompt
/// template (same placeholders as [`ReviewStep::prompt`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepAgent {
    /// Short lane label ("diff-scan", "callers") shown in progress output.
    pub name: String,
    /// The prompt template this reviewer runs.
    pub prompt: String,
}

/// One step of the pipeline: a name, and either a single prompt or a set of
/// parallel `agents` (a fan-out). Prompt templates may use these placeholders,
/// substituted per run:
///
/// - `{{target}}` — what is being reviewed ("uncommitted changes", "changes
///   against `main`"), plus the untracked-file list when relevant.
/// - `{{diff}}` — the unified diff of the change (truncated past
///   [`DIFF_CHAR_BUDGET`]).
/// - `{{previous}}` — the previous step's full output (empty note on step 1;
///   for a fan-out step, the agents' outputs concatenated under `###`
///   headings, in agent order).
/// - `{{max_findings}}` — the configured findings cap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewStep {
    /// Short identifier ("find", "verify") used in events and progress output.
    pub name: String,
    /// The prompt template for a single-agent step. Ignored when `agents` is
    /// non-empty (kept optional so v1 configs load unchanged).
    #[serde(default)]
    pub prompt: String,
    /// Parallel reviewers for this step. Empty = a single agent running
    /// `prompt`; non-empty = a fan-out, all agents run concurrently (capped by
    /// [`ReviewConfig::max_parallel`]) and their outputs are concatenated as
    /// the step's output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<StepAgent>,
}

impl ReviewStep {
    /// The reviewers this step actually runs: `agents` when set, else one
    /// agent named after the step running `prompt`.
    pub fn resolved_agents(&self) -> Vec<StepAgent> {
        if self.agents.is_empty() {
            vec![StepAgent {
                name: self.name.clone(),
                prompt: self.prompt.clone(),
            }]
        } else {
            self.agents.clone()
        }
    }

    /// Whether this step fans out across parallel agents.
    pub fn is_fan_out(&self) -> bool {
        self.agents.len() > 1
    }
}

/// The shareable, user-editable review pipeline definition. Persisted inside
/// the standard [`harness_config::io::Versioned`] envelope, which carries the
/// schema version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewConfig {
    /// The steps run in order; each step's output feeds the next via
    /// `{{previous}}`. Empty reads as the built-in defaults.
    #[serde(default)]
    pub steps: Vec<ReviewStep>,
    /// Cap on findings in the final report.
    #[serde(default = "default_max_findings")]
    pub max_findings: usize,
    /// Cap on subagents running at once within a fan-out step.
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            steps: default_steps(),
            max_findings: DEFAULT_MAX_FINDINGS,
            max_parallel: DEFAULT_MAX_PARALLEL,
        }
    }
}

impl ReviewConfig {
    /// The effective step sequence: the configured steps, or the built-in
    /// defaults when the list is empty (a fresh install, or a user who
    /// deleted every step).
    pub fn resolved_steps(&self) -> Vec<ReviewStep> {
        if self.steps.is_empty() {
            default_steps()
        } else {
            self.steps.clone()
        }
    }

    /// Read the saved pipeline, falling back to the defaults when the file is
    /// missing or unreadable — config is never a hard failure.
    pub fn load() -> Self {
        harness_config::paths::code_review_file()
            .map(|p| harness_config::io::read_versioned::<Self>(&p).1)
            .map(|c| {
                // A default-constructed payload (missing/empty file) or an
                // emptied step list both mean "the built-in pipeline".
                if c.steps.is_empty() {
                    Self {
                        steps: default_steps(),
                        ..c
                    }
                } else {
                    c
                }
            })
            .unwrap_or_default()
    }

    /// Atomically persist the pipeline to `~/.oxen-harness/code-review.json`.
    pub fn save(&self) -> Result<(), ReviewError> {
        let path = harness_config::paths::code_review_file()?;
        harness_config::io::write_versioned(&path, REVIEW_SCHEMA_VERSION, self)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_three_step_pipeline_with_a_fanned_out_finder() {
        let config = ReviewConfig::default();
        let names: Vec<_> = config.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["find", "verify", "report"]);
        assert_eq!(config.max_findings, DEFAULT_MAX_FINDINGS);
        assert_eq!(config.max_parallel, DEFAULT_MAX_PARALLEL);

        // The find step fans out across three lenses; every finder prompt
        // wires in the placeholders the runner fills.
        let find = &config.steps[0];
        assert!(find.is_fan_out());
        let lens_names: Vec<_> = find.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(lens_names, ["diff-scan", "removed-code", "callers"]);
        for agent in &find.agents {
            assert!(agent.prompt.contains("{{target}}"), "{}", agent.name);
            assert!(agent.prompt.contains("{{diff}}"), "{}", agent.name);
        }

        // Verify and report stay single-agent.
        assert!(!config.steps[1].is_fan_out());
        assert_eq!(config.steps[1].resolved_agents().len(), 1);
        assert!(config.steps[1].prompt.contains("{{previous}}"));
        assert!(config.steps[2].prompt.contains("{{max_findings}}"));
    }

    #[test]
    fn empty_step_list_resolves_to_defaults() {
        let config = ReviewConfig {
            steps: Vec::new(),
            max_findings: 5,
            max_parallel: 2,
        };
        assert_eq!(config.resolved_steps(), default_steps());
    }

    #[test]
    fn a_v1_config_with_only_a_prompt_still_loads_as_one_agent() {
        let config: ReviewConfig = serde_json::from_str(
            r#"{"steps":[{"name":"solo","prompt":"Review {{diff}}."}],"max_findings":5}"#,
        )
        .unwrap();
        let step = &config.steps[0];
        assert!(!step.is_fan_out());
        let agents = step.resolved_agents();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "solo");
        assert_eq!(agents[0].prompt, "Review {{diff}}.");
        // Fields v1 files don't have fill from defaults.
        assert_eq!(config.max_parallel, DEFAULT_MAX_PARALLEL);
    }

    #[test]
    fn config_round_trips_through_json() {
        let config = ReviewConfig {
            steps: vec![
                ReviewStep {
                    name: "solo".into(),
                    prompt: "Review {{diff}} and report.".into(),
                    agents: Vec::new(),
                },
                ReviewStep {
                    name: "fan".into(),
                    prompt: String::new(),
                    agents: vec![
                        StepAgent {
                            name: "a".into(),
                            prompt: "look left".into(),
                        },
                        StepAgent {
                            name: "b".into(),
                            prompt: "look right".into(),
                        },
                    ],
                },
            ],
            max_findings: 3,
            max_parallel: 2,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: ReviewConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }
}
