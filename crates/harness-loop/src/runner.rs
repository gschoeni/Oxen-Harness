//! The loop engine: drive an agent through DISCOVER → QUESTION → PLAN →
//! EXECUTE → VERIFY → ITERATE until the verify gates pass or a stop condition
//! trips.
//!
//! Each pass hands the agent the goal, the success criteria, and a digest of
//! what's already been tried (so it doesn't repeat failures), runs one agent
//! turn (during which it can use tools and ask the user questions), then runs
//! the verify gates. The gates — not the agent's own say-so — decide success.
//!
//! Gates are conditional: a gate whose `run_when` doesn't match what the pass
//! actually changed on disk is skipped (a commit-only pass shouldn't pay for
//! the test suite). Two safety rules keep skipping honest: unknown changes
//! (not a git repo) run everything, and a gate the previous pass left failed
//! or blocked always re-runs, so a no-op pass can never launder a red gate
//! into success.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use harness_agent::{Agent, AgentEvent};
use serde::{Deserialize, Serialize};

use crate::changes::WorkspaceSnapshot;
use crate::journal::{Attempt, GateOutcome, GateReport, LoopJournal, VerifyOutcome};
use crate::spec::{LoopSpec, Verify};
use crate::LoopError;

/// Hard cap on captured verify output fed back to the model.
const MAX_OUTPUT_CHARS: usize = 12_000;

/// Why a loop stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum StopReason {
    /// The verify gate passed — the goal was met.
    Succeeded,
    /// Hit the iteration ceiling without passing.
    MaxIterations,
    /// Hit the cumulative token budget.
    TokenBudget { used: usize, budget: usize },
    /// The agent turn failed (LLM/tool/context error).
    Error { message: String },
}

impl StopReason {
    /// A one-line headline for the CLI/UI.
    pub fn headline(&self) -> String {
        match self {
            StopReason::Succeeded => "goal met — the gate is green".to_string(),
            StopReason::MaxIterations => "reached the iteration limit without passing".to_string(),
            StopReason::TokenBudget { used, budget } => {
                format!("token budget spent ({used} / {budget})")
            }
            StopReason::Error { message } => format!("the run errored: {message}"),
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, StopReason::Succeeded)
    }
}

/// Progress surfaced to a caller (CLI/UI) as a loop runs.
#[derive(Debug, Clone)]
pub enum LoopEvent {
    /// The loop began (or resumed).
    Started { max_iterations: u32 },
    /// A new pass started.
    IterationStarted { iteration: u32 },
    /// A streaming/tool event forwarded from the underlying agent turn.
    Agent(AgentEvent),
    /// A gate is about to run.
    VerifyStarted { gate: String, command_gate: bool },
    /// A conditional gate was skipped — the pass didn't change matching files.
    VerifySkipped { gate: String },
    /// The gate passed.
    VerifyPassed { gate: String },
    /// The gate failed, with the (truncated) detail fed back into the next pass.
    VerifyFailed { gate: String, detail: String },
    /// The loop stopped.
    Stopped { reason: StopReason },
}

/// Drives a [`LoopSpec`] against an [`Agent`] in a workspace.
pub struct LoopRunner {
    spec: LoopSpec,
    workspace_root: PathBuf,
    journal_path: Option<PathBuf>,
}

