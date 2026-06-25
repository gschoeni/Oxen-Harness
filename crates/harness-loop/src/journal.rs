//! The loop's memory: a small record of what's been tried, what failed, and
//! how it ended — so a later pass resumes instead of repeating mistakes.

use serde::{Deserialize, Serialize};

use crate::runner::StopReason;

/// The outcome of one verify gate run.
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

/// A single pass through the loop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub iteration: u32,
    /// The worker's short summary of what it did this pass.
    pub summary: String,
    pub verify: VerifyOutcome,
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
            out.push_str(&format!(
                "- iteration {}: {}\n  verify: {verdict}\n",
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
        });
        let d = j.digest();
        assert!(d.contains("iteration 1"));
        assert!(d.contains("ran the formatter"));
        assert!(d.contains("FAILED"));
        assert!(d.contains("unused import"));
    }

    #[test]
    fn journal_round_trips_through_json() {
        let mut j = LoopJournal::new("Green", "green tests");
        j.record(Attempt {
            iteration: 1,
            summary: "fixed a test".into(),
            verify: VerifyOutcome::Passed,
        });
        j.finish(StopReason::Succeeded);
        let raw = serde_json::to_string(&j).unwrap();
        let back: LoopJournal = serde_json::from_str(&raw).unwrap();
        assert_eq!(j, back);
        assert!(back.succeeded());
    }
}
