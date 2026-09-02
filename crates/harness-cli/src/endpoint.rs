//! Session bootstrap: resolve the inference endpoint (cloud or a local
//! llama-server), assemble the tool registry and agent config, and open the
//! history store. Everything `main` needs between parsing the CLI arguments
//! and constructing the [`Agent`](harness_agent::Agent).

use std::io::IsTerminal;
use std::sync::Arc;

use anyhow::{Context, Result};
use harness_agent::AgentConfig;
use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{ToolRegistry, Workspace};

use crate::picker::{self, Choice};
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
///
/// `interactive` is false for the runs nobody is watching (`-p`, `loop run`):
/// they never get the first-run model pick, however empty the config is.
pub(crate) async fn resolve_endpoint(
    args: &Args,
    resume_meta: Option<&SessionMeta>,
    interactive: bool,
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

        // Precedence for the base URL: --base-url > --host > the saved host
        // override (connection.json, set via `/auth host` or the desktop
        // Settings) > env (OXEN_BASE_URL / OXEN_HOST) > default Oxen.ai
        // endpoint. The last three are `effective_base_url`'s resolution — the
        // same one the desktop uses, so both hosts reach the same place.
        let base_url =
            args.base_url
                .clone()
                .or_else(|| args.host.as_deref().map(harness_llm::base_url_from_host))
                .unwrap_or_else(|| {
                    harness_runtime::connection::effective_base_url(
                        &harness_runtime::connection::load(),
                    )
                });
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
        // The catalog-reported context window when a prior fetch has cached
        // it; `None` falls back to the name-derived table in `harness-agent`.
        let context_window = harness_local::limits::context_window(&model);
        Endpoint {
            client,
            model,
            context_window,
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
        return first_run_pick(cloud(ui), args, interactive, ui).await;
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
            first_run_pick(cloud(ui), args, interactive, ui).await
        }
    }
}

/// Whether the command line leaves the model open to a first-run pick: no
/// `--model`, no resumed session (which carries its own), and no `--local`.
fn model_is_unspecified(args: &Args) -> bool {
    args.model.is_none() && args.resume.is_none() && !args.continue_last && args.local.is_none()
}

/// Whether anything is persisted from a previous run — a selected cloud model,
/// a curated catalog, or an activated local model. Anything at all here means
/// the user has chosen before and is never asked again.
fn nothing_persisted() -> bool {
    let cfg = harness_runtime::models::load();
    cfg.selected.trim().is_empty() && cfg.custom.is_empty() && cfg.active_local.trim().is_empty()
}

/// How many models the first-run card may offer: enough to browse, never
/// taller than the terminal — the picker redraws by line count, so a card that
/// doesn't fit on screen smears stale rows into the scrollback. The endpoint
/// lists dozens of models; `/model` is where you see them all.
fn first_run_capacity() -> usize {
    let rows = crossterm::terminal::size()
        .map(|(_, h)| h as usize)
        .unwrap_or(24);
    rows.saturating_sub(12).clamp(4, 10)
}

/// The endpoint's chat models as picker rows, recommended one first, capped at
/// `cap` rows.
///
/// Image and audio endpoints are dropped (a coding harness can't ride them),
/// and [`harness_core::DEFAULT_MODEL`] leads the list so the cursor opens on
/// the model this run would otherwise have used anyway.
fn first_run_rows(hits: Vec<harness_local::source::OxenModelHit>, cap: usize) -> Vec<Choice> {
    let mut chat: Vec<harness_local::source::OxenModelHit> = hits
        .into_iter()
        .filter(|h| h.endpoint.is_empty() || h.endpoint.contains("chat/completions"))
        .filter(|h| h.outputs.is_empty() || h.outputs.iter().any(|o| o == "text"))
        .collect();
    chat.sort_by_key(|h| h.id != harness_core::DEFAULT_MODEL);
    chat.truncate(cap);
    chat.into_iter()
        .map(|h| {
            let recommended = if h.id == harness_core::DEFAULT_MODEL {
                " ← recommended"
            } else {
                ""
            };
            let price = h
                .pricing
                .as_ref()
                .and_then(crate::pricing::format_rate)
                .map(|r| format!(" · {r}"))
                .unwrap_or_default();
            let maker = if h.developer.is_empty() {
                String::new()
            } else {
                format!(" · {}", h.developer)
            };
            Choice::new(h.id, format!("{}{maker}{price}{recommended}", h.name))
        })
        .collect()
}

