//! The pipeline engine: walk the configured steps in order, each on a fresh,
//! isolated side agent, threading step N's reply into step N+1's prompt.
//!
//! Isolation is the point. The verify step must judge the finder's candidates
//! from the *code*, not from the finder's reasoning — sharing a context would
//! anchor it. Each step therefore runs on [`Agent::side_agent`]: same model,
//! tools, and config, but an in-memory transcript that vanishes when the step
//! ends. Only the final report survives, and the host injects it into the real
//! session (via [`Agent::inject_exchange`]) so a follow-up "fix 1 and 3" has
//! the findings in context.

use std::path::{Path, PathBuf};

use harness_agent::{Agent, AgentEvent};
use tokio_util::sync::CancellationToken;

use crate::config::ReviewConfig;
use crate::findings::ReviewReport;
use crate::target::{resolve_target, ReviewInput, ReviewTarget};
use crate::ReviewError;

/// Progress surfaced to a host (CLI/UI) as a review runs.
#[derive(Debug, Clone)]
pub enum ReviewEvent {
    /// The pipeline began: what it reviews and the step names, in order.
    Started { target: String, steps: Vec<String> },
    /// A step began (index is 0-based; total is the step count).
    StepStarted {
        index: usize,
        total: usize,
        name: String,
    },
    /// A streaming/tool event forwarded from the step's agent.
    Agent(AgentEvent),
    /// A step finished.
    StepCompleted { index: usize, name: String },
    /// The pipeline finished; the report is also returned from `run`.
    Completed { findings: usize, parsed: bool },
}

/// Drives a [`ReviewConfig`] against a workspace.
pub struct ReviewRunner {
    config: ReviewConfig,
    target: ReviewTarget,
    workspace_root: PathBuf,
    cancel: CancellationToken,
}

impl ReviewRunner {
    pub fn new(config: ReviewConfig, target: ReviewTarget, workspace_root: &Path) -> Self {
        Self {
            config,
            target,
            workspace_root: workspace_root.to_path_buf(),
            cancel: CancellationToken::new(),
        }
    }

    /// Install a stop signal: cancelling it ends the in-flight step's stream
    /// and the pipeline returns [`ReviewError::Cancelled`] instead of a report.
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Run every step in order and parse the last step's reply as the report.
    ///
    /// Takes the session agent only to spawn detached side agents from it —
    /// the session's transcript is never touched here.
    pub async fn run<F>(&self, agent: &Agent, mut on_event: F) -> Result<ReviewReport, ReviewError>
    where
        F: FnMut(&ReviewEvent),
    {
        let input = resolve_target(&self.workspace_root, self.target.clone())?;
        let steps = self.config.resolved_steps();
        on_event(&ReviewEvent::Started {
            target: input.target.label(),
            steps: steps.iter().map(|s| s.name.clone()).collect(),
        });

        let mut previous = String::new();
        for (index, step) in steps.iter().enumerate() {
            on_event(&ReviewEvent::StepStarted {
                index,
                total: steps.len(),
                name: step.name.clone(),
            });
            let prompt = render_prompt(
                &step.prompt,
                &input,
                &previous,
                self.config.max_findings,
                index,
            );
            let mut side = agent.side_agent()?;
            side.set_cancel_token(self.cancel.clone());
            previous = side
                .run_turn(prompt, |e| on_event(&ReviewEvent::Agent(e.clone())))
                .await?;
            // A cancelled step returns its partial text; don't run the rest of
            // the pipeline on it.
            if self.cancel.is_cancelled() {
                return Err(ReviewError::Cancelled);
            }
            on_event(&ReviewEvent::StepCompleted {
                index,
                name: step.name.clone(),
            });
        }

        let report = ReviewReport::parse(&previous);
        on_event(&ReviewEvent::Completed {
            findings: report.findings.len(),
            parsed: report.parsed,
        });
        Ok(report)
    }
}

/// Fill a step template's placeholders. Unknown placeholders pass through
/// untouched (a typo shows up in the prompt rather than silently vanishing).
fn render_prompt(
    template: &str,
    input: &ReviewInput,
    previous: &str,
    max_findings: usize,
    step_index: usize,
) -> String {
    let previous = if step_index == 0 && previous.is_empty() {
        "(this is the first step — there is no previous output)"
    } else {
        previous
    };
    template
        .replace("{{target}}", &input.description)
        .replace("{{diff}}", &input.diff)
        .replace("{{previous}}", previous)
        .replace("{{max_findings}}", &max_findings.to_string())
}

/// The synthetic exchange a host injects into the real session after a review,
/// so follow-up turns ("fix 1 and 3") have the findings in context.
pub fn session_exchange(target: &ReviewTarget, report: &ReviewReport) -> (String, String) {
    let user = format!(
        "Run a code review of the {} in this workspace.",
        target.label()
    );
    let mut assistant = report.to_markdown();
    if !report.findings.is_empty() {
        assistant.push_str(
            "\n\nTell me which findings to fix (e.g. \"fix 1 and 3\") and I'll apply them.",
        );
    }
    (user, assistant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::Finding;

    fn input() -> ReviewInput {
        ReviewInput {
            target: ReviewTarget::Uncommitted,
            description: "uncommitted changes".into(),
            diff: "+ let x = 1;".into(),
        }
    }

    #[test]
    fn placeholders_fill_and_first_step_gets_a_previous_note() {
        let out = render_prompt(
            "T:{{target}} D:{{diff}} P:{{previous}} N:{{max_findings}}",
            &input(),
            "",
            7,
            0,
        );
        assert_eq!(
            out,
            "T:uncommitted changes D:+ let x = 1; P:(this is the first step — there is no previous output) N:7"
        );

        let later = render_prompt("P:{{previous}}", &input(), "candidates…", 7, 1);
        assert_eq!(later, "P:candidates…");
    }

    #[test]
    fn unknown_placeholders_pass_through() {
        let out = render_prompt("{{tarjet}}", &input(), "", 5, 0);
        assert_eq!(out, "{{tarjet}}");
    }

    #[test]
    fn session_exchange_carries_findings_and_a_fix_hint() {
        let report = ReviewReport {
            findings: vec![Finding {
                title: "Fix the pager".into(),
                file: "src/pager.rs".into(),
                line: Some(42),
                ..Default::default()
            }],
            parsed: true,
            ..Default::default()
        };
        let (user, assistant) = session_exchange(&ReviewTarget::BaseBranch("main".into()), &report);
        assert!(user.contains("changes against `main`"));
        assert!(assistant.contains("src/pager.rs:42"));
        assert!(assistant.contains("fix 1 and 3"));

        // A clean review doesn't ask the user to fix anything.
        let (_, clean) = session_exchange(&ReviewTarget::Uncommitted, &ReviewReport::parse("{}"));
        assert!(!clean.contains("fix 1 and 3"));
    }
}
