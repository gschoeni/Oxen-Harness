//! `oxen-harness` — an interactive, streaming agentic coding REPL.

mod almanac;
mod ask;
mod attach;
mod brave;
mod canvas;
mod diff;
mod live;
mod local;
mod loop_cmd;
mod markdown;
mod oxen_cmd;
mod picker;
mod plan;
mod queue;
mod queue_cmd;
mod render;
mod repl;
mod theme;
mod theme_cmd;
mod trace_cmd;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use harness_agent::{Agent, AgentConfig};
use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{ToolRegistry, Workspace};
use rustyline::DefaultEditor;

use crate::queue::MessageQueue;
use crate::render::{truncate, TurnRenderer};
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

    /// API host\[:port\], e.g. localhost:3001. Expanded to a base URL
    /// (http for local hosts, https otherwise, with an /api/ai path).
    #[arg(long)]
    host: Option<String>,

    /// Resume a previous session by id (printed on the death screen when you
    /// quit). Restores that session's transcript, workspace, and model.
    #[arg(long, value_name = "SESSION_ID")]
    resume: Option<String>,

    /// Run a local model with llama.cpp instead of a remote endpoint, by
    /// catalog id (e.g. qwen3-8b). Downloads it if needed, then serves it
    /// locally for the session. See `oxen-harness models list`.
    #[arg(long, value_name = "MODEL_ID")]
    local: Option<String>,

    #[command(subcommand)]
    command: Option<TopCommand>,
}

#[derive(Debug, clap::Subcommand)]
enum TopCommand {
    /// Manage local models run with llama.cpp (list / pull / remove / path).
    Models {
        #[command(subcommand)]
        action: local::ModelsAction,
    },
    /// Manage themes (list / use / export / import / new).
    Theme {
        #[command(subcommand)]
        action: theme_cmd::ThemeAction,
    },
    /// Run and manage self-verifying agent loops (run / list / new / show ...).
    Loop {
        #[command(subcommand)]
        action: loop_cmd::LoopAction,
    },
    /// Export or share a conversation as an Oxen repo (transcript + attachments).
    Trace {
        #[command(subcommand)]
        action: trace_cmd::TraceAction,
    },
    /// Version your harness config (~/.oxen-harness) with Oxen (init/snapshot/status).
    Oxen {
        #[command(subcommand)]
        action: oxen_cmd::OxenAction,
    },
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

    // Load ~/.oxen-harness/.env into the environment (without overriding vars
    // already set) so saved API keys are available before anything reads them.
    harness_config::secrets::load();

    let theme = harness_theme::Store::open()
        .map(|s| s.load_active())
        .unwrap_or_default();
    let mut ui = Ui::detect(Arc::new(theme));
    let mut args = Args::parse();

    // Subcommands that manage state and exit before the REPL. `loop run` is the
    // exception: it needs a live agent, so it falls through and runs once the
    // session is built (instead of entering the interactive REPL).
    let mut pending_loop: Option<harness_loop::LoopSpec> = None;
    match args.command.take() {
        Some(TopCommand::Models { action }) => return local::run_models(action, &ui).await,
        Some(TopCommand::Theme { action }) => return theme_cmd::run_theme(action, &ui).await,
        Some(TopCommand::Loop { action }) => match loop_cmd::handle_cli(action, &ui).await? {
            loop_cmd::Dispatch::Done => return Ok(()),
            loop_cmd::Dispatch::Run(spec) => pending_loop = Some(*spec),
        },
        Some(TopCommand::Trace { action }) => return trace_cmd::run_trace(action, &ui),
        Some(TopCommand::Oxen { action }) => return oxen_cmd::run_oxen(action, &ui),
        None => {}
    }

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

    let workspace_root = match args.workspace.clone() {
        Some(p) => p,
        None => match resume_meta.as_ref() {
            Some(m) => PathBuf::from(&m.workspace),
            None => std::env::current_dir().context("could not determine current directory")?,
        },
    };
    let workspace = Workspace::new(&workspace_root)
        .with_context(|| format!("opening workspace {}", workspace_root.display()))?;

    // Resolve which model to run and how to reach it (cloud or a local
    // llama-server). The server guard is kept alive for the whole session —
    // dropping it shuts the background process down.
    let Endpoint {
        client,
        model,
        context_window,
        local_server: _local_server,
    } = resolve_endpoint(&args, resume_meta.as_ref(), &ui).await;

