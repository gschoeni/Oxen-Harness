//! Loops for the CLI: the `oxen-harness loop` subcommand and the in-REPL
//! `/loop` command. A loop hands the agent a job, a gate that decides when it's
//! done, and a stop rule — then drives DISCOVER → QUESTION → PLAN → EXECUTE →
//! VERIFY → ITERATE until the gate is green or it gives up.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use harness_agent::Agent;
use harness_loop::{
    Gate, LoopEvent, LoopJournal, LoopRunner, LoopSpec, LoopStore, RunWhen, Verify,
};
use rustyline::DefaultEditor;

use crate::picker::{self, Choice};
use crate::render::{truncate, TurnRenderer};
use crate::theme::Ui;

/// `oxen-harness loop <action>`.
#[derive(Debug, clap::Subcommand)]
pub enum LoopAction {
    /// List available loops (built-in + saved).
    List,
    /// Show a loop's definition and its last run.
    Show { name: String },
    /// Create and save a new loop through a short interview.
    New,
    /// Import a loop from a TOML file.
    Import { path: PathBuf },
    /// Export a loop to a TOML file for sharing.
    Export { name: String, path: PathBuf },
    /// Remove a saved loop (built-ins always remain).
    Remove { name: String },
    /// Print the directory where loops are stored.
    Path,
    /// Run a loop to completion (defaults to the built-in `default` loop).
    Run {
        /// Name of a saved/built-in loop to run.
        name: Option<String>,
        /// Run an ad-hoc loop toward this goal instead (rubric-gated).
        #[arg(long)]
        goal: Option<String>,
        /// Override the loop's iteration ceiling.
        #[arg(long)]
        max_iterations: Option<u32>,
    },
}

/// The result of a top-level `loop` subcommand: either it's fully handled, or
/// it asks `main` to build the agent and run a loop (which needs a live model).
pub enum Dispatch {
    Done,
    Run(Box<LoopSpec>),
}

/// Handle the `oxen-harness loop` subcommand. Management actions complete here;
/// `run` returns a spec for `main` to execute once the agent is ready.
pub async fn handle_cli(action: LoopAction, ui: &Ui) -> Result<Dispatch> {
    let store = LoopStore::open().context("opening loop store")?;
    match action {
        LoopAction::List => print_list(ui, &store),
        LoopAction::Show { name } => show(ui, &store, &name)?,
        LoopAction::New => {
            new_interactive(ui, &store)?;
        }
        LoopAction::Import { path } => {
            let spec = store
                .import(&path)
                .with_context(|| format!("importing {}", path.display()))?;
            println!("  {} {}", ui.green("✓ imported loop"), ui.cream(&spec.name));
        }
        LoopAction::Export { name, path } => {
            let dest = store
                .export(&name, &path)
                .with_context(|| format!("exporting `{name}`"))?;
            println!(
                "  {} {}",
                ui.green("✓ exported to"),
                ui.cream(&dest.display().to_string())
            );
        }
        LoopAction::Remove { name } => {
            store
                .remove(&name)
                .with_context(|| format!("removing `{name}`"))?;
            println!("  {} {}", ui.brown("removed loop:"), ui.cream(&name));
        }
        LoopAction::Path => println!("{}", store.root().display()),
        LoopAction::Run {
            name,
            goal,
            max_iterations,
        } => {
            let spec = resolve_run_spec(&store, name, goal, max_iterations)?;
            return Ok(Dispatch::Run(Box::new(spec)));
        }
    }
    Ok(Dispatch::Done)
}

