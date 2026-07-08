//! The default pipeline's prompt text — the part of the review most worth
//! reading and most worth editing.
//!
//! Everything here is a *default*: it materializes into
//! `~/.oxen-harness/code-review.json` the first time the user saves, and from
//! then on their copy wins. The prompts encode the precision discipline the
//! strongest production reviewers converged on — recall-biased finders that
//! must not self-censor (the adversarial verifier restores precision), quoted
//! evidence for every verdict, and a report that would rather be empty than
//! padded. Template placeholders are documented on
//! [`ReviewStep`](crate::config::ReviewStep).

use crate::config::{ReviewStep, StepAgent};

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
