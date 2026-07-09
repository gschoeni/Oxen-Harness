//! `/code-review` — run the configurable review pipeline against the working
//! diff (or PR-style against a base branch), stream each step, then print the
//! findings and inject them into the conversation so a follow-up "fix 1 and 3"
//! has them in context.
//!
//! The pipeline itself (steps, prompts, diff resolution) lives in
//! `harness-review`; this module is the CLI front end: argument grammar,
//! progress rendering, and the findings printout.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use harness_agent::Agent;
use harness_review::{
    session_exchange, ReviewConfig, ReviewError, ReviewEvent, ReviewReport, ReviewRunner,
    ReviewTarget,
};
use tokio_util::sync::CancellationToken;

use crate::fleet_ui::{BlockPainter, FleetHub, FleetState};
use crate::render::TurnRenderer;
use crate::theme::Ui;
use crate::turn::human_tokens;

/// Handle an in-REPL `/code-review ...` command. Returns `Ok(true)` if a
/// running review was interrupted (Ctrl-C), so the REPL should end the session.
pub async fn handle_repl(
    rest: Option<String>,
    agent: &mut Agent,
    ui: &Ui,
    workspace_root: &Path,
) -> Result<bool> {
    let rest = rest.unwrap_or_default();
    let arg = rest.trim();
    let target = match arg {
        "" => ReviewTarget::Uncommitted,
        "steps" | "config" => {
            print_steps(ui);
            return Ok(false);
        }
        "help" | "-h" | "--help" => {
            print_usage(ui);
            return Ok(false);
        }
        branch => {
            if branch.contains(char::is_whitespace) || !ref_exists(workspace_root, branch) {
                println!(
                    "  {} {}",
                    ui.red("✗"),
                    ui.dim(&format!(
                        "`{branch}` is not a branch or ref in this repository"
                    )),
                );
                print_usage(ui);
                return Ok(false);
            }
            ReviewTarget::BaseBranch(branch.to_string())
        }
    };
    run(target, agent, ui, workspace_root).await
}

/// Run the pipeline to completion, streaming progress. Returns `Ok(true)` if
/// the user interrupted with Ctrl-C.
async fn run(
    target: ReviewTarget,
    agent: &mut Agent,
    ui: &Ui,
    workspace_root: &Path,
) -> Result<bool> {
    let config = ReviewConfig::load();
    let steps: Vec<String> = config
        .resolved_steps()
        .iter()
        .map(|s| {
            let lanes = s.resolved_agents().len();
            if lanes > 1 {
                format!("{}×{lanes}", s.name)
            } else {
                s.name.clone()
            }
        })
        .collect();
    println!();
    println!(
        "  {} {}",
        ui.title("🔍 Code review:"),
        ui.cream(&target.label())
    );
    println!(
        "    {} {}",
        ui.brown("pipeline:"),
        ui.dim(&format!(
            "{} (up to {} findings, {} lanes at once)",
            steps.join(" → "),
            config.max_findings,
            config.max_parallel,
        )),
    );

    // One stop signal for the whole review, with the same blast radius no
    // matter which step is running: a Ctrl-C during a cooked single-agent step
    // trips the signal arm below, which *cancels the review and keeps the
    // session* — exactly what the fan-out painter does when it reads Ctrl-C as
    // a key in raw mode. (This arm used to return Ok(true) and end the whole
    // REPL, so the identical keystroke behaved differently depending on the
    // step.) After signalling we keep awaiting the runner so it stops
    // cooperatively and still reports its spend, rather than dropping the
    // future mid-stream.
    let cancel = CancellationToken::new();
    let runner =
        ReviewRunner::new(config, target.clone(), workspace_root).with_cancel(cancel.clone());
    let display = Rc::new(RefCell::new(ReviewDisplay::new(ui.clone(), cancel.clone())));
    let d = display.clone();
    let result = {
        let run = runner.run(agent, |ev| d.borrow_mut().on_event(ev));
        tokio::pin!(run);
        loop {
            tokio::select! {
                res = &mut run => break res,
                _ = tokio::signal::ctrl_c(), if !cancel.is_cancelled() => cancel.cancel(),
            }
        }
    };
    display.borrow_mut().finish();
    let tokens_used = display.borrow().tokens_used;

    match result {
        Ok(report) => {
            print_report(ui, &report);
            // Land the report in the session so follow-up turns can act on it
            // ("fix 1 and 3") without re-running anything.
            let (user, assistant) = session_exchange(&target, &report);
            agent.inject_exchange(user, assistant)?;
            if tokens_used > 0 {
                println!(
                    "  {}",
                    ui.dim(&format!(
                        "~{} tokens across the pipeline's reviewers",
                        human_tokens(tokens_used)
                    )),
                );
            }
            if !report.findings.is_empty() {
                println!(
                    "  {}",
                    ui.dim("the findings are in the conversation — ask to fix by number, e.g. `fix 1 and 2`"),
                );
            }
            Ok(false)
        }
        Err(ReviewError::NothingToReview) => {
            println!(
                "  {} {}",
                ui.green("✓"),
                ui.dim("nothing to review — the target has no changes"),
            );
            Ok(false)
        }
        Err(ReviewError::Cancelled { tokens_used }) => {
            println!(
                "  {} {}",
                ui.brown("⛺"),
                ui.dim("review stopped — nothing was recorded"),
            );
            if tokens_used > 0 {
                println!(
                    "  {}",
                    ui.dim(&format!(
                        "~{} tokens spent by the reviewers before stopping",
                        human_tokens(tokens_used)
                    )),
                );
            }
            Ok(false)
        }
        Err(e) => {
            println!(
                "  {} {}",
                ui.red("✗"),
                ui.dim(&format!("review failed: {e}"))
            );
            Ok(false)
        }
    }
}