/// Handle an in-REPL `/loop ...` command. Returns `Ok(true)` if a running loop
/// was interrupted (Ctrl-C), so the REPL should end the session.
pub async fn handle_repl(
    rest: Option<String>,
    agent: &mut Agent,
    ui: &Ui,
    workspace_root: &Path,
) -> Result<bool> {
    let store = LoopStore::open().context("opening loop store")?;
    let rest = rest.unwrap_or_default();
    let mut parts = rest.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("");
    let payload = parts.next().map(str::trim).unwrap_or("");

    match sub {
        "" | "list" | "ls" => print_list(ui, &store),
        "show" => {
            if payload.is_empty() {
                println!("  {}", ui.dim("usage: /loop show <name>"));
            } else {
                show(ui, &store, payload)?;
            }
        }
        "new" | "create" => {
            new_interactive(ui, &store)?;
        }
        "run" | "go" => {
            let name = if payload.is_empty() { "default" } else { payload };
            let spec = store
                .resolve(name)
                .with_context(|| format!("no loop `{name}`"))?;
            return run(spec, agent, ui, workspace_root).await;
        }
        "goal" => {
            if payload.is_empty() {
                println!("  {}", ui.dim("usage: /loop goal <what should be true when done>"));
            } else {
                return run(LoopSpec::from_goal(payload), agent, ui, workspace_root).await;
            }
        }
        "export" => {
            let (name, path) = split_two(payload);
            match (name, path) {
                (Some(name), Some(path)) => {
                    let dest = store.export(&name, &path)?;
                    println!(
                        "  {} {}",
                        ui.green("✓ exported to"),
                        ui.cream(&dest.display().to_string())
                    );
                }
                _ => println!("  {}", ui.dim("usage: /loop export <name> <path>")),
            }
        }
        "import" => {
            if payload.is_empty() {
                println!("  {}", ui.dim("usage: /loop import <path>"));
            } else {
                let spec = store.import(PathBuf::from(payload))?;
                println!("  {} {}", ui.green("✓ imported loop"), ui.cream(&spec.name));
            }
        }
        "rm" | "remove" => {
            if payload.is_empty() {
                println!("  {}", ui.dim("usage: /loop rm <name>"));
            } else {
                store.remove(payload)?;
                println!("  {} {}", ui.brown("removed loop:"), ui.cream(payload));
            }
        }
        "path" => println!("  {}", ui.dim(&store.root().display().to_string())),
        _ => println!(
            "  {}",
            ui.dim("loop: list | run [name] | goal <text> | new | show <name> | import <path> | export <name> <path> | rm <name>"),
        ),
    }
    Ok(false)
}

/// Resolve the spec a `loop run` should execute.
fn resolve_run_spec(
    store: &LoopStore,
    name: Option<String>,
    goal: Option<String>,
    max_iterations: Option<u32>,
) -> Result<LoopSpec> {
    let mut spec = match (goal, name) {
        (Some(goal), name) => {
            let mut s = LoopSpec::from_goal(goal);
            if let Some(n) = name {
                s.name = n;
            }
            s
        }
        (None, Some(name)) => store
            .resolve(&name)
            .with_context(|| format!("no loop `{name}`"))?,
        (None, None) => store
            .resolve("default")
            .context("no built-in default loop")?,
    };
    if let Some(max) = max_iterations {
        spec.max_iterations = max;
    }
    Ok(spec)
}

/// Run a loop to completion, streaming progress. Returns `Ok(true)` if the user
/// interrupted with Ctrl-C.
pub async fn run(
    spec: LoopSpec,
    agent: &mut Agent,
    ui: &Ui,
    workspace_root: &Path,
) -> Result<bool> {
    let store = LoopStore::open().context("opening loop store")?;
    let journal_path = store.journal_path_for(&spec.name);

    print_header(ui, &spec);
    if spec.has_rubric_gate() {
        println!(
            "  {}",
            ui.dim("tip: a rubric is graded by the model — add a command gate (e.g. `cargo test`) for an objective check."),
        );
    }

    let runner =
        LoopRunner::new(spec.clone(), workspace_root.to_path_buf()).persisting_to(journal_path);
    let renderer = Rc::new(RefCell::new(TurnRenderer::new(ui.clone())));

    let r = renderer.clone();
    let ev_ui = ui.clone();
    let result = tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            renderer.borrow_mut().finish();
            return Ok(true);
        }
        res = runner.run(agent, |ev| render_event(ev, &r, &ev_ui)) => res,
    };
    renderer.borrow_mut().finish();

    match result {
        Ok(journal) => {
            print_summary(ui, &spec, &journal);
            Ok(false)
        }
        Err(e) => {
            println!("\n{}", ui.red(&ui.death()));
            println!("  {}", ui.dim(&format!("The trail guide says: {e}")));
            Ok(false)
        }
    }
}