    // Migrate any legacy plaintext keys out of connection.json into .env (the
    // shared store, already loaded into the env above) so web search and auth
    // work without re-entering keys.
    let _ = harness_runtime::connection::load();

    let tools = build_tool_registry(&workspace, &ui);
    let base_url = client.base_url().to_string();
    let config = agent_config(&model, context_window, &tools, &workspace);

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
                provider: "oxen".into(),
                base_url: base_url.clone(),
                context_window: context_window.map(|w| w as i64),
                ..Default::default()
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
            &session,
            agent.tokens_used(),
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

    // `oxen-harness loop run ...`: run the loop once, then exit (no REPL).
    if let Some(spec) = pending_loop {
        loop_cmd::run(spec, &mut agent, &ui, workspace.root()).await?;
        return Ok(());
    }

    // Interactive TTYs use the bordered, bottom-pinned box composer (multi-line,
    // history, queue) for both idle and in-turn input. Pipes, dumb terminals, and
    // an explicit `OXEN_HARNESS_CLASSIC_INPUT` fall back to the classic readline.
    let use_box = live_enabled(&ui) && std::env::var_os("OXEN_HARNESS_CLASSIC_INPUT").is_none();
    let ctx = ReplContext {
        store: &store,
        session: &session,
        workspace_root: workspace.root(),
        base_url: &base_url,
    };
    if use_box {
        run_box_repl(&mut agent, &mut ui, &ctx).await
    } else {
        run_classic_repl(&mut agent, &mut ui, &ctx).await
    }
}

/// The resolved inference endpoint for a session: the model, a client bound to
/// it, the context window to budget against (a local server's real size, else
/// derived from the model name), and — for `--local` — the llama-server process
/// to keep alive for the session's lifetime.
struct Endpoint {
    client: OxenClient,
    model: String,
    context_window: Option<usize>,
    local_server: Option<harness_local::LocalServer>,
}

/// Resolve which model to run and how to reach it.
///
/// `--local <id>` runs a model on this machine via llama.cpp; absent any
/// explicit choice we restore the last local model the user activated (in the
/// desktop dropdown or a prior `--local` run). Anything else connects to a
/// remote Oxen.ai-style endpoint. A *restored* (non-explicit) local model that
/// can't start here falls back to the cloud, while an explicit `--local` failure
/// — or an unreachable cloud endpoint — prints the death screen and exits.
async fn resolve_endpoint(args: &Args, resume_meta: Option<&SessionMeta>, ui: &Ui) -> Endpoint {
    // A cloud client + model, honoring the persisted dropdown selection when
    // nothing is given on the CLI. Used directly, and as the fallback when a
    // restored local model can't start.
    let cloud = |ui: &Ui| -> Endpoint {
        let model = args
            .model
            .clone()
            .or_else(|| resume_meta.map(|m| m.model.clone()))
            .unwrap_or_else(harness_runtime::models::selected);

        // Precedence for the base URL: --base-url > --host > env (OXEN_BASE_URL
        // / OXEN_HOST) > default Oxen.ai endpoint.
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
                eprintln!("\n{}", ui.red(&ui.death()));
                eprintln!("  {}", ui.dim(&format!("The trail guide says: {e}")));
                eprintln!(
                    "  {}",
                    ui.dim("Set OXEN_API_KEY, or log in with the `oxen` CLI, then set out again.")
                );
                std::process::exit(1);
            }
        };
        Endpoint {
            client,
            model,
            context_window: None,
            local_server: None,
        }
    };

    let explicit_local = args.local.clone();
    let local_id = explicit_local.clone().or_else(|| {
        if args.model.is_none() && args.resume.is_none() {
            harness_runtime::models::active_local()
        } else {
            None
        }
    });
    let Some(local_id) = local_id else {
        return cloud(ui);
    };

    match local::start_for(&local_id, ui).await {
        Ok((server, alias)) => {
            let client = OxenClient::new(server.base_url(), "local", &alias);
            Endpoint {
                client,
                model: alias,
                // Budget against the server's actual context size (smaller than
                // the model's theoretical max).
                context_window: Some(server.context_size() as usize),
                local_server: Some(server),
            }
        }
        // An explicit `--local` failure is fatal; a *restored* local model that
        // can't start (e.g. runtime not installed here) falls back to cloud.
        Err(e) if explicit_local.is_some() => {
            eprintln!("\n{}", ui.red(&ui.death()));
            eprintln!("  {}", ui.dim(&format!("The trail guide says: {e}")));
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!(
                "  {}",
                ui.dim(&format!(
                    "Local model {local_id} unavailable ({e}); using the cloud model."
                ))
            );
            cloud(ui)
        }
    }
}

