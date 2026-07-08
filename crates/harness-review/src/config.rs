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

/// The built-in find → verify → report pipeline. The find step fans out
/// across three parallel reviewers, each with a narrow lens (the angles the
/// strongest production reviewers use): a line-by-line diff scan, a
/// removed-behavior audit, and a cross-file caller trace.
pub fn default_steps() -> Vec<ReviewStep> {
    vec![
        ReviewStep {
            name: "find".to_string(),
            prompt: String::new(),
            agents: vec![
                StepAgent {
                    name: "diff-scan".to_string(),
                    prompt: finder_prompt(DIFF_SCAN_LENS),
                },
                StepAgent {
                    name: "removed-code".to_string(),
                    prompt: finder_prompt(REMOVED_CODE_LENS),
                },
                StepAgent {
                    name: "callers".to_string(),
                    prompt: finder_prompt(CALLERS_LENS),
                },
            ],
        },
        ReviewStep {
            name: "verify".to_string(),
            prompt: VERIFY_PROMPT.to_string(),
            agents: Vec::new(),
        },
        ReviewStep {
            name: "report".to_string(),
            prompt: REPORT_PROMPT.to_string(),
            agents: Vec::new(),
        },
    ]
}

/// Build one finder's prompt: the shared recall framing around a single lens.
/// Told explicitly not to self-censor — the verify step exists to restore
/// precision, and finders that silently drop half-believed candidates bypass
/// it.
pub fn finder_prompt(lens: &str) -> String {
    format!(
        "\
You are one of several independent reviewers examining the same code change in parallel, each from a different angle. This pass is about RECALL: surface every candidate issue your angle can find. An independent verifier will judge every candidate next, so do not self-censor — pass through every candidate with a nameable failure scenario, even ones you only half believe. Other reviewers cover the other angles; stay on yours.

YOUR ANGLE: {lens}

Flag only issues introduced or re-exposed by this change — not pre-existing problems. Ignore style, naming, formatting, and missing tests unless they hide a real defect. Read any untracked files listed in the target.

TARGET: {{{{target}}}}

THE CHANGE:
{{{{diff}}}}

End your reply with ONLY a JSON array of candidates (no code fences, no prose after it):
[{{\"title\": \"<one line, imperative>\", \"file\": \"path/from/repo/root\", \"line\": 123, \"summary\": \"<one sentence: what is wrong>\", \"failure_scenario\": \"<the concrete inputs or state that trigger it, and the wrong output, crash, or data loss that results>\"}}]
If there are no candidates, end with []."
    )
}

/// Finder lens 1 — line-by-line scan of the hunks themselves.
pub const DIFF_SCAN_LENS: &str = "\
Read every hunk in the diff line by line, then use your tools to read the enclosing function of each hunk. For every changed line ask: what input, state, timing, or platform makes this line wrong? Look for inverted or wrong conditions, off-by-one, null/None/undefined dereference on a reachable path, missing error handling or await, wrong-variable copy-paste, swallowed errors that should propagate, falsy-zero checks, and the classic pitfalls of the diff's language (mutable default arguments, closure-captured loop variables, == coercion, integer overflow, races).";

/// Finder lens 2 — what the removed code was protecting.
pub const REMOVED_CODE_LENS: &str = "\
For every line the diff DELETES or replaces, name the invariant or behavior it enforced, then search the new code for where that invariant is re-established. If you cannot find it, that is a candidate: a removed guard, a dropped error path, narrowed validation, a deleted check that was covering a real case, cleanup that no longer runs.";

/// Finder lens 3 — the blast radius beyond the diff.
pub const CALLERS_LENS: &str = "\
For each function, type, or contract the diff changes, use your tools to find its callers (search for the symbol across the repo) and check whether the change breaks any call site: a new precondition, a changed return shape or error type, a new failure mode, an ordering dependency. Also check the callees the new code relies on — does it hold their contracts? Include security exposure: injection through new inputs (SQL, command, path), missing authorization on new surfaces, secrets in code or logs.";

/// Step 2 — adversarial verifier. Judges each candidate independently against
/// the actual code, with a three-state verdict and quoted evidence.
pub const VERIFY_PROMPT: &str = "\
You are a skeptical, adversarial verifier on a code review. You did NOT write the candidate findings below; your job is to try to REFUTE each one by reading the actual code with your tools. Do not take the finders' word for anything — check every claim against the source. The candidates come from several independent reviewers working in parallel, so some may overlap or describe the same defect — judge each claim on its own; a later step merges duplicates.

TARGET: {{target}}

Give each candidate exactly one verdict:
- CONFIRMED — you can name the inputs or state that trigger it and the wrong output or crash that results. Quote the offending line.
- PLAUSIBLE — the mechanism is real but the trigger is uncertain (timing, environment, config). Say what would confirm it. Realistic-but-rare paths (error handlers, cold caches, missing optional fields, boundary values) are PLAUSIBLE, not refuted — do not refute a candidate merely for being \"speculative\".
- REFUTED — the claim is factually wrong (the code doesn't say that), provably impossible (a type, constant, or invariant rules it out — show it), or already guarded elsewhere. Quote the line that proves it.

CANDIDATES:
{{previous}}

THE CHANGE:
{{diff}}

End your reply with ONLY a JSON array carrying every candidate plus your verdict and evidence (no code fences, no prose after it):
[{\"title\": \"...\", \"file\": \"...\", \"line\": 123, \"summary\": \"...\", \"failure_scenario\": \"...\", \"verdict\": \"CONFIRMED|PLAUSIBLE|REFUTED\", \"evidence\": \"<the quoted line(s) and one sentence of reasoning>\"}]";

/// Step 3 — report writer. Pure synthesis: drop refuted, merge duplicates,
/// rank, cap, and emit the machine-readable report a fixing agent consumes.
pub const REPORT_PROMPT: &str = "\
Write the final code-review report from the verified candidates below. Work only from what is given — do not invent new findings.

TARGET: {{target}}

Rules:
1. Drop every REFUTED candidate.
2. Merge candidates that describe the same root cause, keeping the strongest evidence.
3. Rank most-severe first: correctness and security beat everything else; CONFIRMED beats PLAUSIBLE.
4. Tag each finding with a priority: 0 = drop everything and fix (universal breakage), 1 = urgent, should not merge as-is, 2 = should be fixed soon, 3 = nice to have.
5. Keep at most {{max_findings}} findings. If nothing survives, return an empty list — a clean review is a valid result; do not pad.
6. Each body is at most one short paragraph, matter-of-fact, and states the conditions under which the issue occurs. No praise, no filler.

VERIFIED CANDIDATES:
{{previous}}

Reply with ONLY this JSON (no code fences, no prose before or after):
{\"findings\": [{\"title\": \"<one line, imperative>\", \"file\": \"path/from/repo/root\", \"line\": 123, \"priority\": 1, \"verdict\": \"CONFIRMED\", \"body\": \"<why this is a bug and when it bites>\", \"failure_scenario\": \"<inputs/state → wrong outcome>\"}], \"overall_correctness\": \"correct|incorrect\", \"overall_explanation\": \"<1-2 sentences>\"}";

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
