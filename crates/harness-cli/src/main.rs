//! `oxen-harness` — an interactive, streaming agentic coding REPL.

mod markdown;
mod repl;
mod theme;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use harness_agent::{Agent, AgentConfig, AgentEvent};
use harness_core::DEFAULT_MODEL;
use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{ToolRegistry, Workspace};
use rustyline::DefaultEditor;

use crate::repl::{parse_command, Command};
use crate::theme::Ui;

/// Interactive agentic coding harness powered by Oxen.ai.
#[derive(Debug, Parser)]
#[command(name = "oxen-harness", version, about)]
struct Args {
    /// Model to use (any Oxen.ai chat-completions model with tool calling).
    #[arg(long)]
    model: Option<String>,

    /// Working directory the agent is scoped to (defaults to the current dir).
    #[arg(long)]
    workspace: Option<PathBuf>,

    /// Full API base URL, e.g. http://localhost:3001/api/ai.
    /// Overrides --host and the OXEN_BASE_URL/OXEN_HOST env vars.
    #[arg(long)]
    base_url: Option<String>,

    /// API host[:port], e.g. localhost:3001. Expanded to a base URL
    /// (http for local hosts, https otherwise, with an /api/ai path).
    #[arg(long)]
    host: Option<String>,

    /// Resume a previous session by id (printed on the death screen when you
    /// quit). Restores that session's transcript, workspace, and model.
    #[arg(long, value_name = "SESSION_ID")]
    resume: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let ui = Ui::detect();
    let args = Args::parse();

    let store = Arc::new(open_store()?);

    // When resuming, the saved session supplies default workspace + model.
    let resume_meta = match &args.resume {
        Some(id) => match store.session_meta(id) {
            Ok(meta) => Some(meta),
            Err(e) => {
                eprintln!(
                    "\n{}",
                    ui.red("No trail journal found for that session id.")
                );
                eprintln!("  {}", ui.dim(&format!("{e}")));
                std::process::exit(1);
            }
        },
        None => None,
    };

    let model = args
        .model
        .clone()
        .or_else(|| resume_meta.as_ref().map(|m| m.model.clone()))
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let workspace_root = match args.workspace.clone() {
        Some(p) => p,
        None => match resume_meta.as_ref() {
            Some(m) => PathBuf::from(&m.workspace),
            None => std::env::current_dir().context("could not determine current directory")?,
        },
    };
    let workspace = Workspace::new(&workspace_root)
        .with_context(|| format!("opening workspace {}", workspace_root.display()))?;

    // Precedence for the base URL: --base-url > --host > env (OXEN_BASE_URL /
    // OXEN_HOST) > default Oxen.ai endpoint.
    let base_url_override = args
        .base_url
        .clone()
        .or_else(|| args.host.as_deref().map(harness_llm::base_url_from_host));
    let client_result = match base_url_override {
        Some(base_url) => OxenClient::connect(base_url, &model),
        None => OxenClient::from_default_config().map(|c| c.with_model(&model)),
    };
    let client = match client_result {
        Ok(c) => c,
        Err(e) => {
            eprintln!("\n{}", ui.red(theme::death()));
            eprintln!("  {}", ui.dim(&format!("The trail guide says: {e}")));
            eprintln!(
                "  {}",
                ui.dim("Set OXEN_API_KEY, or log in with the `oxen` CLI, then set out again.")
            );
            std::process::exit(1);
        }
    };

    let tools = ToolRegistry::default_for_workspace(workspace.clone());
    let base_url = client.base_url().to_string();
    let config = AgentConfig {
        model: model.clone(),
        ..AgentConfig::default()
    };

    // Resume an existing transcript, or strike out on a fresh session.
    let (mut agent, session, resumed_entries) = match &args.resume {
        Some(id) => {
            let agent = Agent::resume_from_store(client, tools, store.clone(), id.clone(), config)?;
            let entries = agent.messages().len();
            (agent, id.clone(), Some(entries))
        }
        None => {
            let session = store.create_session(&SessionMeta {
                workspace: workspace.root().display().to_string(),
                model: model.clone(),
            })?;
            let agent = Agent::new(client, tools, store.clone(), session.clone(), config)?;
            (agent, session, None)
        }
    };

    print!(
        "{}",
        theme::banner(
            &ui,
            &base_url,
            &model,
            &workspace.root().display().to_string(),
            &session
        )
    );
    println!();
    if let Some(n) = resumed_entries {
        println!(
            "  {} {}",
            ui.green("↺ Picking up the trail:"),
            ui.cream(&format!("{n} journal entries restored")),
        );
        println!();
    }

