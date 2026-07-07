//! The loop's memory: a small record of what's been tried, what failed, and
//! how it ended — so a later pass resumes instead of repeating mistakes.

use serde::{Deserialize, Serialize};

use crate::runner::StopReason;

/// The overall outcome of one pass's verify sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum VerifyOutcome {
    Passed,
    Failed { detail: String },
}

impl VerifyOutcome {
    pub fn passed(&self) -> bool {
        matches!(self, VerifyOutcome::Passed)
    }
}

/// What happened to one gate within a pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum GateOutcome {
    Passed,
    Failed {
        detail: String,
    },
    /// Its `run_when` condition didn't match this pass — counts as satisfied.
    Skipped,
    /// An earlier gate failed before this one could run — still pending, so it
    /// must run on the next pass even without new changes.
    Blocked,
}

impl GateOutcome {
    /// True if this gate still owes a real run on the next pass: it failed, or
    /// never got to run because an earlier gate failed.
    pub fn is_pending(&self) -> bool {
        matches!(self, GateOutcome::Failed { .. } | GateOutcome::Blocked)
    }

    /// A compact verdict word for digests.
    fn word(&self) -> &'static str {
        match self {
            GateOutcome::Passed => "passed",
            GateOutcome::Failed { .. } => "failed",
            GateOutcome::Skipped => "skipped",
            GateOutcome::Blocked => "blocked",
        }
    }
}

/// One gate's result within a recorded pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateReport {
    pub name: String,
    pub outcome: GateOutcome,
}

/// A single pass through the loop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub iteration: u32,
    /// The worker's short summary of what it did this pass.
    pub summary: String,
    pub verify: VerifyOutcome,
    /// Per-gate results (empty in journals written before gates existed).
    #[serde(default)]
    pub gates: Vec<GateReport>,
}

/// The full record of a loop run, persisted between iterations for resumability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopJournal {
    pub loop_name: String,
    pub goal: String,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    /// How the run ended, or `None` while it is still in progress.
    #[serde(default)]
    pub stop: Option<StopReason>,
}

impl LoopJournal {
    pub fn new(loop_name: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            loop_name: loop_name.into(),
            goal: goal.into(),
            attempts: Vec::new(),
            stop: None,
        }
    }

    /// The number of passes recorded so far.
    pub fn iterations(&self) -> u32 {
        self.attempts.len() as u32
    }

    pub fn record(&mut self, attempt: Attempt) {
        self.attempts.push(attempt);
    }

    pub fn finish(&mut self, stop: StopReason) {
        self.stop = Some(stop);
    }

    /// True if the run finished by meeting its goal.
    pub fn succeeded(&self) -> bool {
        matches!(self.stop, Some(StopReason::Succeeded))
    }

    /// Gates the latest pass left pending (failed, or blocked behind a
    /// failure). These must run on the next pass even if nothing changed —
    /// otherwise a no-op pass could let a red gate "succeed" unchecked.
    pub fn pending_gates(&self) -> Vec<String> {
        self.attempts
            .last()
            .map(|a| {
                a.gates
                    .iter()
                    .filter(|g| g.outcome.is_pending())
                    .map(|g| g.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A compact, model-readable digest of prior attempts to feed into the next
    /// pass so the agent doesn't repeat what already failed.
    pub fn digest(&self) -> String {
        if self.attempts.is_empty() {
            return "(no previous attempts)".to_string();
        }
        let mut out = String::new();
        for a in &self.attempts {
            let verdict = match &a.verify {
                VerifyOutcome::Passed => "PASSED".to_string(),
                VerifyOutcome::Failed { detail } => {
                    format!("FAILED — {}", truncate(detail, 400))
                }
            };
            let gates = if a.gates.is_empty() {
                String::new()
            } else {
                let listing = a
                    .gates
                    .iter()
                    .map(|g| format!("{}={}", g.name, g.outcome.word()))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("\n  gates: {listing}")
            };
            out.push_str(&format!(
                "- iteration {}: {}\n  verify: {verdict}{gates}\n",
                a.iteration,
                truncate(&a.summary, 300),
            ));
        }
        out
    }
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let kept: String = s.chars().take(max).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_lists_attempts_and_verdicts() {
        let mut j = LoopJournal::new("Tidy", "tidy up");
        assert_eq!(j.digest(), "(no previous attempts)");
        j.record(Attempt {
            iteration: 1,
            summary: "ran the formatter".into(),
            verify: VerifyOutcome::Failed {
                detail: "clippy: unused import".into(),
            },
            gates: vec![
                GateReport {
                    name: "clippy".into(),
                    outcome: GateOutcome::Failed {
                        detail: "unused import".into(),
                    },
                },
                GateReport {
                    name: "tests".into(),
                    outcome: GateOutcome::Blocked,
                },
            ],
        });
        let d = j.digest();
        assert!(d.contains("iteration 1"));
        assert!(d.contains("ran the formatter"));
        assert!(d.contains("FAILED"));
        assert!(d.contains("unused import"));
        assert!(d.contains("clippy=failed tests=blocked"));
    }

    #[test]
    fn pending_gates_come_from_the_latest_attempt() {
        let mut j = LoopJournal::new("Tidy", "tidy up");
        assert!(j.pending_gates().is_empty());
        j.record(Attempt {
            iteration: 1,
            summary: "tried".into(),
            verify: VerifyOutcome::Failed {
                detail: "fmt".into(),
            },
            gates: vec![
                GateReport {
                    name: "fmt".into(),
                    outcome: GateOutcome::Failed {
                        detail: "diff".into(),
                    },
                },
                GateReport {
                    name: "tests".into(),
                    outcome: GateOutcome::Blocked,
                },
                GateReport {
                    name: "docs".into(),
                    outcome: GateOutcome::Skipped,
                },
            ],
        });
        assert_eq!(j.pending_gates(), vec!["fmt", "tests"]);
    }

    #[test]
    fn journal_round_trips_through_json() {
        let mut j = LoopJournal::new("Green", "green tests");
        j.record(Attempt {
            iteration: 1,
            summary: "fixed a test".into(),
            verify: VerifyOutcome::Passed,
            gates: vec![GateReport {
                name: "tests".into(),
                outcome: GateOutcome::Passed,
            }],
        });
        j.finish(StopReason::Succeeded);
        let raw = serde_json::to_string(&j).unwrap();
        let back: LoopJournal = serde_json::from_str(&raw).unwrap();
        assert_eq!(j, back);
        assert!(back.succeeded());
    }
}
