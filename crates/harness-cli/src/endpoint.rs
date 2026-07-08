//! Session bootstrap: resolve the inference endpoint (cloud or a local
//! llama-server), assemble the tool registry and agent config, and open the
//! history store. Everything `main` needs between parsing the CLI arguments
//! and constructing the [`Agent`](harness_agent::Agent).

use std::sync::Arc;

use anyhow::{Context, Result};
use harness_agent::AgentConfig;
use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{ToolRegistry, Workspace};

use crate::theme::Ui;
use crate::{ask, canvas, commands, local, Args};

/// The resolved inference endpoint for a session: the model, a client bound to
/// it, the context window to budget against (a local server's real size, else
/// derived from the model name), and — for `--local` — the llama-server process
/// to keep alive for the session's lifetime.
pub(crate) struct Endpoint {
    pub(crate) client: OxenClient,
    pub(crate) model: String,
    pub(crate) context_window: Option<usize>,
    pub(crate) local_server: Option<harness_local::LocalServer>,
}

/// Resolve which model to run and how to reach it.
///
/// `--local <id>` runs a model on this machine via llama.cpp; absent any
/// explicit choice we restore the last local model the user activated (in the
/// desktop dropdown or a prior `--local` run). Anything else connects to a
/// remote Oxen.ai-style endpoint. A *restored* (non-explicit) local model that
/// can't start here falls back to the cloud, while an explicit `--local` failure
/// — or an unreachable cloud endpoint — prints the death screen and exits.
pub(crate) async fn resolve_endpoint(
    args: &Args,
    resume_meta: Option<&SessionMeta>,
    ui: &Ui,
) -> Endpoint {
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
                match commands::auth::prompt_for_missing_key(ui, &base_url) {
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
pub(crate) fn build_tool_registry(workspace: &Workspace, ui: &Ui) -> ToolRegistry {
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

/// Register the `spawn_agents` fleet tool on a finished registry. The spawner
/// snapshots the registry *before* the tool registers — subagents get every
/// tool except the fleet itself (one fan-out level deep) — and lanes render
/// through the shared hub: the live composer's pinned block during interactive
/// turns, an in-place painter in cooked mode. Prefs re-apply afterward so a
/// user-disabled `spawn_agents` stays off.
pub(crate) fn register_fleet_tool(
    tools: &mut ToolRegistry,
    client: &harness_llm::OxenClient,
    config: &AgentConfig,
    ui: &Ui,
) {
    let spawner = Arc::new(harness_agent::FleetSpawner::new(
        client.clone(),
        tools.clone(),
        config.clone(),
    ));
    tools.register_typed(harness_agent::FleetTool::new(
        spawner,
        Arc::new(crate::fleet_sink::CliFleetSink::new(ui.clone())),
    ));
    harness_runtime::tools::load().apply(tools);
}

/// The agent configuration for a CLI session: model + window, a system prompt
/// gated on which tools actually survived the user's preferences (so the model
/// is never told about web search or the canvas when they're disabled), and an
/// attachment root so images/PDFs are stored on disk rather than inlined.
pub(crate) fn agent_config(
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

/// Open the SQLite history store at its standard `~/.oxen-harness` location.
pub(crate) fn open_store() -> Result<HistoryStore> {
    let path = harness_config::paths::history_db()
        .map_err(|e| anyhow::anyhow!("resolving history path: {e}"))?;
    HistoryStore::open(&path).with_context(|| format!("opening history at {}", path.display()))
}