/// Owns the two rendering modes a review flips between: the single-stream
/// [`TurnRenderer`] for one-agent steps, and the multi-lane fleet block (a
/// [`FleetHub`] + [`BlockPainter`], with 1-9/esc lane switching) for fan-out
/// steps. Exactly one is live at a time. On non-animating terminals the
/// fan-out degrades to milestone lines.
struct ReviewDisplay {
    ui: Ui,
    turn: TurnRenderer,
    hub: Arc<FleetHub>,
    painter: Option<BlockPainter>,
    /// Milestone-print mode (pipes, `NO_COLOR`): no painter thread.
    plain: bool,
    /// The review-wide stop signal, handed to each fan-out block so the
    /// painter's Ctrl-C key can fire it.
    cancel: CancellationToken,
    tokens_used: usize,
}

impl ReviewDisplay {
    fn new(ui: Ui, cancel: CancellationToken) -> Self {
        Self {
            turn: TurnRenderer::new(ui.clone()),
            plain: !ui.animates(),
            ui,
            hub: Arc::new(FleetHub::default()),
            painter: None,
            cancel,
            tokens_used: 0,
        }
    }

    fn on_event(&mut self, ev: &ReviewEvent) {
        match ev {
            ReviewEvent::Started { .. } => {}
            ReviewEvent::StepStarted {
                index,
                total,
                name,
                agents,
            } => {
                self.settle();
                println!();
                if agents.len() > 1 {
                    println!(
                        "  {}",
                        self.ui.title(&format!(
                            "─── Step {}/{total}: {name} — {} reviewers in parallel ───",
                            index + 1,
                            agents.len(),
                        )),
                    );
                    self.hub
                        .install(FleetState::new(agents, Some(self.cancel.clone())));
                    if !self.plain {
                        self.painter = Some(BlockPainter::start(&self.ui, self.hub.clone()));
                    }
                } else {
                    println!(
                        "  {}",
                        self.ui
                            .title(&format!("─── Step {}/{total}: {name} ───", index + 1)),
                    );
                    self.turn.begin_thinking();
                }
            }
            ReviewEvent::Agent(e) => self.turn.on_event(e),
            // A fan-out step's lanes are a fleet — route through the same hub
            // logic the spawn_agents sink uses, so review lanes and turn lanes
            // behave identically.
            ReviewEvent::Fleet(event) => {
                crate::fleet_ui::apply_fleet_event(&self.hub, &self.ui, self.plain, event)
            }
            ReviewEvent::StepCompleted { .. } => self.settle(),
            ReviewEvent::Completed { tokens_used, .. } => self.tokens_used = *tokens_used,
        }
    }

    /// Close whichever renderer is live, leaving its final output on screen.
    fn settle(&mut self) {
        self.turn.finish();
        if let Some(painter) = self.painter.take() {
            painter.finish();
        }
        self.hub.clear();
    }

    fn finish(&mut self) {
        self.settle();
    }
}