/// A first run with nothing chosen yet asks, once, which model to ride —
/// instead of silently defaulting and leaving the choice buried in `/model`.
///
/// Runs *after* the endpoint (and so the API key) resolves, since the catalog
/// is fetched from that endpoint. Anything that goes wrong — no catalog, an
/// empty one, a cancelled picker — quietly keeps today's default.
async fn first_run_pick(endpoint: Endpoint, args: &Args, interactive: bool, ui: &Ui) -> Endpoint {
    if !interactive
        || !model_is_unspecified(args)
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || !nothing_persisted()
    {
        return endpoint;
    }
    let base_url = endpoint.client.base_url().to_string();
    let token = harness_runtime::connection::effective_api_key(&base_url);
    let hits = harness_local::source::oxen_search_models(
        &base_url,
        "",
        (!token.trim().is_empty()).then_some(token.as_str()),
    )
    .await
    .unwrap_or_default();
    let options = first_run_rows(hits, first_run_capacity());
    let picked = picker::select(
        ui,
        "Model",
        "First trail out — which model should pull the wagon? \
         (or type any model id; /model lists them all later)",
        &options,
        false,
    )
    .ok()
    .flatten()
    .and_then(|sel| sel.into_iter().next())
    .map(|id| id.trim().to_string())
    .filter(|id| !id.is_empty());
    let Some(model) = picked else {
        return endpoint;
    };
    // Persist it as the catalog entry *and* the selection, so the next launch
    // (here and in the desktop dropdown) never asks again.
    let name = options
        .iter()
        .find(|o| o.label == model)
        .map(|o| o.description.split(" · ").next().unwrap_or_default())
        .unwrap_or_default()
        .trim()
        .to_string();
    let _ = harness_runtime::models::add(&model, &name);
    let _ = harness_runtime::models::set_selected(&model);
    println!("  {} {}", ui.brown("🐂 yoked to"), ui.accent(&model),);
    println!("  {}", ui.dim("change it any time with /model"));
    Endpoint {
        client: endpoint.client.with_model(&model),
        context_window: harness_local::limits::context_window(&model),
        model,
        local_server: endpoint.local_server,
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
    // `open_file` (the desktop's file-viewer panel) is deliberately NOT
    // registered: the terminal has no viewer surface, and a host-surface tool
    // that can't surface anything would just mislead the model (the prompt
    // gates on registration, so the CLI's model never hears about it).
    // Dev servers for live preview: the terminal can't embed a browser, so the
    // trio registers with a status-remembering sink and `/preview` opens the
    // app in the user's browser. (No screenshot/console sight tools here.)
    let (start_server, stop_server, server_logs) = harness_preview::session_tools(
        crate::preview::manager(),
        crate::preview::SESSION_KEY,
        workspace.root(),
        Arc::new(crate::preview::CliPreviewSink),
    );
    tools.register_typed(start_server.with_verify_hint(
        "The user can open the running app in their browser with /preview — \
         mention that. Check dev_server_logs after exercising the app.",
    ));
    tools.register_typed(stop_server);
    tools.register_typed(server_logs);
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

/// The session's fleet spawner, kept reachable so `/model` and `/login` can
/// point future subagents at a swapped model/client (see [`update_fleet_endpoint`]).
/// A process-wide slot is honest here: the CLI runs exactly one session, one
/// agent, one spawner — the same reasoning behind [`crate::fleet_ui::FleetHub::global`].
static FLEET_SPAWNER: std::sync::OnceLock<Arc<harness_agent::FleetSpawner>> =
    std::sync::OnceLock::new();

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
    workspace: &Workspace,
    usage_store: Arc<HistoryStore>,
    ui: &Ui,
) {
    let spawner = Arc::new(
        harness_agent::FleetSpawner::new(client.clone(), tools.clone(), config.clone())
            .with_workspace(workspace.root())
            .with_usage_store(usage_store),
    );
    // Keep a handle so a later model/endpoint swap reaches future subagents;
    // set_or_ignore, since a session registers exactly once.
    let _ = FLEET_SPAWNER.set(spawner.clone());
    tools.register_typed(
        harness_agent::FleetTool::new(
            spawner,
            Arc::new(crate::fleet_sink::CliFleetSink::new(ui.clone())),
        )
        // A `wait: false` fleet leaves its report here for the next round.
        .with_asides(tools.asides()),
    );
    harness_runtime::tools::load().apply(tools);
}

/// Point the session's fleet spawner at a swapped client and/or model, so a
/// `spawn_agents` fleet launched after a `/model` or `/login` runs on the new
/// endpoint rather than the one captured when the tool was registered. A no-op
/// when the fleet tool isn't registered (the user disabled it).
pub(crate) fn update_fleet_endpoint(client: Option<&OxenClient>, model: Option<&str>) {
    let Some(spawner) = FLEET_SPAWNER.get() else {
        return;
    };
    if let Some(client) = client {
        spawner.set_client(client.clone());
    }
    if let Some(model) = model {
        spawner.set_model(model);
    }
}

/// The agent configuration for a CLI session: model + window, a system prompt
/// gated on which tools actually survived the user's preferences (so the model
/// is never told about web search or the canvas when they're disabled), an
/// attachment root so images/PDFs are stored on disk rather than inlined, and
/// the permission gate wired to the terminal approval prompt.
pub(crate) fn agent_config(
    model: &str,
    context_window: Option<usize>,
    tools: &ToolRegistry,
    workspace: &Workspace,
    ui: &Ui,
) -> AgentConfig {
    let conventions = harness_runtime::context_files::discover(workspace.root());
    // Path-scoped conventions stay out of the prompt; the fs tools surface
    // them the first time the model touches a file they govern.
    if let Some(files) = tools.files() {
        files.set_rules(conventions.path_rules());
    }
    let system_prompt = format!(
        "{}{}{}",
        harness_agent::system_prompt_with_env(
            harness_agent::OptionalTools::from_registry(tools),
            workspace.root(),
        ),
        // The repository's own conventions (AGENTS.md and friends) before the
        // user's project metadata: general to specific, most authoritative last.
        conventions.prompt_section(),
        harness_runtime::project::prompt_section(workspace.root())
    );
    let limits = harness_runtime::limits::load();
    AgentConfig {
        model: model.to_string(),
        context_window,
        // The catalog-reported reply ceiling, when a fetch has cached it
        // (misses for local aliases — they fall back to the reserve).
        max_output_tokens: harness_local::limits::max_output_tokens(model),
        system_prompt: Some(system_prompt),
        attachment_root: Some(workspace.root().to_path_buf()),
        initial_attachments: harness_runtime::project::binary_context_paths(workspace.root()),
        // Context compression (off/audit/on) per the user's saved preference.
        compression: harness_runtime::compression::mode(),
        // Retry attempts and failed turns append to ~/.oxen-harness/errors.jsonl
        // so a developer can dig into what the endpoint said later.
        error_log: harness_config::paths::errors_log().ok(),
        // Every model call appends its size, cache-prefix classification,
        // latency, and reported usage to ~/.oxen-harness/requests.jsonl.
        request_log: harness_config::paths::requests_log().ok(),
        // Spend limits + summary routing from ~/.oxen-harness/limits.json.
        budget: limits
            .max_session_tokens
            .map(harness_agent::SessionBudget::new),
        // Route the work that doesn't need the session model: compaction
        // summaries and fleet/review lanes.
        roles: harness_agent::ModelRoles {
            smol: limits.smol_model,
            summary: limits.summary_model,
        },
        // A model that keeps failing hands the call to the next one rather
        // than ending the turn.
        retry: harness_agent::RetryPolicy {
            fallback_models: limits.fallback_models,
            ..Default::default()
        },
        // Gate tool calls behind the permission layer, with approval prompts
        // rendered through the terminal picker. Fleet/review subagents get the
        // gate's auto-deny form automatically (see AgentConfig::permissions).
        permissions: Some(Arc::new(harness_permissions::PermissionGate::new(
            workspace.root(),
            Arc::new(crate::approve::CliApprover::new(ui.clone())),
        ))),
        ..AgentConfig::default()
    }
}

/// The workspace's stream rules, compiled. A rule whose pattern no longer
/// compiles is reported and skipped rather than taking the session down —
/// `harness_agent::rules::compile_all` owns that policy for both front ends.
pub(crate) fn stream_rules(workspace_root: &std::path::Path) -> harness_agent::rules::RuleSet {
    let specs = harness_runtime::rules::load(workspace_root);
    let (rules, skipped) = harness_agent::rules::compile_all(specs.iter().map(|s| s.parts()));
    for note in skipped {
        eprintln!("  skipping stream rule {note}");
    }
    rules
}

/// Open the SQLite history store at its standard `~/.oxen-harness` location.
pub(crate) fn open_store() -> Result<HistoryStore> {
    let path = harness_config::paths::history_db()
        .map_err(|e| anyhow::anyhow!("resolving history path: {e}"))?;
    HistoryStore::open(&path).with_context(|| format!("opening history at {}", path.display()))
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use harness_local::source::{ModelPricing, OxenModelHit};

    fn args(argv: &[&str]) -> Args {
        Args::try_parse_from(argv).expect("valid arguments")
    }

    fn hit(id: &str, endpoint: &str, outputs: &[&str]) -> OxenModelHit {
        OxenModelHit {
            id: id.to_string(),
            name: id.to_string(),
            developer: "Anthropic".into(),
            summary: String::new(),
            description: String::new(),
            endpoint: endpoint.to_string(),
            pricing: Some(ModelPricing {
                input_cost_per_token: 0.000_003,
                output_cost_per_token: 0.000_015,
            }),
            inputs: vec!["text".into()],
            outputs: outputs.iter().map(|o| o.to_string()).collect(),
            context_length: None,
            max_output_tokens: None,
        }
    }

    #[test]
    fn only_a_run_that_names_no_model_may_be_asked() {
        assert!(model_is_unspecified(&args(&["oxen-harness"])));
        // Anything that already settles the model answers the question for us.
        assert!(!model_is_unspecified(&args(&[
            "oxen-harness",
            "--model",
            "claude-sonnet-5"
        ])));
        assert!(!model_is_unspecified(&args(&[
            "oxen-harness",
            "--resume",
            "8f3c"
        ])));
        assert!(!model_is_unspecified(&args(&["oxen-harness", "-c"])));
        assert!(!model_is_unspecified(&args(&[
            "oxen-harness",
            "--local",
            "qwen3-8b"
        ])));
    }

    #[test]
    fn first_run_rows_lead_with_the_recommended_chat_model() {
        let rows = first_run_rows(
            vec![
                hit("some-other-model", "/chat/completions", &["text"]),
                hit("an-image-model", "/images/generate", &["image"]),
                hit(harness_core::DEFAULT_MODEL, "/chat/completions", &["text"]),
            ],
            8,
        );
        // Image endpoints are dropped — a coding harness can't ride them.
        assert_eq!(rows.len(), 2);
        // The cursor opens on the recommended model, which leads and says so.
        assert_eq!(rows[0].label, harness_core::DEFAULT_MODEL);
        assert!(rows[0].description.ends_with(" ← recommended"));
        assert!(rows[0].description.contains("$3/M in"));
        assert_eq!(rows[1].label, "some-other-model");
        assert!(!rows[1].description.contains("recommended"));
    }

    #[test]
    fn the_card_is_capped_and_keeps_the_recommendation() {
        // Dozens of models would draw a card taller than the terminal, and the
        // picker's redraw counts lines — so the list is capped, recommendation
        // first. `/model` is where the full catalog lives.
        let mut hits: Vec<OxenModelHit> = (0..40)
            .map(|i| hit(&format!("model-{i}"), "/chat/completions", &["text"]))
            .collect();
        hits.push(hit(
            harness_core::DEFAULT_MODEL,
            "/chat/completions",
            &["text"],
        ));
        let rows = first_run_rows(hits, 6);
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0].label, harness_core::DEFAULT_MODEL);
    }

    #[test]
    fn a_catalog_that_omits_endpoints_and_modalities_still_lists() {
        // Self-hosted endpoints may report neither field; assuming "chat" is
        // the useful default — an empty picker would just be a dead end.
        let rows = first_run_rows(vec![hit("mystery-model", "", &[])], 8);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "mystery-model");
    }
}
