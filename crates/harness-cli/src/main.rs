//! `oxen-harness` — an interactive, streaming agentic coding REPL.

mod almanac;
mod ask;
mod attach;
mod auth_cmd;
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

use crate::picker::Choice;
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

    /// Continue your most recent session — transcript, workspace, and model
    /// restored. Handy after a provider outage or lost internet: relaunch with
    /// -c and /retry the turn that died.
    #[arg(long = "continue", short = 'c', conflicts_with = "resume")]
    continue_last: bool,

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

    // `--continue` is `--resume` pointed at the newest session on record.
    if args.continue_last {
        match store.list_sessions()?.first() {
            Some(latest) => args.resume = Some(latest.id.clone()),
            None => {
                eprintln!(
                    "\n{}",
                    ui.red("No previous expedition to continue — set out fresh.")
                );
                std::process::exit(1);
            }
        }
    }

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
        // A transcript that stops mid-turn (the reply never landed — provider
        // error, no internet, a crash) can be continued in place.
        if ends_mid_turn(agent.messages()) {
            println!(
                "  {} {}",
                ui.red("⚠"),
                ui.dim("this expedition stopped mid-turn — /retry picks up where it left off"),
            );
        }
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
        let base_url = args
            .base_url
            .clone()
            .or_else(|| args.host.as_deref().map(harness_llm::base_url_from_host))
            .unwrap_or_else(harness_llm::resolve_base_url);
        let client = match OxenClient::connect(base_url.clone(), &model) {
            Ok(c) => c,
            // No key resolves anywhere — offer the masked `/auth` entry card
            // right here so a first run can be authenticated without leaving.
            Err(e) => {
                match auth_cmd::prompt_for_missing_key(ui, &base_url) {
                    Some(key) => OxenClient::new(base_url, key, &model),
                    None => {
                        eprintln!("\n{}", ui.red(&ui.death()));
                        eprintln!("  {}", ui.dim(&format!("The trail guide says: {e}")));
                        eprintln!(
                        "  {}",
                        ui.dim("Set OXEN_API_KEY, or log in with the `oxen` CLI, then set out again.")
                    );
                        std::process::exit(1);
                    }
                }
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
/// front ends, with the user's saved tool preferences and skills applied — the
/// same setup the desktop app builds, so sessions behave identically.
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
    // Honor the user's saved tool preferences (Settings → Tools in the desktop
    // app): custom HTTP tools register, disabled tools drop, and description
    // overrides layer into the definitions the model sees.
    harness_runtime::tools::load().apply(&mut tools);
    // Skills load on demand through the `skill` tool; it's only registered when
    // the user has enabled skills, so an empty set costs no prompt tokens.
    if let Some(skill_tool) = harness_runtime::skills::enabled_tool(workspace.root()) {
        tools.register_typed(skill_tool);
    }
    tools
}

/// The agent configuration for a CLI session: model + window, a system prompt
/// gated on which tools actually survived the user's preferences (so the model
/// is never told about web search or the canvas when they're disabled), and an
/// attachment root so images/PDFs are stored on disk rather than inlined.
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
            tools.get(harness_tools::CANVAS_TOOL).is_some(),
            workspace.root(),
        )),
        attachment_root: Some(workspace.root().to_path_buf()),
        // Context compression (off/audit/on) per the user's saved preference.
        compression: harness_runtime::compression::mode(),
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
        let status = Some(context_usage_line(agent, ui));
        let compression = compression_status_line(agent, ui);
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
        Command::Skills => print_skills(ui, ctx.workspace_root),
        Command::Auth(rest) => auth_cmd::handle_repl(rest, agent, ui, ctx.base_url)?,
        Command::Compression(rest) => switch_compression(rest, agent, ui)?,
        Command::Model(None) => {
            println!("  {} {}", ui.brown("oxen yoked:"), ui.cream(agent.model()))
        }
        Command::Model(Some(m)) => {
            agent.set_model(&m);
            // An id we've never seen (not in the cloud catalog, not an
            // installed local model) is saved as a custom catalog entry so it
            // shows up in the picker from now on — here and in the desktop.
            let known_cloud = harness_runtime::models::catalog().iter().any(|c| c.id == m);
            let known_local = harness_local::ModelStore::open()
                .map(|s| s.installed().iter().any(|l| l.id == m))
                .unwrap_or(false);
            if !known_cloud && !known_local {
                match harness_runtime::models::add(&m, "") {
                    Ok(_) => println!(
                        "  {} {}",
                        ui.dim("new model saved to the catalog:"),
                        ui.cream(&m)
                    ),
                    Err(e) => println!("  {} {e}", ui.dim("couldn't save to the catalog:")),
                }
            }
            // Persist the choice (clearing any local selection) so it's the
            // default next launch — here and in the desktop dropdown.
            let _ = harness_runtime::models::set_selected(&m);
            println!("  {} {}", ui.brown("fresh oxen yoked:"), ui.accent(&m));
        }
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