impl LoopRunner {
    pub fn new(spec: LoopSpec, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            spec,
            workspace_root: workspace_root.into(),
            journal_path: None,
        }
    }

    /// Persist the run journal to `path` after every iteration (resumability).
    pub fn persisting_to(mut self, path: impl Into<PathBuf>) -> Self {
        self.journal_path = Some(path.into());
        self
    }

    pub fn spec(&self) -> &LoopSpec {
        &self.spec
    }

    /// Run the loop from scratch.
    pub async fn run<F>(&self, agent: &mut Agent, on_event: F) -> Result<LoopJournal, LoopError>
    where
        F: FnMut(&LoopEvent),
    {
        let journal = LoopJournal::new(&self.spec.name, &self.spec.goal);
        self.drive(agent, journal, on_event).await
    }

    /// Continue a previously-saved run, building on its recorded attempts.
    pub async fn resume<F>(
        &self,
        agent: &mut Agent,
        journal: LoopJournal,
        on_event: F,
    ) -> Result<LoopJournal, LoopError>
    where
        F: FnMut(&LoopEvent),
    {
        self.drive(agent, journal, on_event).await
    }

    async fn drive<F>(
        &self,
        agent: &mut Agent,
        mut journal: LoopJournal,
        mut on_event: F,
    ) -> Result<LoopJournal, LoopError>
    where
        F: FnMut(&LoopEvent),
    {
        journal.stop = None;
        on_event(&LoopEvent::Started {
            max_iterations: self.spec.max_iterations,
        });

        let stop = loop {
            let iteration = journal.iterations() + 1;
            if iteration > self.spec.max_iterations {
                break StopReason::MaxIterations;
            }
            if let Some(budget) = self.spec.token_budget {
                let used = agent.tokens_used();
                if used >= budget {
                    break StopReason::TokenBudget { used, budget };
                }
            }

            on_event(&LoopEvent::IterationStarted { iteration });

            let snapshot = WorkspaceSnapshot::capture(&self.workspace_root);
            let prompt = self.compose_prompt(&journal);
            let turn = agent
                .run_turn(prompt, |e| on_event(&LoopEvent::Agent(e.clone())))
                .await;
            let final_text = match turn {
                Ok(t) => t,
                Err(e) => {
                    break StopReason::Error {
                        message: e.to_string(),
                    }
                }
            };

            // What did the pass actually change? `None` = unknown (run everything).
            let changed = snapshot.and_then(|s| s.changed_paths(&self.workspace_root));
            let pending = journal.pending_gates();
            let (outcome, gates) = self
                .run_gates(
                    agent,
                    &final_text,
                    changed.as_deref(),
                    &pending,
                    &mut on_event,
                )
                .await?;

            let passed = outcome.passed();
            journal.record(Attempt {
                iteration,
                summary: summarize(&final_text),
                verify: outcome,
                gates,
            });
            self.persist(&journal);

            if passed {
                break StopReason::Succeeded;
            }
        };

        journal.finish(stop.clone());
        self.persist(&journal);
        on_event(&LoopEvent::Stopped { reason: stop });
        Ok(journal)
    }

    fn persist(&self, journal: &LoopJournal) {
        if let Some(path) = &self.journal_path {
            if let Ok(json) = serde_json::to_string_pretty(journal) {
                let _ = std::fs::write(path, json);
            }
        }
    }

    /// Build the instruction for the next pass: the protocol + goal on the
    /// first pass, or a continuation carrying the last failure + journal digest.
    fn compose_prompt(&self, journal: &LoopJournal) -> String {
        let criteria = self.criteria_block();
        if journal.attempts.is_empty() {
            format!(
                "{PROTOCOL}\n\nGOAL:\n{}\n\nSUCCESS CRITERIA (strict — no soft passes):\n{}\n\n\
                 VERIFY GATES: after your turn I will check the work with: {}. \
                 Conditional gates only run when the pass changed matching files, \
                 so don't make gratuitous edits just to exercise them. \
                 Do not declare success yourself. Begin the loop now: discover, \
                 (ask if genuinely needed), plan, then execute. End your turn with a \
                 short summary of exactly what you changed this pass.",
                self.spec.goal,
                criteria,
                self.spec.gate_summary(),
            )
        } else {
            let last = journal
                .attempts
                .last()
                .map(|a| match &a.verify {
                    VerifyOutcome::Passed => "passed".to_string(),
                    VerifyOutcome::Failed { detail } => detail.clone(),
                })
                .unwrap_or_default();
            format!(
                "The verify gate did NOT pass yet — keep going toward the goal.\n\n\
                 GOAL:\n{}\n\nSUCCESS CRITERIA:\n{}\n\n\
                 LATEST VERIFY OUTPUT:\n{}\n\n\
                 WHAT YOU'VE ALREADY TRIED:\n{}\n\
                 Fix the single highest-impact problem first, then make the smallest \
                 change that moves the gate toward passing. Don't repeat a failed \
                 approach. End with a short summary of what you changed this pass.",
                self.spec.goal,
                criteria,
                truncate(&last, MAX_OUTPUT_CHARS),
                journal.digest(),
            )
        }
    }

    fn criteria_block(&self) -> String {
        if self.spec.success_criteria.is_empty() {
            "- The goal is fully and verifiably accomplished.".to_string()
        } else {
            self.spec
                .success_criteria
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    /// Run the pass's gate sequence in order, honoring each gate's `run_when`
    /// against what the pass changed. A gate the previous pass left pending
    /// (failed or blocked) always runs; a failure blocks the gates after it.
    /// The pass succeeds when every gate either passed or was legitimately
    /// skipped.
    async fn run_gates<F>(
        &self,
        agent: &Agent,
        work: &str,
        changed: Option<&[String]>,
        pending: &[String],
        on_event: &mut F,
    ) -> Result<(VerifyOutcome, Vec<GateReport>), LoopError>
    where
        F: FnMut(&LoopEvent),
    {
        let mut reports = Vec::new();
        let mut failure: Option<String> = None;

        for gate in self.spec.resolved_gates() {
            if failure.is_some() {
                reports.push(GateReport {
                    name: gate.name,
                    outcome: GateOutcome::Blocked,
                });
                continue;
            }
            let owes_a_run = pending.iter().any(|p| p == &gate.name);
            if !owes_a_run && !gate.run_when.should_run(changed) {
                on_event(&LoopEvent::VerifySkipped {
                    gate: gate.name.clone(),
                });
                reports.push(GateReport {
                    name: gate.name,
                    outcome: GateOutcome::Skipped,
                });
                continue;
            }

            on_event(&LoopEvent::VerifyStarted {
                gate: gate.name.clone(),
                command_gate: gate.verify.is_command(),
            });
            match self.check(agent, &gate.verify, work).await? {
                VerifyOutcome::Passed => {
                    on_event(&LoopEvent::VerifyPassed {
                        gate: gate.name.clone(),
                    });
                    reports.push(GateReport {
                        name: gate.name,
                        outcome: GateOutcome::Passed,
                    });
                }
                VerifyOutcome::Failed { detail } => {
                    on_event(&LoopEvent::VerifyFailed {
                        gate: gate.name.clone(),
                        detail: detail.clone(),
                    });
                    failure = Some(format!("gate `{}` failed — {detail}", gate.name));
                    reports.push(GateReport {
                        name: gate.name,
                        outcome: GateOutcome::Failed { detail },
                    });
                }
            }
        }

        let outcome = match failure {
            Some(detail) => VerifyOutcome::Failed { detail },
            None => VerifyOutcome::Passed,
        };
        Ok((outcome, reports))
    }

    /// Run one gate's check.
    async fn check(
        &self,
        agent: &Agent,
        verify: &Verify,
        work: &str,
    ) -> Result<VerifyOutcome, LoopError> {
        match verify {
            Verify::Command {
                command,
                timeout_ms,
            } => {
                let (code, output) = run_command(command, &self.workspace_root, *timeout_ms).await;
                if code == Some(0) {
                    Ok(VerifyOutcome::Passed)
                } else {
                    let code = code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "no exit code".to_string());
                    Ok(VerifyOutcome::Failed {
                        detail: format!(
                            "`{command}` exited with {code}\n{}",
                            truncate(&output, MAX_OUTPUT_CHARS)
                        ),
                    })
                }
            }
            Verify::Rubric { threshold } => self.rubric(agent, work, *threshold).await,
        }
    }

    /// Score the work with a separate, strict checker turn (the maker shouldn't
    /// grade its own homework). Passes only if every criterion scores high enough.
    async fn rubric(
        &self,
        agent: &Agent,
        work: &str,
        threshold: u8,
    ) -> Result<VerifyOutcome, LoopError> {
        let user = format!(
            "GOAL:\n{}\n\nSUCCESS CRITERIA:\n{}\n\nTHE WORKER'S REPORT OF THIS PASS:\n{}\n\n\
             Score each criterion from 1-10 (10 = fully, verifiably met). Be harsh; \
             you did NOT do this work. Output ONLY JSON of the form:\n\
             {{\"scores\":[{{\"criterion\":\"...\",\"score\":N}}],\"feedback\":\"what is still weak\"}}",
            self.spec.goal,
            self.criteria_block(),
            truncate(work, MAX_OUTPUT_CHARS),
        );
        let raw = agent.complete(RUBRIC_SYSTEM, &user).await?;
        Ok(score_rubric(&raw, threshold))
    }
}

