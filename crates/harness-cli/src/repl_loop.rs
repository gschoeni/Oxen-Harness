//! The interactive REPL drivers: read one line (via the classic readline or
//! the live bottom-pinned box), dispatch it through [`handle_line`], repeat.
//!
//! Command *parsing* lives in [`crate::repl`]; turn *execution* in
//! [`crate::turn`]. This module is the loop that connects them: prompt
//! history, the stacked-queue reminder, and the `/command` dispatch table.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use harness_agent::Agent;
use harness_store::HistoryStore;
use rustyline::DefaultEditor;

use crate::queue::MessageQueue;
use crate::repl::{parse_command, Command};
use crate::theme::{self, Ui};
use crate::turn::{ends_mid_turn, run_turn_and_drain, TurnRequest};
use crate::{
    auth_cmd, code_review_cmd, compression_cmd, live, loop_cmd, model_cmd, queue_cmd, theme_cmd,
};

/// Immutable, session-scoped context for a REPL run: the values set once at
/// startup and threaded through the line handler unchanged. Bundling them keeps
/// [`handle_line`] to the state that actually varies (the agent, UI, and queue).
pub(crate) struct ReplContext<'a> {
    pub(crate) store: &'a Arc<HistoryStore>,
    pub(crate) session: &'a str,
    pub(crate) workspace_root: &'a Path,
    pub(crate) base_url: &'a str,
}