/// What to drive through the agent for one turn: a fresh prompt (the normal
/// case), or a continuation of the transcript's dangling last turn — `/retry`
/// after a failure, with no user message re-appended.
#[derive(Debug, Clone)]
pub(crate) enum TurnRequest {
    Prompt(String),
    Continue,
}

/// Whether the transcript stops mid-turn — it ends on a user message (the
/// reply never arrived: a provider error, no internet, a crash) or on a tool
/// result the model never got to react to. Such a session can be continued in
/// place with `/retry` (`Agent::continue_turn`).
pub(crate) fn ends_mid_turn(messages: &[harness_llm::ChatMessage]) -> bool {
    messages
        .last()
        .is_some_and(|m| m.role == "user" || m.role == "tool")
}

/// The one-line notice for a transient model-call failure being retried with
/// backoff — shared by the classic renderer and the live composer.
pub(crate) fn retry_notice(attempt: u32, max_attempts: u32, delay_ms: u64, error: &str) -> String {
    let secs = (delay_ms as f64 / 1000.0).ceil() as u64;
    format!(
        "{error} — retrying in {secs}s (attempt {} of {max_attempts})",
        attempt + 1
    )
}

/// The failure report for a turn that died even after retries: what happened,
/// then how to pick the conversation back up — now (`/retry`, `/model`) or
/// later (`--continue` / `--resume`). Auth failures get the `/auth` hint
/// instead of the generic recovery lines. Shared by the classic prompt and the
/// live composer so both terminals explain the same way out.
pub(crate) fn turn_failure_lines(
    agent: &Agent,
    ui: &Ui,
    err: &harness_agent::AgentError,
) -> Vec<String> {
    let mut lines = vec![
        format!("  {}", ui.red(&ui.death())),
        format!("  {}", ui.dim(&format!("The trail guide says: {err}"))),
    ];
    if let Some(hint) = auth_cmd::auth_hint(ui, &err.to_string()) {
        lines.push(hint);
        return lines;
    }
    lines.push(format!(
        "  {}",
        ui.dim("Nothing is lost — every step is saved in the trail journal.")
    ));
    lines.push(format!(
        "  {} {}",
        ui.dim("·"),
        ui.dim("/retry to try the turn again · /model <name> to switch oxen first"),
    ));
    lines.push(format!(
        "  {} {}",
        ui.dim("·"),
        ui.dim(&format!(
            "later: oxen-harness --continue (or --resume {}), then /retry",
            agent.session_id()
        )),
    ));
    lines
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

/// Whether the sticky-bottom live composer should drive this turn: only for an
/// interactive, animating TTY. Pipes, tests, `NO_COLOR`, and `TERM=dumb` fall
/// back to the classic blocking prompt with unchanged behavior.
fn live_enabled(ui: &Ui) -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && ui.animates()
}