/// Print the findings, most-severe first, numbered to match the injected
/// report (so "fix 2" is unambiguous).
fn print_report(ui: &Ui, report: &ReviewReport) {
    println!();
    if !report.parsed {
        println!(
            "  {} {}",
            ui.brown("⚠"),
            ui.dim("the report step returned unstructured output:"),
        );
        for line in report.raw.trim().lines() {
            println!("    {}", ui.cream(line));
        }
        return;
    }
    if report.findings.is_empty() {
        println!(
            "  {} {}",
            ui.green("🏆 No findings —"),
            ui.green("nothing qualifying survived verification"),
        );
    } else {
        println!(
            "  {} {}",
            ui.title(&format!("⚠ {} finding(s)", report.findings.len())),
            ui.dim("(most severe first)"),
        );
        for (i, f) in report.findings.iter().enumerate() {
            let priority = f.priority.map(|p| format!("[P{p}] ")).unwrap_or_default();
            let verdict = f
                .verdict
                .as_deref()
                .map(|v| format!("  {v}"))
                .unwrap_or_default();
            println!();
            println!(
                "  {} {}{}{}",
                ui.accent(&format!("{}.", i + 1)),
                ui.cream(&format!("{priority}{}", f.title)),
                ui.brown(&format!("  {}", f.location())),
                ui.dim(&verdict),
            );
            if !f.body.is_empty() {
                println!("     {}", ui.dim(&f.body));
            }
            if !f.failure_scenario.is_empty() {
                println!(
                    "     {}",
                    ui.dim(&format!("scenario: {}", f.failure_scenario))
                );
            }
        }
    }
    if let Some(correctness) = &report.overall_correctness {
        println!();
        let line = format!(
            "overall: the change looks {correctness}. {}",
            report.overall_explanation.as_deref().unwrap_or_default()
        );
        if correctness == "correct" {
            println!("  {}", ui.green(&line));
        } else {
            println!("  {}", ui.brown(&line));
        }
    }
    println!();
}

/// Print the configured pipeline and where to edit it.
fn print_steps(ui: &Ui) {
    let config = ReviewConfig::load();
    println!();
    println!("  {}", ui.title("Code-review pipeline"));
    for (i, step) in config.resolved_steps().iter().enumerate() {
        let agents = step.resolved_agents();
        // A fan-out step keeps its prompts in `agents` (its own `prompt` is
        // empty), so summarize each lane; a single-agent step shows its one
        // prompt's first line.
        let summary = if step.is_fan_out() {
            format!(
                "{} reviewers in parallel: {}",
                agents.len(),
                agents
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        } else {
            agents
                .first()
                .and_then(|a| a.prompt.lines().next())
                .unwrap_or_default()
                .to_string()
        };
        println!(
            "  {} {}",
            ui.accent(&format!("{}. {:<8}", i + 1, step.name)),
            ui.dim(&crate::render::truncate(&summary, 80)),
        );
    }
    println!(
        "  {} {}",
        ui.brown("max findings:"),
        ui.cream(&config.max_findings.to_string()),
    );
    if let Ok(path) = harness_config::paths::code_review_file() {
        println!(
            "  {}",
            ui.dim(&format!(
                "edit the step prompts in {} (or the desktop app's Settings → Code Review)",
                path.display()
            )),
        );
    }
}

fn print_usage(ui: &Ui) {
    println!(
        "  {}",
        ui.dim("code-review: /code-review (uncommitted changes) | /code-review <base-branch> (PR-style) | /code-review steps"),
    );
}

/// Whether `name` resolves to a commit-ish in this repo (branch, tag, sha).
fn ref_exists(root: &Path, name: &str) -> bool {
    std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{name}^{{commit}}"),
        ])
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_exists_accepts_real_refs_and_rejects_junk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .ok()
                .filter(|o| o.status.success())
        };
        if run(&["init", "-q", "-b", "main"]).is_none() {
            return; // no git in this environment
        }
        run(&["config", "user.email", "t@t"]).unwrap();
        run(&["config", "user.name", "t"]).unwrap();
        std::fs::write(root.join("a"), "x").unwrap();
        run(&["add", "."]).unwrap();
        run(&["commit", "-q", "-m", "init"]).unwrap();

        assert!(ref_exists(root, "main"));
        assert!(ref_exists(root, "HEAD"));
        assert!(!ref_exists(root, "no-such-branch"));
    }
}