    let prompt = theme::prompt(&ui);
    let mut editor = DefaultEditor::new()?;
    loop {
        match editor.readline(&prompt) {
            Ok(line) => {
                let _ = editor.add_history_entry(line.as_str());
                if handle_line(&line, &mut agent, &store, &session, &ui).await? {
                    break;
                }
            }
            // Ctrl-C / Ctrl-D: leave cleanly.
            Err(rustyline::error::ReadlineError::Interrupted)
            | Err(rustyline::error::ReadlineError::Eof) => {
                print!("{}", theme::death_screen(&ui, &session));
                break;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Handle one line of input. Returns `Ok(true)` when the REPL should exit.
async fn handle_line(
    line: &str,
    agent: &mut Agent,
    store: &Arc<HistoryStore>,
    session: &str,
    ui: &Ui,
) -> Result<bool> {
    match parse_command(line) {
        Command::Empty => {}
        Command::Exit => {
            print!("{}", theme::death_screen(ui, session));
            return Ok(true);
        }
        Command::Help => print!("{}", theme::help(ui)),
        Command::Model(None) => {
            println!("  {} {}", ui.brown("oxen yoked:"), ui.cream(agent.model()))
        }
        Command::Model(Some(m)) => {
            agent.set_model(&m);
            println!("  {} {}", ui.brown("fresh oxen yoked:"), ui.accent(&m));
        }
        Command::Export(dest) => export(store, session, dest, ui)?,
        Command::Prompt(prompt) => {
            // Ctrl-C mid-stream ends the expedition just like quitting does.
            if run_prompt(agent, &prompt, ui).await? {
                print!("{}", theme::death_screen(ui, session));
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Renders one turn's live progress: a trail-status spinner while the model
/// thinks or a tool runs, assistant text streamed through the Markdown
/// renderer, and themed tool lines.
struct TurnRenderer {
    ui: Ui,
    spinner: Option<theme::Spinner>,
    md: Option<markdown::MarkdownStream<std::io::Stdout>>,
}

impl TurnRenderer {
    fn new(ui: Ui) -> Self {
        Self {
            ui,
            spinner: None,
            md: None,
        }
    }

    /// Flush and drop the active Markdown segment, if any.
    fn end_markdown(&mut self) {
        if let Some(mut md) = self.md.take() {
            md.finish();
        }
    }

    fn begin_thinking(&mut self) {
        self.stop_spinner();
        self.spinner = Some(theme::Spinner::start(&self.ui, theme::THINKING));
    }

    fn begin_working(&mut self, tool: &str) {
        self.stop_spinner();
        self.spinner = Some(theme::Spinner::start(&self.ui, theme::tool_verbs(tool)));
    }

    fn stop_spinner(&mut self) {
        if let Some(s) = self.spinner.take() {
            s.stop();
        }
    }

    fn on_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Token(t) => {
                if self.md.is_none() {
                    self.stop_spinner();
                    println!();
                    self.md = Some(markdown::MarkdownStream::new(self.ui, std::io::stdout()));
                }
                if let Some(md) = self.md.as_mut() {
                    md.push(t);
                }
            }
            AgentEvent::ToolStart { name, arguments } => {
                self.stop_spinner();
                self.end_markdown();
                let verb = theme::tool_verbs(name)
                    .first()
                    .copied()
                    .unwrap_or("Working the trail");
                println!(
                    "  {} {}  {}",
                    self.ui.green("◆"),
                    self.ui.accent(verb),
                    self.ui
                        .dim(&format!("{name}({})", truncate(arguments, 100))),
                );
                self.begin_working(name);
            }
            AgentEvent::ToolEnd { name: _, result } => {
                self.stop_spinner();
                println!(
                    "  {} {}",
                    self.ui.brown("└─"),
                    self.ui.dim(&truncate(result, 140)),
                );
                self.begin_thinking();
            }
        }
    }

    fn finish(&mut self) {
        self.stop_spinner();
        self.end_markdown();
    }
}

/// Run one turn, racing it against Ctrl-C. Returns `Ok(true)` if the user
/// interrupted the stream (the caller should then end the session).
async fn run_prompt(agent: &mut Agent, prompt: &str, ui: &Ui) -> Result<bool> {
    let renderer = Rc::new(RefCell::new(TurnRenderer::new(*ui)));
    renderer.borrow_mut().begin_thinking();

    let cb = renderer.clone();
    let result = tokio::select! {
        // Cancel the in-flight turn on Ctrl-C and signal an exit.
        _ = tokio::signal::ctrl_c() => {
            renderer.borrow_mut().finish();
            return Ok(true);
        }
        result = agent.run_turn(prompt, move |event| cb.borrow_mut().on_event(event)) => result,
    };
    renderer.borrow_mut().finish();

    match result {
        Ok(_) => Ok(false),
        Err(e) => {
            println!("\n{}", ui.red(theme::death()));
            println!("  {}", ui.dim(&format!("The trail guide says: {e}")));
            Ok(false)
        }
    }
}

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

fn open_store() -> Result<HistoryStore> {
    let dir = dirs::home_dir()
        .map(|h| h.join(".oxen-harness"))
        .context("could not determine home directory")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("history.sqlite");
    HistoryStore::open(&path).with_context(|| format!("opening history at {}", path.display()))
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let kept: String = s.chars().take(max).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_collapses_newlines_and_caps_length() {
        assert_eq!(truncate("a\nb", 10), "a b");
        let long = "x".repeat(50);
        let out = truncate(&long, 10);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 11);
    }
}