/// Run one turn (a prompt or a `/retry` continuation), then auto-drain any
/// stacked messages in order. On a live TTY this hands off to the sticky
/// composer (which lets the user keep stacking while turns run); otherwise it
/// runs the classic prompt and drains after.
/// Returns `Ok(true)` when the session should end.
async fn run_turn_and_drain(
    agent: &mut Agent,
    request: TurnRequest,
    ui: &Ui,
    queue: &mut MessageQueue,
    carryover: &mut String,
) -> Result<bool> {
    if live_enabled(ui) {
        // The live composer hands back any half-typed next message so the idle
        // prompt can keep it instead of wiping it when the turn ends.
        let (exit, draft) = live::run_prompt(agent, request, ui, queue).await?;
        *carryover = draft;
        return Ok(exit);
    }
    if run_prompt(agent, &request, ui).await? {
        return Ok(true);
    }
    while !queue.is_empty() {
        let next = queue.pop_front().expect("queue is non-empty");
        println!(
            "  {} {}",
            ui.brown("▶ rolling the wagon:"),
            ui.cream(&truncate(&next, 80)),
        );
        if run_prompt(agent, &TurnRequest::Prompt(next), ui).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Run one turn, racing it against Ctrl-C. Returns `Ok(true)` if the user
/// interrupted the stream (the caller should then end the session).
async fn run_prompt(agent: &mut Agent, request: &TurnRequest, ui: &Ui) -> Result<bool> {
    let (text, attachments) = match request {
        TurnRequest::Prompt(prompt) => {
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
            (text, attachments)
        }
        // A retry re-drives the transcript as-is; there is no new message.
        TurnRequest::Continue => (String::new(), Vec::new()),
    };

    let renderer = Rc::new(RefCell::new(TurnRenderer::new(ui.clone())));
    renderer.borrow_mut().begin_thinking();

    let cb = renderer.clone();
    let mut on_event = move |event: &harness_agent::AgentEvent| cb.borrow_mut().on_event(event);
    let is_continue = matches!(request, TurnRequest::Continue);
    let result = tokio::select! {
        // Cancel the in-flight turn on Ctrl-C and signal an exit.
        _ = tokio::signal::ctrl_c() => {
            renderer.borrow_mut().finish();
            return Ok(true);
        }
        result = async {
            if is_continue {
                agent.continue_turn(&mut on_event).await
            } else {
                agent.run_turn_with_attachments(text, attachments, &mut on_event).await
            }
        } => result,
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
            println!();
            for line in turn_failure_lines(agent, ui, &e) {
                println!("{line}");
            }
            Ok(false)
        }
    }
}

/// A subtle trailer showing how full the model's context window is, set apart
/// from the turn's output by a blank line.
fn print_context_usage(agent: &Agent, ui: &Ui) {
    println!();
    println!("{}", context_usage_line(agent, ui));
}

/// The context-usage trailer as a (themed, indented) line — the current model
/// alongside how full its context window is. Shared by the classic prompt and
/// the live composer (which pins it just above the input divider).
pub(crate) fn context_usage_line(agent: &Agent, ui: &Ui) -> String {
    let used = agent.context_tokens();
    let window = agent.context_window();
    let pct = (used * 100).checked_div(window).map_or(0, |p| p.min(100));
    format!(
        "  {} {}",
        ui.dim(&format!(
            "🧭 context {} / {} tokens ({pct}%) ·",
            human_tokens(used),
            human_tokens(window),
        )),
        ui.accent(agent.model()),
    )
}

/// `/compression [off|audit|on]` — show or switch context compression. With no
/// argument, opens the interactive picker (current mode marked). Applies to
/// this live conversation immediately (`Agent::set_compression_mode`) and
/// persists the preference for new sessions — mirroring the desktop toggle.
fn switch_compression(rest: Option<String>, agent: &mut Agent, ui: &Ui) -> Result<()> {
    use harness_compress::CompressionMode;

    let current = agent.compression_mode();
    let choice = match rest {
        Some(arg) => arg,
        None => {
            let mark = |m: CompressionMode| if m == current { "  ← current" } else { "" };
            let options = [
                Choice::new(
                    "off",
                    format!(
                        "send every tool result untouched{}",
                        mark(CompressionMode::Off)
                    ),
                ),
                Choice::new(
                    "audit",
                    format!(
                        "measure what compression would save, change nothing{}",
                        mark(CompressionMode::Audit)
                    ),
                ),
                Choice::new(
                    "on",
                    format!(
                        "compress stale tool output (retrieve_original restores){}",
                        mark(CompressionMode::On)
                    ),
                ),
            ];
            match picker::select(
                ui,
                "Compression",
                &format!("Context compression is `{}` — switch it?", current.as_str()),
                &options,
                false,
            )? {
                Some(sel) => sel.into_iter().next().unwrap_or_default(),
                // Cancelled (or no interactive terminal) — leave it untouched.
                None => return Ok(()),
            }
        }
    };

    let mode = match choice.trim().to_ascii_lowercase().as_str() {
        "off" => CompressionMode::Off,
        "audit" => CompressionMode::Audit,
        "on" => CompressionMode::On,
        other => {
            println!(
                "  {} {}",
                ui.red("✗"),
                ui.dim(&format!(
                    "unknown mode `{other}` — expected off, audit, or on"
                )),
            );
            return Ok(());
        }
    };

    agent.set_compression_mode(mode);
    // Persist for future sessions too; failing to persist still leaves the
    // live session switched.
    let persisted = harness_runtime::compression::set_mode(mode);
    let scope = match persisted {
        Ok(()) => "for this chat and new sessions",
        Err(_) => "for this chat (couldn't persist the preference)",
    };
    println!(
        "  {} {}",
        ui.brown("⊙ compression:"),
        ui.cream(&format!("{} — {scope}", mode.as_str())),
    );
    Ok(())
}

/// The compression-savings trailer, pinned directly above the context meter:
/// the current mode leads (accented), then what compression saved (`on`) or
/// would have saved (`audit`) so far this session, then the `/compression`
/// hint so switching is discoverable from the meter itself. `None` with
/// compression off, so the row disappears entirely rather than showing a dead
/// `off` line.
pub(crate) fn compression_status_line(agent: &Agent, ui: &Ui) -> Option<String> {
    let mode = agent.compression_mode();
    if mode == harness_compress::CompressionMode::Off {
        return None;
    }
    let verb = if mode == harness_compress::CompressionMode::Audit {
        "would save"
    } else {
        "saved"
    };
    Some(format!(
        "  {} {} {} {}",
        ui.brown("⊙"),
        ui.dim("compression:"),
        ui.accent(mode.as_str()),
        ui.dim(&format!(
            "· {verb} ~{} tokens this session · /compression to switch",
            human_tokens(agent.tokens_saved()),
        )),
    ))
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
    use super::{ends_mid_turn, human_tokens, retry_notice};
    use harness_llm::ChatMessage;

    #[test]
    fn human_tokens_scales_units() {
        assert_eq!(human_tokens(980), "980");
        assert_eq!(human_tokens(12_300), "12.3k");
        assert_eq!(human_tokens(1_200_000), "1.2M");
    }

    #[test]
    fn ends_mid_turn_flags_dangling_user_and_tool_messages() {
        // Ends on a user message: the reply never arrived → retryable.
        let dangling_user = vec![ChatMessage::system("s"), ChatMessage::user("hi")];
        assert!(ends_mid_turn(&dangling_user));

        // Ends on a tool result the model never reacted to → retryable.
        let dangling_tool = vec![
            ChatMessage::user("hi"),
            ChatMessage::tool_result("t1", "output".to_string()),
        ];
        assert!(ends_mid_turn(&dangling_tool));

        // A settled conversation (assistant spoke last) has nothing to retry.
        let settled = vec![ChatMessage::user("hi"), ChatMessage::assistant("hello")];
        assert!(!ends_mid_turn(&settled));
        assert!(!ends_mid_turn(&[]));
    }

    #[test]
    fn retry_notice_reports_the_upcoming_attempt_and_wait() {
        let notice = retry_notice(1, 4, 2000, "Oxen API error (502): provider error");
        assert!(notice.contains("Oxen API error (502)"));
        assert!(notice.contains("retrying in 2s"));
        assert!(notice.contains("attempt 2 of 4"));
    }
}