fn render_event(ev: &LoopEvent, renderer: &Rc<RefCell<TurnRenderer>>, ui: &Ui) {
    match ev {
        LoopEvent::Started { .. } => {}
        LoopEvent::IterationStarted { iteration } => {
            println!();
            println!("  {}", ui.title(&format!("─── Iteration {iteration} ───")));
            renderer.borrow_mut().begin_thinking();
        }
        LoopEvent::Agent(e) => renderer.borrow_mut().on_event(e),
        LoopEvent::VerifyStarted { gate, command_gate } => {
            renderer.borrow_mut().finish();
            let what = if *command_gate {
                format!("Checking gate `{gate}` (running the verify command)…")
            } else {
                format!("Checking gate `{gate}` (scoring against the criteria)…")
            };
            println!("  {} {}", ui.brown("⚖"), ui.dim(&what));
        }
        LoopEvent::VerifySkipped { gate } => {
            renderer.borrow_mut().finish();
            println!(
                "  {} {}",
                ui.brown("↷"),
                ui.dim(&format!(
                    "gate `{gate}` skipped — no matching changes this pass"
                )),
            );
        }
        LoopEvent::VerifyPassed { gate } => {
            println!(
                "  {} {}",
                ui.green("✓"),
                ui.green(&format!("gate `{gate}` passed"))
            );
        }
        LoopEvent::VerifyFailed { gate, detail } => {
            println!(
                "  {} {}",
                ui.brown("✗"),
                ui.dim(&format!("gate `{gate}` failed — {}", truncate(detail, 160))),
            );
        }
        LoopEvent::Stopped { .. } => {}
    }
}

fn print_header(ui: &Ui, spec: &LoopSpec) {
    println!();
    println!("  {} {}", ui.title("▶ Loop:"), ui.cream(&spec.name));
    println!("    {} {}", ui.brown("goal:"), ui.cream(&spec.goal));
    for gate in spec.resolved_gates() {
        println!("    {} {}", ui.brown("gate:"), ui.dim(&gate.label()));
    }
    println!(
        "    {} {}",
        ui.brown("stop:"),
        ui.dim(&format!("after {} iterations", spec.max_iterations)),
    );
}

fn print_summary(ui: &Ui, spec: &LoopSpec, journal: &LoopJournal) {
    println!();
    if journal.succeeded() {
        println!(
            "  {} {}",
            ui.green("🏁 Loop complete:"),
            ui.cream(&format!(
                "gate is green after {} iteration(s)",
                journal.iterations()
            )),
        );
    } else {
        let reason = journal
            .stop
            .clone()
            .map(|s| s.headline())
            .unwrap_or_else(|| "stopped".to_string());
        println!("  {} {}", ui.brown("⛺ Loop stopped:"), ui.cream(&reason));
        println!(
            "    {} {}",
            ui.dim(&format!("{} iteration(s) recorded.", journal.iterations())),
            ui.dim(&format!("resume with  /loop run {}", spec.name)),
        );
    }
}

fn print_list(ui: &Ui, store: &LoopStore) {
    println!();
    println!("  {}", ui.title("Available loops"));
    for s in store.list() {
        let tag = if s.builtin {
            ui.dim("built-in")
        } else {
            ui.brown("custom")
        };
        println!(
            "  {} {}  {}",
            ui.cream(&format!("{:<16}", s.name)),
            tag,
            ui.dim(&s.description),
        );
        println!("    {} {}", ui.dim("gate:"), ui.dim(&s.verify));
    }
    println!();
    println!(
        "  {}",
        ui.dim("Run one with  /loop run <name>   ·   make your own with  /loop new")
    );
}

fn show(ui: &Ui, store: &LoopStore, name: &str) -> Result<()> {
    let spec = store
        .resolve(name)
        .with_context(|| format!("no loop `{name}`"))?;
    println!();
    println!("  {} {}", ui.title("Loop:"), ui.cream(&spec.name));
    if !spec.description.is_empty() {
        println!("  {}", ui.dim(&spec.description));
    }
    println!("  {} {}", ui.brown("goal:"), ui.cream(&spec.goal));
    for gate in spec.resolved_gates() {
        println!("  {} {}", ui.brown("gate:"), ui.dim(&gate.label()));
    }
    if !spec.success_criteria.is_empty() {
        println!("  {}", ui.brown("criteria:"));
        for c in &spec.success_criteria {
            println!("    {} {}", ui.dim("·"), ui.dim(c));
        }
    }
    println!(
        "  {} {}",
        ui.brown("stop:"),
        ui.dim(&format!("after {} iterations", spec.max_iterations)),
    );

    if let Some(journal) = store.load_journal(name) {
        let status = journal
            .stop
            .clone()
            .map(|s| s.headline())
            .unwrap_or_else(|| "in progress".to_string());
        println!(
            "  {} {}",
            ui.brown("last run:"),
            ui.dim(&format!("{} iteration(s), {status}", journal.iterations())),
        );
    }
    Ok(())
}

