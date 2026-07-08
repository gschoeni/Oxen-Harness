//! The pipeline engine: walk the configured steps in order, threading each
//! step's reply into the next step's prompt. A single-agent step runs on one
//! fresh, isolated side agent; a fan-out step runs its agents as a parallel
//! [`harness_agent::fleet`], capped by `max_parallel`, and concatenates
//! their outputs (labeled, in agent order) as the step's reply.
//!
//! Isolation is the point. The verify step must judge the finders' candidates
//! from the *code*, not from the finders' reasoning — sharing a context would
//! anchor it. Every reviewer therefore runs on [`Agent::side_agent`]: same
//! model, tools, and config, but an in-memory transcript that vanishes when
//! the step ends. Only the final report survives, and the host injects it into
//! the real session (via [`Agent::inject_exchange`]) so a follow-up "fix 1 and
//! 3" has the findings in context.

use std::path::{Path, PathBuf};

use harness_agent::fleet::{self, FleetEvent, SubagentTask};
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
    /// A step began (index is 0-based; total is the step count). `agents` is
    /// the lane labels, in order — more than one means the step fans out.
    StepStarted {
        index: usize,
        total: usize,
        name: String,
        agents: Vec<String>,
    },
    /// A streaming/tool event from a single-agent step's reviewer.
    Agent(AgentEvent),
    /// A fan-out lane acquired a slot and its turn is running.
    SubagentStarted { agent: usize, name: String },
    /// A streaming/tool event from one fan-out lane, tagged by lane index.
    Subagent { agent: usize, event: AgentEvent },
    /// A fan-out lane finished (`summary` is its truncated reply or error).
    SubagentCompleted {
        agent: usize,
        name: String,
        ok: bool,
        tokens_used: usize,
        summary: String,
    },
    /// A step finished.
    StepCompleted { index: usize, name: String },
    /// The pipeline finished; the report is also returned from `run`.
    /// `tokens_used` is the estimated total across every reviewer agent.
    Completed {
        findings: usize,
        parsed: bool,
        tokens_used: usize,
    },
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
        let mut tokens_used = 0usize;
        for (index, step) in steps.iter().enumerate() {
            let agents = step.resolved_agents();
            on_event(&ReviewEvent::StepStarted {
                index,
                total: steps.len(),
                name: step.name.clone(),
                agents: agents.iter().map(|a| a.name.clone()).collect(),
            });

            let (text, step_tokens) = if agents.len() == 1 {
                self.run_single(agent, &agents[0].prompt, &input, &previous, index, |e| {
                    on_event(e)
                })
                .await?
            } else {
                self.run_fan_out(agent, &agents, &input, &previous, index, |e| on_event(e))
                    .await?
            };
            previous = text;
            tokens_used += step_tokens;

            // A cancelled step returns partial text; don't run the rest of the
            // pipeline on it.
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
            tokens_used,
        });
        Ok(report)
    }

    /// One reviewer on one side agent, streaming its events unwrapped.
    /// Returns the reply and the reviewer's token cost.
    async fn run_single<F>(
        &self,
        agent: &Agent,
        template: &str,
        input: &ReviewInput,
        previous: &str,
        step_index: usize,
        mut on_event: F,
    ) -> Result<(String, usize), ReviewError>
    where
        F: FnMut(&ReviewEvent),
    {
        let prompt = render_prompt(
            template,
            input,
            previous,
            self.config.max_findings,
            step_index,
        );
        let mut side = agent.side_agent()?;
        side.set_cancel_token(self.cancel.clone());
        let text = side
            .run_turn(prompt, |e| on_event(&ReviewEvent::Agent(e.clone())))
            .await?;
        Ok((text, side.tokens_used()))
    }

    /// A fan-out step: every reviewer as a parallel fleet task, outputs
    /// concatenated under `###` headings in agent order. One lane failing is
    /// reported inline (the verifier reads what survived); every lane failing
    /// fails the step with the first error. Returns the combined reply and the
    /// lanes' summed token cost.
    async fn run_fan_out<F>(
        &self,
        agent: &Agent,
        agents: &[crate::config::StepAgent],
        input: &ReviewInput,
        previous: &str,
        step_index: usize,
        mut on_event: F,
    ) -> Result<(String, usize), ReviewError>
    where
        F: FnMut(&ReviewEvent),
    {
        let tasks: Vec<SubagentTask> = agents
            .iter()
            .map(|a| {
                SubagentTask::new(
                    a.name.clone(),
                    render_prompt(
                        &a.prompt,
                        input,
                        previous,
                        self.config.max_findings,
                        step_index,
                    ),
                )
            })
            .collect();

        let outcomes = fleet::run_fleet(
            || agent.side_agent(),
            tasks,
            self.config.max_parallel,
            self.cancel.clone(),
            |event| match event {
                FleetEvent::TaskStarted { index, label } => {
                    on_event(&ReviewEvent::SubagentStarted {
                        agent: *index,
                        name: label.clone(),
                    })
                }
                FleetEvent::Agent { index, event } => on_event(&ReviewEvent::Subagent {
                    agent: *index,
                    event: event.clone(),
                }),
                FleetEvent::TaskCompleted {
                    index,
                    label,
                    ok,
                    tokens_used,
                    summary,
                } => on_event(&ReviewEvent::SubagentCompleted {
                    agent: *index,
                    name: label.clone(),
                    ok: *ok,
                    tokens_used: *tokens_used,
                    summary: summary.clone(),
                }),
            },
        )
        .await?;

        let tokens: usize = outcomes.iter().map(|o| o.tokens_used).sum();
        if outcomes.iter().all(|o| !o.ok()) {
            // Nothing survived; surface the first error rather than feeding the
            // next step an all-failure report.
            let first = outcomes
                .into_iter()
                .find_map(|o| o.result.err())
                .expect("all-failed fleet has an error");
            return Err(first.into());
        }

        let mut combined = String::new();
        for outcome in outcomes {
            combined.push_str(&format!("### {}\n\n", outcome.label));
            match outcome.result {
                Ok(text) => combined.push_str(text.trim()),
                Err(e) => combined.push_str(&format!("(this reviewer failed: {e})")),
            }
            combined.push_str("\n\n");
        }
        Ok((combined.trim_end().to_string(), tokens))
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