/// The CLI's tool set: the workspace file/shell/git/web tools, plus the
/// interactive question picker and the canvas document viewer wired to their CLI
/// front ends.
fn build_tool_registry(workspace: &Workspace, ui: &Ui) -> ToolRegistry {
    let mut tools = ToolRegistry::default_for_workspace(workspace.clone());
    // Let the agent interview the user via the interactive terminal picker.
    tools.register_typed(harness_tools::AskUserTool::new(Arc::new(
        ask::CliAsker::new(ui.clone()),
    )));
    // Show documents in the canvas: write them to disk, open web docs in the
    // browser, and preview text docs inline.
    tools.register_typed(harness_tools::CanvasTool::new(Arc::new(
        canvas::CliCanvasSink,
    )));
    tools
}

/// The agent configuration for a CLI session: model + window, a system prompt
/// that advertises web search only when the Brave key enabled it (the canvas
/// tool is always registered), and an attachment root so images/PDFs are stored
/// on disk rather than inlined.
fn agent_config(
    model: &str,
    context_window: Option<usize>,
    tools: &ToolRegistry,
    workspace: &Workspace,
) -> AgentConfig {
    AgentConfig {
        model: model.to_string(),
        context_window,
        system_prompt: Some(harness_agent::system_prompt_with_env(
            tools.get(harness_tools::WEB_SEARCH_TOOL).is_some(),
            true,
            workspace.root(),
        )),
        attachment_root: Some(workspace.root().to_path_buf()),
        ..AgentConfig::default()
    }
}

/// The classic readline REPL for pipes, dumb terminals, and
/// `OXEN_HARNESS_CLASSIC_INPUT`: a blocking line prompt with persistent,
/// cross-session Up-arrow history, dispatching each line through [`handle_line`].
/// The live-terminal counterpart is [`run_box_repl`].
async fn run_classic_repl(agent: &mut Agent, ui: &mut Ui, ctx: &ReplContext<'_>) -> Result<()> {
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

/// Immutable, session-scoped context for a REPL run: the values set once at
/// startup and threaded through the line handler unchanged. Bundling them keeps
/// [`handle_line`] to the state that actually varies (the agent, UI, and queue).
struct ReplContext<'a> {
    store: &'a Arc<HistoryStore>,
    session: &'a str,
    workspace_root: &'a Path,
    base_url: &'a str,
}