/// A short interview that builds and saves a new loop definition.
fn new_interactive(ui: &Ui, store: &LoopStore) -> Result<()> {
    let mut editor = DefaultEditor::new()?;
    println!();
    println!("  {}", ui.title("Build a new loop"));

    let name = prompt_line(&mut editor, ui, "Name (e.g. \"green tests\")")?;
    let name = if name.trim().is_empty() {
        "my-loop".to_string()
    } else {
        name
    };
    let goal = prompt_line(
        &mut editor,
        ui,
        "Goal — what should be true when it's done?",
    )?;

    let gate = picker::select(
        ui,
        "Verify gate",
        "How should each pass be checked?",
        &[
            Choice::new(
                "Command (objective)",
                "run a shell command; exit 0 = pass (recommended)",
            ),
            Choice::new(
                "Rubric (model-scored)",
                "a strict checker scores the work against the criteria",
            ),
        ],
        false,
    )?;
    let use_command = gate
        .as_ref()
        .and_then(|g| g.first())
        .map(|g| g.starts_with("Command"))
        .unwrap_or(true);

    let verify = if use_command {
        let cmd = prompt_line(&mut editor, ui, "Verify command (e.g. cargo test)")?;
        Verify::Command {
            command: cmd,
            timeout_ms: harness_loop::DEFAULT_VERIFY_TIMEOUT_MS,
        }
    } else {
        Verify::default()
    };

    let when = picker::select(
        ui,
        "When to run the gate",
        "Skip expensive checks on passes that didn't change matching files?",
        &[
            Choice::new("Every pass", "always run, whether or not files changed"),
            Choice::new(
                "Only when files change",
                "skip the gate when the pass didn't modify matching files",
            ),
        ],
        false,
    )?;
    let on_change = when
        .as_ref()
        .and_then(|w| w.first())
        .map(|w| w.starts_with("Only"))
        .unwrap_or(false);
    let run_when = if on_change {
        let globs_line = prompt_line(
            &mut editor,
            ui,
            "File globs, comma-separated (optional — empty = any change, e.g. **/*.rs)",
        )?;
        let paths: Vec<String> = globs_line
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        RunWhen::OnChange { paths }
    } else {
        RunWhen::Always
    };
    let gate = Gate {
        name: if verify.is_command() {
            "verify".to_string()
        } else {
            "rubric".to_string()
        },
        run_when,
        verify,
    };

    let criteria_line = prompt_line(
        &mut editor,
        ui,
        "Success criteria, comma-separated (optional)",
    )?;
    let success_criteria: Vec<String> = criteria_line
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let spec = LoopSpec {
        schema_version: harness_loop::LOOP_SCHEMA_VERSION,
        name: name.clone(),
        description: String::new(),
        goal,
        success_criteria,
        verify: None,
        gates: vec![gate],
        max_iterations: harness_loop::DEFAULT_MAX_ITERATIONS,
        token_budget: None,
    };
    let path = store.save(&spec).context("saving the loop")?;
    println!(
        "  {} {}",
        ui.green("✓ saved loop:"),
        ui.cream(&format!("{} → {}", name, path.display())),
    );
    println!("  {}", ui.dim(&format!("run it with  /loop run {name}")));
    Ok(())
}

fn prompt_line(editor: &mut DefaultEditor, ui: &Ui, label: &str) -> Result<String> {
    let prompt = format!("  {} ", ui.accent(&format!("{label}:")));
    Ok(editor.readline(&prompt)?.trim().to_string())
}

/// Split `"<name> <path>"`, keeping the path as the remainder.
fn split_two(s: &str) -> (Option<String>, Option<String>) {
    let mut parts = s.trim().splitn(2, char::is_whitespace);
    let a = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    let b = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_two_separates_name_and_path() {
        assert_eq!(
            split_two("green-tests out.toml"),
            (Some("green-tests".into()), Some("out.toml".into()))
        );
        assert_eq!(split_two("solo"), (Some("solo".into()), None));
        assert_eq!(split_two(""), (None, None));
    }

    #[test]
    fn ad_hoc_run_spec_uses_goal_and_override() {
        let dir = tempfile::tempdir().unwrap();
        let store = LoopStore::with_root(dir.path()).unwrap();
        let spec = resolve_run_spec(&store, None, Some("make it green".into()), Some(3)).unwrap();
        assert_eq!(spec.goal, "make it green");
        assert_eq!(spec.max_iterations, 3);
    }

    #[test]
    fn default_run_spec_falls_back_to_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let store = LoopStore::with_root(dir.path()).unwrap();
        let spec = resolve_run_spec(&store, None, None, None).unwrap();
        assert_eq!(spec.name, "default");
    }
}