const PROTOCOL: &str =
    "You are running an autonomous work LOOP toward a goal. Follow this protocol:\n\
     1. DISCOVER — read the relevant code/docs (fully, not skimmed) to work out what \
        actually needs doing, and copy the patterns and libraries already in use.\n\
     2. QUESTION — only if, after discovering, something is genuinely ambiguous or you're \
        missing context the user has, call `ask_user_question`. Don't ask about things you \
        can decide yourself.\n\
     3. PLAN — decide the approach and a concrete success criterion before you write code.\n\
     4. EXECUTE — do the work with your tools; make real changes. Keep them surgical and \
        simple: the smallest diff that solves the problem now, fixing root causes rather \
        than papering over symptoms. Don't reformat or touch code outside the task.\n\
     5. VERIFY — a gate checks the result after your turn (you don't grade yourself).\n\
     6. ITERATE — if it doesn't pass, you'll get the failure and try again.";

const RUBRIC_SYSTEM: &str =
    "You are a STRICT, skeptical verifier. You did not do the work; your only job is to \
     judge it honestly and harshly against the stated criteria. A criterion only scores \
     high if it is clearly, verifiably met by the report. Reward evidence, punish vague \
     claims. Output ONLY the requested JSON — no prose, no code fences.";

/// Parse the checker's JSON and decide pass/fail. Lenient: if it can't be
/// parsed, the gate fails with the raw text as feedback (never a silent pass).
fn score_rubric(raw: &str, threshold: u8) -> VerifyOutcome {
    let Some(value) = extract_json(raw) else {
        return VerifyOutcome::Failed {
            detail: format!(
                "verifier did not return parseable JSON: {}",
                truncate(raw, 400)
            ),
        };
    };
    let scores = value.get("scores").and_then(|s| s.as_array());
    let feedback = value
        .get("feedback")
        .and_then(|f| f.as_str())
        .unwrap_or("")
        .to_string();

    let Some(scores) = scores else {
        return VerifyOutcome::Failed {
            detail: format!("verifier returned no scores. {feedback}"),
        };
    };
    if scores.is_empty() {
        return VerifyOutcome::Failed {
            detail: format!("verifier returned no scores. {feedback}"),
        };
    }

    let mut lows = Vec::new();
    for s in scores {
        let score = s.get("score").and_then(|n| n.as_u64()).unwrap_or(0);
        if score < threshold as u64 {
            let criterion = s
                .get("criterion")
                .and_then(|c| c.as_str())
                .unwrap_or("criterion");
            lows.push(format!("{criterion}: {score}/10"));
        }
    }

    if lows.is_empty() {
        VerifyOutcome::Passed
    } else {
        VerifyOutcome::Failed {
            detail: format!("below the bar — {}. {feedback}", lows.join("; ")),
        }
    }
}