/// The classic readline REPL for pipes, dumb terminals, and
/// `OXEN_HARNESS_CLASSIC_INPUT`: a blocking line prompt with persistent,
/// cross-session Up-arrow history, dispatching each line through [`handle_line`].
/// The live-terminal counterpart is [`run_box_repl`].
pub(crate) async fn run_classic_repl(
    agent: &mut Agent,
    ui: &mut Ui,
    ctx: &ReplContext<'_>,
) -> Result<()> {
    // A persistent prompt history: Up-arrow recalls prompts from earlier
    // sessions, not just this one. Load whatever's on disk, then append each new
    // prompt so the history survives across runs (and a crash mid-session).
    let history_config = rustyline::Config::builder()
        .max_history_size(1000)
        .map_err(|e| anyhow::anyhow!(e))?
        .build();
    let mut editor: DefaultEditor = rustyline::Editor::with_config(history_config)?;
    let history_path = prompt_history_path();
    if let Some(path) = &history_path {
        // Missing file on first run is expected — ignore it.
        let _ = editor.load_history(path);
    }
    let mut queue = MessageQueue::default();
    // A half-typed message left in the live composer when a turn ends; it seeds
    // the next idle prompt so typing isn't wiped when the agent finishes.
    let mut carryover = String::new();
    loop {
        // Remind the user about stacked messages waiting to be sent.
        if !queue.is_empty() {
            println!(
                "  {} {}",
                ui.brown(&format!("⛺ {} stacked in the wagon", queue.len())),
                ui.dim("— /queue to review, /queue run to send"),
            );
        }
        // Recomputed each turn so a mid-session theme switch takes effect.
        let prompt = theme::prompt(ui);
        // A faint rule + blank line set the input apart from the output above.
        // Only on an interactive terminal — piped/scripted runs stay clean.
        if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            println!("\n{}", theme::divider(ui));
        }
        // Seed the line with any draft carried over from the live composer, so a
        // message typed while the agent was finishing continues uninterrupted.
        let seed = std::mem::take(&mut carryover);
        let read = if seed.is_empty() {
            editor.readline(&prompt)
        } else {
            editor.readline_with_initial(&prompt, (&seed, ""))
        };
        match read {
            Ok(line) => {
                let mut new_history = editor.add_history_entry(line.as_str()).unwrap_or(false);
                let exit = handle_line(&line, agent, ui, &mut queue, &mut carryover, ctx).await?;
                // Fold any prompts queued during the turn — typed into the live
                // composer or added via `/queue add` — into the history too, so
                // queued prompts are recallable next session, not just ones typed
                // at the idle prompt.
                for prompt in queue.take_authored() {
                    if editor.add_history_entry(prompt.as_str()).unwrap_or(false) {
                        new_history = true;
                    }
                }
                // Persist once per turn so prompts survive even an abrupt exit.
                if new_history {
                    if let Some(path) = &history_path {
                        let _ = editor.append_history(path);
                    }
                }
                if exit {
                    break;
                }
            }
            // Ctrl-C / Ctrl-D: leave cleanly.
            Err(rustyline::error::ReadlineError::Interrupted)
            | Err(rustyline::error::ReadlineError::Eof) => {
                print!("{}", theme::death_screen(ui, ctx.session));
                break;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// The interactive REPL for a live terminal: a bordered, bottom-pinned box
/// composer (multi-line, history, queue) for both idle input and in-turn
/// queueing. Reads one submission at idle via [`live::read_idle`], then dispatches
/// it (a `/command` or a prompt) through [`handle_line`] in cooked mode.
pub(crate) async fn run_box_repl(
    agent: &mut Agent,
    ui: &mut Ui,
    ctx: &ReplContext<'_>,
) -> Result<()> {
    let history_path = prompt_history_path();
    let mut history = load_prompt_history(history_path.as_deref());
    let mut queue = MessageQueue::default();
    // A half-typed message left in the live composer when a turn ends, seeding
    // the next idle box so typing continues uninterrupted.
    let mut seed = String::new();
    loop {
        if !queue.is_empty() {
            println!(
                "  {} {}",
                ui.brown(&format!("⛺ {} stacked in the wagon", queue.len())),
                ui.dim("— /queue to review, /queue run to send"),
            );
        }
        let status = Some(crate::turn::context_usage_line(agent, ui));
        let compression = compression_cmd::status_line(agent, ui);
        let idle =
            live::read_idle(ui, &mut queue, &mut history, &seed, status, compression).await?;
        match idle {
            live::Idle::Exit => {
                print!("{}", theme::death_screen(ui, ctx.session));
                break;
            }
            live::Idle::Submit(line) => {
                let mut carryover = String::new();
                let exit = handle_line(&line, agent, ui, &mut queue, &mut carryover, ctx).await?;
                seed = carryover;
                // Fold any prompts queued during the turn into the recallable
                // history too, then persist (survives an abrupt exit).
                for authored in queue.take_authored() {
                    if history.last() != Some(&authored) {
                        history.push(authored);
                    }
                }
                save_prompt_history(history_path.as_deref(), &history);
                if exit {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Handle one line of input. Returns `Ok(true)` when the REPL should exit.
async fn handle_line(
    line: &str,
    agent: &mut Agent,
    ui: &mut Ui,
    queue: &mut MessageQueue,
    carryover: &mut String,
    ctx: &ReplContext<'_>,
) -> Result<bool> {
    match parse_command(line) {
        Command::Empty => {}
        Command::Exit => {
            print!("{}", theme::death_screen(ui, ctx.session));
            return Ok(true);
        }
        Command::Help => print!("{}", theme::help(ui)),
        Command::Theme(args) => theme_cmd::handle_repl(args, agent, ui).await?,
        Command::Queue(rest) => {
            // `/queue run` may stream turns that the user can Ctrl-C to quit.
            if queue_cmd::handle_repl(rest, queue, agent, ui, carryover).await? {
                print!("{}", theme::death_screen(ui, ctx.session));
                return Ok(true);
            }
        }
        Command::Loop(rest) => {
            // A running loop streams turns the user can Ctrl-C to quit.
            if loop_cmd::handle_repl(rest, agent, ui, ctx.workspace_root).await? {
                print!("{}", theme::death_screen(ui, ctx.session));
                return Ok(true);
            }
        }
        Command::CodeReview(rest) => {
            // A running review streams turns the user can Ctrl-C to quit.
            if code_review_cmd::handle_repl(rest, agent, ui, ctx.workspace_root).await? {
                print!("{}", theme::death_screen(ui, ctx.session));
                return Ok(true);
            }
        }
        Command::Departing(None) => match ui.departing() {
            Some((label, value)) => {
                println!("  {} {}", ui.brown(&format!("{label}:")), ui.cream(value))
            }
            None => println!("  {}", ui.dim("no departing location set")),
        },
        Command::Departing(Some(place)) => {
            let label = ui.set_departing(&place);
            // Reprint the whole welcome banner so the new row shows in context.
            print!(
                "{}",
                theme::banner(
                    ui,
                    ctx.base_url,
                    agent.model(),
                    &ctx.workspace_root.display().to_string(),
                    ctx.session,
                    agent.tokens_used(),
                )
            );
            println!();
            println!(
                "  {} {}",
                ui.green(&format!("⛺ {label} set:")),
                ui.cream(&place),
            );
        }
        Command::Skills => print_skills(ui, ctx.workspace_root),
        Command::Auth(rest) => auth_cmd::handle_repl(rest, agent, ui, ctx.base_url)?,
        Command::Compression(rest) => compression_cmd::handle_repl(rest, agent, ui)?,
        Command::Model(rest) => model_cmd::handle_repl(rest, agent, ui)?,
        Command::Export(dest) => export(ctx.store, ctx.session, dest, ui)?,
        Command::Retry => {
            // Only a transcript that stops mid-turn has anything to re-drive;
            // retrying a settled conversation would confuse the model (and
            // the user).
            if !ends_mid_turn(agent.messages()) {
                println!(
                    "  {} {}",
                    ui.dim("nothing to retry —"),
                    ui.dim("the last turn finished (send a new message instead)"),
                );
                return Ok(false);
            }
            if run_turn_and_drain(agent, TurnRequest::Continue, ui, queue, carryover).await? {
                print!("{}", theme::death_screen(ui, ctx.session));
                return Ok(true);
            }
        }
        Command::Prompt(prompt) => {
            // Ctrl-C mid-stream ends the expedition just like quitting does.
            if run_turn_and_drain(agent, TurnRequest::Prompt(prompt), ui, queue, carryover).await? {
                print!("{}", theme::death_screen(ui, ctx.session));
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Print the skills discovered for this workspace (global + project, with the
/// user's enable/disable preferences), and where to add more. Skills apply when
/// a session starts, so a mid-session edit shows here but reaches the agent on
/// the next run.
fn print_skills(ui: &Ui, workspace_root: &Path) {
    let skills = harness_runtime::skills::list(workspace_root, &harness_runtime::skills::load());
    if skills.is_empty() {
        println!(
            "  {}",
            ui.dim("no skills yet — teach the agent a workflow:")
        );
        println!(
            "  {}",
            ui.dim("drop a SKILL.md folder in ~/.oxen-harness/skills/ (or .oxen-harness/skills/")
        );
        println!(
            "  {}",
            ui.dim("in this repo). See \"Adding a skill\" in the README.")
        );
        return;
    }
    println!("  {}", ui.brown("know-how on hand:"));
    for s in &skills {
        let scope = match s.scope {
            harness_tools::SkillScope::Global => "global",
            harness_tools::SkillScope::Project => "project",
        };
        let status = if s.enabled { "" } else { "  (off)" };
        println!(
            "    {} {}{}",
            ui.accent(&format!("{:<20}", s.name)),
            ui.dim(&format!("[{scope}]")),
            ui.red(status),
        );
        println!("      {}", ui.cream(&s.description));
    }
    println!(
        "  {}",
        ui.dim("the model loads a skill's instructions on demand; new skills apply to new runs")
    );
}

/// `/export [path]` — save the session transcript as JSONL, or report its size.
fn export(store: &Arc<HistoryStore>, session: &str, dest: Option<String>, ui: &Ui) -> Result<()> {
    let jsonl = store.export_jsonl(session)?;
    let count = jsonl.lines().count();
    match dest {
        Some(path) => {
            std::fs::write(&path, &jsonl).with_context(|| format!("writing {path}"))?;
            println!(
                "  {} {}",
                ui.green("🏆 Oregon Top Ten saved:"),
                ui.cream(&format!("{count} entries → {path}")),
            );
        }
        None => println!(
            "  {} {}",
            ui.brown("This journey so far:"),
            ui.dim(&format!(
                "{count} journal entries (pass a path to save JSONL)"
            )),
        ),
    }
    Ok(())
}

/// File backing the readline prompt history, so Up-arrow recalls prompts typed
/// in previous CLI sessions (separate from the SQLite chat transcript store).
fn prompt_history_path() -> Option<std::path::PathBuf> {
    harness_config::paths::prompt_history_file().ok()
}

/// Load the flat prompt-history file (one entry per line) for box-mode recall.
fn load_prompt_history(path: Option<&Path>) -> Vec<String> {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| {
            s.lines()
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Persist the prompt history, flattening newlines so the one-line-per-entry
/// file stays valid (a recalled multi-line entry returns single-line next run).
fn save_prompt_history(path: Option<&Path>, entries: &[String]) {
    if let Some(p) = path {
        let body = entries
            .iter()
            .map(|e| e.replace('\n', " "))
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(p, body);
    }
}
