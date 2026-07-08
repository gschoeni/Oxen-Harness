//! The durable definition of the review pipeline: an ordered list of named
//! steps, each a user-editable prompt template.
//!
//! The defaults encode the pipeline the strongest production reviewers
//! (Claude Code's `/code-review`, Codex's `/review`) converged on: a
//! recall-biased **find** pass that is told not to self-censor, an adversarial
//! **verify** pass that tries to refute each candidate with quoted evidence,
//! and a **report** pass that drops refuted candidates, dedups, ranks, caps,
//! and emits machine-readable findings. Users can edit any prompt, reorder,
//! add, or remove steps from Settings — the runner just walks the list,
//! feeding each step's output to the next.

use serde::{Deserialize, Serialize};

use crate::ReviewError;

/// Current `code-review.json` schema version. Bump on incompatible changes.
pub const REVIEW_SCHEMA_VERSION: u32 = 1;

/// Default cap on findings in the final report.
pub const DEFAULT_MAX_FINDINGS: usize = 10;

/// Hard cap on the diff text substituted into a step prompt. Bigger changes
/// are truncated with a note — the step agents have git and file tools, so
/// they can read the rest themselves.
pub const DIFF_CHAR_BUDGET: usize = 60_000;

fn default_max_findings() -> usize {
    DEFAULT_MAX_FINDINGS
}

/// One step of the pipeline: a name (shown in progress output) and a prompt
/// template. Templates may use these placeholders, substituted per run:
///
/// - `{{target}}` — what is being reviewed ("uncommitted changes", "changes
///   against `main`"), plus the untracked-file list when relevant.
/// - `{{diff}}` — the unified diff of the change (truncated past
///   [`DIFF_CHAR_BUDGET`]).
/// - `{{previous}}` — the previous step's full output (empty note on step 1).
/// - `{{max_findings}}` — the configured findings cap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewStep {
    /// Short identifier ("find", "verify") used in events and progress output.
    pub name: String,
    /// The prompt template sent to this step's agent.
    pub prompt: String,
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
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            steps: default_steps(),
            max_findings: DEFAULT_MAX_FINDINGS,
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
                        max_findings: c.max_findings,
                        steps: default_steps(),
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

/// The built-in find → verify → report pipeline.
pub fn default_steps() -> Vec<ReviewStep> {
    vec![
        ReviewStep {
            name: "find".to_string(),
            prompt: FIND_PROMPT.to_string(),
        },
        ReviewStep {
            name: "verify".to_string(),
            prompt: VERIFY_PROMPT.to_string(),
        },
        ReviewStep {
            name: "report".to_string(),
            prompt: REPORT_PROMPT.to_string(),
        },
    ]
}

/// Step 1 — recall-biased finder. Told explicitly not to self-censor: the
/// verify step exists to restore precision, and finders that silently drop
/// half-believed candidates bypass it.
pub const FIND_PROMPT: &str = "\
You are reviewing a code change for a teammate. This pass is about RECALL: surface every candidate issue a careful reviewer could find. An independent verifier will judge each candidate next, so do not self-censor — pass through every candidate with a nameable failure scenario, even ones you only half believe.

TARGET: {{target}}

Use your tools to go beyond the diff: read the enclosing function of every changed hunk, find the callers of changed functions and check each call site, and for every line the diff deletes or replaces, work out what invariant it enforced and where the new code re-establishes it. Read any untracked files listed in the target.

Flag only issues introduced or re-exposed by this change — not pre-existing problems. Look for, in priority order:
1. Runtime correctness: inverted or wrong conditions, off-by-one, null/None/undefined dereference on a reachable path, missing error handling or await, removed guards or validation, wrong-variable copy-paste, swallowed errors that should propagate, ordering/race problems.
2. Security: injection (SQL, command, path), missing authorization on new surfaces, secrets in code or logs, unsafe deserialization.
3. Broken contracts: changed return shapes, new preconditions, or new failure modes that break existing callers; API or schema changes without migration.

Ignore style, naming, formatting, and missing tests unless they hide a real defect.

THE CHANGE:
{{diff}}

End your reply with ONLY a JSON array of candidates (no code fences, no prose after it):
[{\"title\": \"<one line, imperative>\", \"file\": \"path/from/repo/root\", \"line\": 123, \"summary\": \"<one sentence: what is wrong>\", \"failure_scenario\": \"<the concrete inputs or state that trigger it, and the wrong output, crash, or data loss that results>\"}]
If there are no candidates, end with [].";

/// Step 2 — adversarial verifier. Judges each candidate independently against
/// the actual code, with a three-state verdict and quoted evidence.
pub const VERIFY_PROMPT: &str = "\
You are a skeptical, adversarial verifier on a code review. You did NOT write the candidate findings below; your job is to try to REFUTE each one by reading the actual code with your tools. Do not take the finder's word for anything — check every claim against the source.

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
    fn defaults_are_the_three_step_pipeline() {
        let config = ReviewConfig::default();
        let names: Vec<_> = config.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["find", "verify", "report"]);
        assert_eq!(config.max_findings, DEFAULT_MAX_FINDINGS);
        // Every default prompt wires in the placeholders the runner fills.
        for step in &config.steps {
            assert!(step.prompt.contains("{{target}}"), "{}", step.name);
        }
        assert!(config.steps[0].prompt.contains("{{diff}}"));
        assert!(config.steps[1].prompt.contains("{{previous}}"));
        assert!(config.steps[2].prompt.contains("{{max_findings}}"));
    }

    #[test]
    fn empty_step_list_resolves_to_defaults() {
        let config = ReviewConfig {
            steps: Vec::new(),
            max_findings: 5,
        };
        assert_eq!(config.resolved_steps(), default_steps());
    }

    #[test]
    fn config_round_trips_through_json() {
        let config = ReviewConfig {
            steps: vec![ReviewStep {
                name: "solo".into(),
                prompt: "Review {{diff}} and report.".into(),
            }],
            max_findings: 3,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: ReviewConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }
}
