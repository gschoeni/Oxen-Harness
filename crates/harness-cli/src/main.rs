//! `oxen-harness` — an interactive, streaming agentic coding REPL.
//!
//! This file is the entry point: parse the CLI surface ([`Args`] and the
//! `models`/`theme`/`loop`/`trace`/`oxen` subcommands), bootstrap a session
//! (endpoint, tools, store — see [`endpoint`]), and hand control to a REPL
//! driver in [`repl_loop`]. Turn execution lives in [`turn`], each `/command`
//! in its [`commands`] module, and the live bottom-pinned composer in [`live`].

mod almanac;
mod ansi;
mod approve;
mod ask;
mod attach;
mod brave;
mod canvas;
mod commands;
mod diff;
mod endpoint;
mod event_lines;
mod fleet_sink;
mod fleet_ui;
mod highlight;
mod interrupt;
mod live;
mod local;
mod markdown;
mod media;
mod picker;
mod plan;
mod preview;
mod pricing;
mod queue;
mod render;
mod repl;
mod repl_loop;
mod theme;
mod training;
mod turn;
mod width;

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
    /// catalog id (e.g. qwen3-8b) or any downloaded model's id. Anything
    /// already on disk starts fully offline; a catalog model is downloaded
    /// first if needed. See `oxen-harness models list`.
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
        action: commands::theme::ThemeAction,
    },
    /// Run and manage self-verifying agent loops (run / list / new / show ...).
    Loop {
        #[command(subcommand)]
        action: commands::loops::LoopAction,
    },
    /// Export or share a conversation as an Oxen repo (transcript + attachments).
    Trace {
        #[command(subcommand)]
        action: commands::trace::TraceAction,
    },
    /// Version your harness config (~/.oxen-harness) with Oxen (init/snapshot/status).
    Oxen {
        #[command(subcommand)]
        action: commands::oxen::OxenAction,
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

    // Surface a crash from the previous run — a fatal signal leaves no other
    // trace — then arm the handler for this one. Check-before-install, so the
    // old marker is read before it could be clobbered.
    if let Ok(marker) = harness_config::paths::last_crash_file() {
        if let Some(signal) = harness_crash::arm(&marker) {
            let log = harness_config::paths::errors_log().ok();
            harness_agent::errlog::record(
                log.as_deref(),
                "crashed",
                serde_json::json!({ "signal": signal }),
            );
            eprintln!(
                "note: the previous oxen-harness run crashed ({signal}) — \
                 details, if any, are in ~/.oxen-harness/errors.jsonl"
            );
        }
    }

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
        Some(TopCommand::Theme { action }) => return commands::theme::run_theme(action, &ui).await,
        Some(TopCommand::Loop { action }) => {
            match commands::loops::handle_cli(action, &ui).await? {
                commands::loops::Dispatch::Done => return Ok(()),
                commands::loops::Dispatch::Run(spec) => pending_loop = Some(*spec),
            }
        }
        Some(TopCommand::Trace { action }) => return commands::trace::run_trace(action, &ui),
        Some(TopCommand::Oxen { action }) => return commands::oxen::run_oxen(action, &ui),
        None => {}
    }

    let store = Arc::new(open_store()?);

    // `--continue` is `--resume` pointed at the newest *native* session on
    // record. Imported transcripts (Claude Code / Cursor) share the store but
    // are review-only — they must never resume as a live agent.
    if args.continue_last {
        match store
            .list_sessions()?
            .into_iter()
            .find(|s| s.source.is_empty())
        {
            Some(latest) => args.resume = Some(latest.id),
            None => {
                eprintln!(
                    "\n{}",
                    ui.red("No previous expedition to continue — set out fresh.")
                );
                std::process::exit(1);
            }
        }
    } else if let Some(id) = &args.resume {
        // An explicitly named imported session gets a clear refusal instead of
        // silently replaying a foreign transcript with its workspace and model.
        let imported = store
            .list_sessions()?
            .into_iter()
            .find(|s| &s.id == id)
            .is_some_and(|s| !s.source.is_empty());
        if imported {
            eprintln!(
                "\n{}",
                ui.red("That session was imported for training-data review and can't be resumed.")
            );
            std::process::exit(1);
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
    let config = agent_config(&model, context_window, &tools, &workspace, &ui);

    endpoint::register_fleet_tool(&mut tools, &client, &config, store.clone(), &ui);

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

    // The opening banner shows the all-time grand total spend across every model
    // and project (best-effort; unavailable reads as `None` → "—"). Per-turn
    // usage is priced and recorded as the session runs (see `handle_line`).
    let cost_usd = commands::usage::total_cost_usd(&store).await;
    let total_tokens = commands::usage::total_tokens(&store);

    print!(
        "{}",
        theme::banner(
            &ui,
            &base_url,
            &model,
            &workspace.root().display().to_string(),
            &session,
            total_tokens,
            cost_usd,
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
                ui.dim(
                    "this expedition stopped mid-turn — /retry is pre-filled, \
                     press ⏎ to pick up where it left off"
                ),
            );
        }
        println!();
    }

    // Shared per-run context (store, pricing cache, session) used by both the
    // one-shot loop path and the interactive REPL.
    let ctx = ReplContext {
        store: &store,
        session: &session,
        workspace_root: workspace.root(),
    };

    // `oxen-harness loop run ...`: run the loop once, then exit (no REPL).
    // Dev servers the loop started must be stopped on *both* paths — the
    // manager is a process-wide static, so nothing drops them for us.
    if let Some(spec) = pending_loop {
        let outcome = commands::loops::run(spec, &mut agent, &ui, workspace.root()).await;
        preview::shutdown().await;
        outcome?;
        return Ok(());
    }

    // Warm the pricing cache before the REPL paints its first context trailer,
    // so the per-token rate is visible next to the model up front — before the
    // session has spent a single token.
    pricing::warm_for(agent.model()).await;

    // Interactive TTYs use the bordered, bottom-pinned box composer (multi-line,
    // history, queue) for both idle and in-turn input. Pipes, dumb terminals, and
    // an explicit `OXEN_HARNESS_CLASSIC_INPUT` fall back to the classic readline.
    let use_box = live_enabled(&ui) && std::env::var_os("OXEN_HARNESS_CLASSIC_INPUT").is_none();
    let result = if use_box {
        run_box_repl(&mut agent, &mut ui, &ctx).await
    } else {
        run_classic_repl(&mut agent, &mut ui, &ctx).await
    };

    // On the way out, offer to label the run for the training-data export —
    // while the user still remembers whether it was a good one.
    training::prompt_session_review(&store, &session, &agent, &ui);
    // Dev servers live in a process-wide manager (nothing drops it): stop them
    // explicitly so an `npm run dev` never outlives the expedition.
    preview::shutdown().await;
    result
}