/// The interactive REPL for a live terminal: a bordered, bottom-pinned box
/// composer (multi-line, history, queue) for both idle input and in-turn
/// queueing. Reads one submission at idle via [`live::read_idle`], then dispatches
/// it (a `/command` or a prompt) through [`handle_line`] in cooked mode.
async fn run_box_repl(agent: &mut Agent, ui: &mut Ui, ctx: &ReplContext<'_>) -> Result<()> {
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
        let idle = live::read_idle(ui, &mut queue, &mut history, &seed).await?;
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
        Command::Model(None) => {
            println!("  {} {}", ui.brown("oxen yoked:"), ui.cream(agent.model()))
        }
        Command::Model(Some(m)) => {
            agent.set_model(&m);
            // Persist the choice (clearing any local selection) so it's the
            // default next launch — here and in the desktop dropdown.
            let _ = harness_runtime::models::set_selected(&m);
            println!("  {} {}", ui.brown("fresh oxen yoked:"), ui.accent(&m));
        }
        Command::Export(dest) => export(ctx.store, ctx.session, dest, ui)?,
        Command::Prompt(prompt) => {
            // Ctrl-C mid-stream ends the expedition just like quitting does.
            if run_turn_and_drain(agent, &prompt, ui, queue, carryover).await? {
                print!("{}", theme::death_screen(ui, ctx.session));
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether the sticky-bottom live composer should drive this turn: only for an
/// interactive, animating TTY. Pipes, tests, `NO_COLOR`, and `TERM=dumb` fall
/// back to the classic blocking prompt with unchanged behavior.
fn live_enabled(ui: &Ui) -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && ui.animates()
}

/// Run `prompt`, then auto-drain any stacked messages in order. On a live TTY
/// this hands off to the sticky composer (which lets the user keep stacking
/// while turns run); otherwise it runs the classic prompt and drains after.
/// Returns `Ok(true)` when the session should end.
async fn run_turn_and_drain(
    agent: &mut Agent,
    prompt: &str,
    ui: &Ui,
    queue: &mut MessageQueue,
    carryover: &mut String,
) -> Result<bool> {
    if live_enabled(ui) {
        // The live composer hands back any half-typed next message so the idle
        // prompt can keep it instead of wiping it when the turn ends.
        let (exit, draft) = live::run_prompt(agent, prompt, ui, queue).await?;
        *carryover = draft;
        return Ok(exit);
    }
    if run_prompt(agent, prompt, ui).await? {
        return Ok(true);
    }
    while !queue.is_empty() {
        let next = queue.pop_front().expect("queue is non-empty");
        println!(
            "  {} {}",
            ui.brown("▶ rolling the wagon:"),
            ui.cream(&truncate(&next, 80)),
        );
        if run_prompt(agent, &next, ui).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Run one turn, racing it against Ctrl-C. Returns `Ok(true)` if the user
/// interrupted the stream (the caller should then end the session).
async fn run_prompt(agent: &mut Agent, prompt: &str, ui: &Ui) -> Result<bool> {
    let (text, attachments, warnings) = attach::extract_attachments(prompt);
    for w in &warnings {
        println!("  {} {}", ui.red("⚠"), ui.dim(w));
    }
    if !attachments.is_empty() {
        let names: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
        println!(
            "  {} {}",
            ui.green("📎 attached:"),
            ui.cream(&names.join(", "))
        );
    }

    let renderer = Rc::new(RefCell::new(TurnRenderer::new(ui.clone())));
    renderer.borrow_mut().begin_thinking();

    let cb = renderer.clone();
    let result = tokio::select! {
        // Cancel the in-flight turn on Ctrl-C and signal an exit.
        _ = tokio::signal::ctrl_c() => {
            renderer.borrow_mut().finish();
            return Ok(true);
        }
        result = agent.run_turn_with_attachments(text, attachments, move |event| cb.borrow_mut().on_event(event)) => result,
    };
    renderer.borrow_mut().finish();
    let needs_brave_key = renderer.borrow().needs_brave_key();

    match result {
        Ok(_) => {
            print_context_usage(agent, ui);
            // Offer to set up web search if the model tried it without a key.
            if needs_brave_key {
                brave::prompt_after_failed_search(ui);
            }
            Ok(false)
        }
        Err(e) => {
            println!("\n{}", ui.red(&ui.death()));
            println!("  {}", ui.dim(&format!("The trail guide says: {e}")));
            Ok(false)
        }
    }
}

/// A subtle trailer showing how full the model's context window is.
fn print_context_usage(agent: &Agent, ui: &Ui) {
    println!("{}", context_usage_line(agent, ui));
}

/// The context-usage trailer as a (themed, indented) line — shared by the
/// classic prompt and the live composer (which writes it into its scroll region).
pub(crate) fn context_usage_line(agent: &Agent, ui: &Ui) -> String {
    let used = agent.context_tokens();
    let window = agent.context_window();
    let pct = if window > 0 {
        (used * 100 / window).min(100)
    } else {
        0
    };
    format!(
        "  {}",
        ui.dim(&format!(
            "🧭 context {} / {} tokens ({pct}%)",
            human_tokens(used),
            human_tokens(window),
        )),
    )
}

/// Human-friendly token count: `980`, `12.3k`, `1.2M`.
fn human_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
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
    let path = harness_config::paths::history_db()
        .map_err(|e| anyhow::anyhow!("resolving history path: {e}"))?;
    HistoryStore::open(&path).with_context(|| format!("opening history at {}", path.display()))
}

/// File backing the readline prompt history, so Up-arrow recalls prompts typed
/// in previous CLI sessions (separate from the SQLite chat transcript store).
fn prompt_history_path() -> Option<std::path::PathBuf> {
    harness_config::paths::prompt_history_file().ok()
}

#[cfg(test)]
mod tests {
    use super::human_tokens;

    #[test]
    fn human_tokens_scales_units() {
        assert_eq!(human_tokens(980), "980");
        assert_eq!(human_tokens(12_300), "12.3k");
        assert_eq!(human_tokens(1_200_000), "1.2M");
    }
}
