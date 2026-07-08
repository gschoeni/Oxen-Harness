//! `oxen-harness` — an interactive, streaming agentic coding REPL.
//!
//! This file is the entry point: parse the CLI surface ([`Args`] and the
//! `models`/`theme`/`loop`/`trace`/`oxen` subcommands), bootstrap a session
//! (endpoint, tools, store — see [`endpoint`]), and hand control to a REPL
//! driver in [`repl_loop`]. Turn execution lives in [`turn`], each `/command`
//! in its `*_cmd` module, and the live bottom-pinned composer in [`live`].

mod almanac;
mod ask;
mod attach;
mod auth_cmd;
mod brave;
mod canvas;
mod code_review_cmd;
mod compression_cmd;
mod diff;
mod endpoint;
mod fleet_sink;
mod fleet_ui;
mod live;
mod local;
mod loop_cmd;
mod markdown;
mod model_cmd;
mod oxen_cmd;
mod picker;
mod plan;
mod queue;
mod queue_cmd;
mod render;
mod repl;
mod repl_loop;
mod theme;
mod theme_cmd;
mod trace_cmd;
mod turn;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use harness_agent::Agent;
use harness_store::SessionMeta;
use harness_tools::Workspace;

use crate::endpoint::{agent_config, build_tool_registry, open_store, resolve_endpoint, Endpoint};
use crate::repl_loop::{run_box_repl, run_classic_repl, ReplContext};
use crate::theme::Ui;
use crate::turn::{ends_mid_turn, live_enabled};

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

    let mut tools = build_tool_registry(&workspace, &ui);
    let base_url = client.base_url().to_string();
    let config = agent_config(&model, context_window, &tools, &workspace);

    // The fleet: `spawn_agents` lets the model fan work out across parallel
    // subagents. The spawner snapshots the registry *before* the tool registers
    // (subagents get every tool except the fleet itself — one level deep), and
    // lanes render through the shared hub (the live composer's pinned block,
    // or an in-place painter in cooked mode). Prefs re-apply so a disabled
    // `spawn_agents` stays off.
    let spawner = Arc::new(harness_agent::FleetSpawner::new(
        client.clone(),
        tools.clone(),
        config.clone(),
    ));
    tools.register_typed(harness_agent::FleetTool::new(
        spawner,
        Arc::new(fleet_sink::CliFleetSink::new(ui.clone())),
    ));
    harness_runtime::tools::load().apply(&mut tools);

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
