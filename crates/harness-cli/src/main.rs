//! `oxen-harness` — an interactive, streaming agentic coding REPL.

mod repl;

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use harness_agent::{Agent, AgentConfig, AgentEvent};
use harness_core::{DEFAULT_BASE_URL, DEFAULT_MODEL};
use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{ToolRegistry, Workspace};
use rustyline::DefaultEditor;

use crate::repl::{parse_command, Command, HELP_TEXT};

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

    let args = Args::parse();
    let model = args.model.unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let workspace_root = match args.workspace {
        Some(p) => p,
        None => std::env::current_dir().context("could not determine current directory")?,
    };
    let workspace = Workspace::new(&workspace_root)
        .with_context(|| format!("opening workspace {}", workspace_root.display()))?;

    let client = match OxenClient::from_default_config() {
        Ok(c) => c.with_model(&model),
        Err(e) => {
            eprintln!("Could not start: {e}");
            eprintln!("Set OXEN_API_KEY, or log in with the `oxen` CLI, then try again.");
            std::process::exit(1);
        }
    };

    let tools = ToolRegistry::default_for_workspace(workspace.clone());
    let store = Arc::new(open_store()?);
    let session = store.create_session(&SessionMeta {
        workspace: workspace.root().display().to_string(),
        model: model.clone(),
    })?;

    let config = AgentConfig {
        model: model.clone(),
        ..AgentConfig::default()
    };
    let mut agent = Agent::new(client, tools, store.clone(), session.clone(), config)?;

    print_banner(&model, workspace.root(), &session);

    let mut editor = DefaultEditor::new()?;
    loop {
        match editor.readline("oxen» ") {
            Ok(line) => {
                let _ = editor.add_history_entry(line.as_str());
                if handle_line(&line, &mut agent, &store, &session).await? {
                    break;
                }
            }
            // Ctrl-C / Ctrl-D: leave cleanly.
            Err(rustyline::error::ReadlineError::Interrupted)
            | Err(rustyline::error::ReadlineError::Eof) => {
                println!("bye 🐂");
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
) -> Result<bool> {
    match parse_command(line) {
        Command::Empty => {}
        Command::Exit => {
            println!("bye 🐂");
            return Ok(true);
        }
        Command::Help => println!("{HELP_TEXT}"),
        Command::Model(None) => println!("model: {}", agent.model()),
        Command::Model(Some(m)) => {
            agent.set_model(&m);
            println!("model set to {m}");
        }
        Command::Export(dest) => export(store, session, dest)?,
        Command::Prompt(prompt) => run_prompt(agent, &prompt).await?,
    }
    Ok(false)
}

async fn run_prompt(agent: &mut Agent, prompt: &str) -> Result<()> {
    let result = agent
        .run_turn(prompt, |event| match event {
            AgentEvent::Token(t) => {
                print!("{t}");
                let _ = std::io::stdout().flush();
            }
            AgentEvent::ToolStart { name, arguments } => {
                println!("\n  ⚙ {name}({})", truncate(arguments, 200));
            }
            AgentEvent::ToolEnd { name, result } => {
                println!("  ✓ {name} → {}", truncate(result, 200));
            }
        })
        .await;

    println!();
    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("error: {e}");
            Ok(())
        }
    }
}

fn export(store: &Arc<HistoryStore>, session: &str, dest: Option<String>) -> Result<()> {
    let jsonl = store.export_jsonl(session)?;
    match dest {
        Some(path) => {
            std::fs::write(&path, &jsonl).with_context(|| format!("writing {path}"))?;
            println!("exported {} messages to {path}", jsonl.lines().count());
        }
        None => println!(
            "session {session}: {} messages (pass a path to write JSONL)",
            jsonl.lines().count()
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

fn print_banner(model: &str, workspace: &std::path::Path, session: &str) {
    println!("oxen-harness {}", env!("CARGO_PKG_VERSION"));
    println!("provider  : Oxen.ai ({DEFAULT_BASE_URL})");
    println!("model     : {model}");
    println!("workspace : {}", workspace.display());
    println!("session   : {session}");
    println!("Type /help for commands. Ctrl-D to exit.\n");
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