/// Pull the first JSON object out of a model response (handles stray prose/fences).
fn extract_json(raw: &str) -> Option<serde_json::Value> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&raw[start..=end]).ok()
}

/// Keep a worker turn's summary compact for the journal.
fn summarize(final_text: &str) -> String {
    truncate(final_text, 800)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max).collect();
    format!("{kept}\n… [truncated]")
}

/// Run `command` via the platform shell in `cwd`, returning the exit code (if
/// any) and combined stdout+stderr. A timeout abandons a hung command.
async fn run_command(command: &str, cwd: &Path, timeout_ms: u64) -> (Option<i32>, String) {
    let mut cmd = shell_command(command);
    cmd.current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match tokio::time::timeout(Duration::from_millis(timeout_ms), cmd.output()).await {
        Ok(Ok(output)) => {
            let mut combined = String::new();
            combined.push_str(&String::from_utf8_lossy(&output.stdout));
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.trim().is_empty() {
                combined.push_str("\n--- stderr ---\n");
                combined.push_str(&stderr);
            }
            (output.status.code(), combined)
        }
        Ok(Err(e)) => (None, format!("could not run `{command}`: {e}")),
        Err(_) => (
            None,
            format!("`{command}` exceeded {timeout_ms} ms and was abandoned"),
        ),
    }
}

#[cfg(windows)]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubric_passes_only_when_all_scores_clear_threshold() {
        let raw = r#"{"scores":[{"criterion":"a","score":9},{"criterion":"b","score":8}],"feedback":"good"}"#;
        assert!(matches!(score_rubric(raw, 8), VerifyOutcome::Passed));

        let raw = r#"prose {"scores":[{"criterion":"a","score":9},{"criterion":"b","score":5}],"feedback":"b weak"} trailing"#;
        match score_rubric(raw, 8) {
            VerifyOutcome::Failed { detail } => {
                assert!(detail.contains("b: 5/10"));
                assert!(detail.contains("weak"));
            }
            _ => panic!("expected failure"),
        }
    }

    #[test]
    fn rubric_fails_closed_on_unparseable_output() {
        assert!(matches!(
            score_rubric("I think it's fine honestly", 8),
            VerifyOutcome::Failed { .. }
        ));
    }

    #[test]
    fn extract_json_finds_object_amid_noise() {
        let v = extract_json("```json\n{\"a\":1}\n```").unwrap();
        assert_eq!(v["a"], 1);
        assert!(extract_json("no json here").is_none());
    }

    #[tokio::test]
    async fn command_gate_reads_exit_codes() {
        let dir = tempfile::tempdir().unwrap();
        let (code, out) = run_command("echo hi && exit 0", dir.path(), 10_000).await;
        assert_eq!(code, Some(0));
        assert!(out.contains("hi"));

        let (code, _) = run_command("exit 7", dir.path(), 10_000).await;
        assert_eq!(code, Some(7));
    }

    #[tokio::test]
    async fn command_gate_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let (code, out) = run_command("sleep 5", dir.path(), 80).await;
        assert_eq!(code, None);
        assert!(out.contains("abandoned"));
    }
}
