//! Tauri desktop bridge for oxen-harness.
//!
//! Exposes the agent loop to the web UI: the `run_turn` command drives
//! [`harness_agent::Agent`], emitting `agent://token` and `agent://tool` events
//! as the turn streams, and returning the assistant's final text. The agent is
//! initialized lazily on first use so the window always opens, even without an
//! API key configured.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use harness_agent::{Agent, AgentConfig, AgentEvent};
use harness_core::DEFAULT_MODEL;
use harness_llm::OxenClient;
use harness_local::{
    can_auto_install, install_hint, install_llama_server, llama_server_path, LocalServer,
    ModelStatus, ModelStore,
};
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{
    AskUserTool, Question, QuestionAnswer, QuestionAsker, ToolError, ToolRegistry, Workspace,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{oneshot, Mutex};

/// Outstanding `ask_user_question` prompts awaiting a UI answer, keyed by id.
type Pending = Arc<StdMutex<HashMap<String, oneshot::Sender<Vec<QuestionAnswer>>>>>;

/// Lazily-initialized agent shared across commands.
#[derive(Default)]
pub struct AppState {
    agent: Mutex<Option<Agent>>,
    /// A local `llama-server` kept alive while a local model is selected.
    local_server: Mutex<Option<LocalServer>>,
    /// Questions the agent is currently waiting on the user to answer.
    pending: Pending,
}

/// Bridges the agent's `ask_user_question` tool to the web UI: emits an
/// `agent://question` event and parks on a channel until `answer_question`
/// delivers the user's selection (or the channel is dropped → no answer).
struct TauriAsker {
    app: AppHandle,
    pending: Pending,
}

#[derive(Clone, Serialize)]
struct QuestionPayload {
    id: String,
    questions: Vec<Question>,
}

#[async_trait]
impl QuestionAsker for TauriAsker {
    async fn ask(&self, questions: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = format!("q{}", COUNTER.fetch_add(1, Ordering::Relaxed));

        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .insert(id.clone(), tx);

        self.app
            .emit(
                "agent://question",
                QuestionPayload {
                    id: id.clone(),
                    questions: questions.to_vec(),
                },
            )
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        match rx.await {
            Ok(answers) => Ok(Some(answers)),
            Err(_) => {
                self.pending
                    .lock()
                    .expect("pending mutex poisoned")
                    .remove(&id);
                Ok(None)
            }
        }
    }
}

#[derive(Clone, Serialize)]
struct ModelsView {
    models: Vec<ModelStatus>,
    total_disk_bytes: u64,
    dir: String,
    /// Whether `llama-server` is available to actually run a local model.
    llama_installed: bool,
    /// Whether the app can install `llama-server` for the user automatically.
    can_auto_install: bool,
    /// How to install `llama-server` when it's missing.
    install_hint: String,
}

#[derive(Clone, Serialize)]
struct DownloadEvent {
    id: String,
    downloaded: u64,
    total: Option<u64>,
    fraction: Option<f64>,
}

#[derive(Clone, Serialize)]
struct ToolEventPayload {
    phase: &'static str,
    name: String,
    detail: String,
}

#[derive(Clone, Serialize)]
struct SessionInfo {
    model: String,
    workspace: String,
    session_id: String,
}

fn build_agent(app: &AppHandle, pending: Pending) -> Result<Agent, String> {
    let client = OxenClient::from_default_config().map_err(|e| e.to_string())?;
    new_agent(app, pending, client, DEFAULT_MODEL)
}

/// Build an agent around an already-configured client, recording `model_label`
/// as the session's model and the request model.
fn new_agent(
    app: &AppHandle,
    pending: Pending,
    client: OxenClient,
    model_label: &str,
) -> Result<Agent, String> {
    let workspace_root = std::env::current_dir().map_err(|e| e.to_string())?;
    let workspace = Workspace::new(&workspace_root).map_err(|e| e.to_string())?;
    let mut tools = ToolRegistry::default_for_workspace(workspace.clone());
    tools.register(Arc::new(AskUserTool::new(Arc::new(TauriAsker {
        app: app.clone(),
        pending,
    }))));

    let dir = dirs_home()?.join(".oxen-harness");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let store =
        Arc::new(HistoryStore::open(dir.join("history.sqlite")).map_err(|e| e.to_string())?);
    let session = store
        .create_session(&SessionMeta {
            workspace: workspace.root().display().to_string(),
            model: model_label.to_string(),
        })
        .map_err(|e| e.to_string())?;

    let config = AgentConfig {
        model: model_label.to_string(),
        ..AgentConfig::default()
    };
    Agent::new(client, tools, store, session, config).map_err(|e| e.to_string())
}

fn dirs_home() -> Result<std::path::PathBuf, String> {
    #[allow(deprecated)]
    std::env::home_dir().ok_or_else(|| "could not determine home directory".to_string())
}

/// Lazily build the shared agent on first use, returning a mutable handle.
///
/// Initializing on demand (rather than at startup) means the window always
/// opens even when no API key is configured — the error surfaces on the first
/// command instead of blocking launch.
fn ensure_initialized<'a>(
    app: &AppHandle,
    pending: Pending,
    slot: &'a mut Option<Agent>,
) -> Result<&'a mut Agent, String> {
    if slot.is_none() {
        *slot = Some(build_agent(app, pending)?);
    }
    Ok(slot.as_mut().expect("agent initialized above"))
}

/// Run one user turn, streaming events to the UI; returns the final text.
#[tauri::command]
async fn run_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    prompt: String,
) -> Result<String, String> {
    let mut guard = state.agent.lock().await;
    let agent = ensure_initialized(&app, state.pending.clone(), &mut guard)?;

    agent
        .run_turn(prompt, |event| match event {
            AgentEvent::Token(t) => {
                let _ = app.emit("agent://token", t.clone());
            }
            AgentEvent::ToolStart { name, arguments } => {
                let _ = app.emit(
                    "agent://tool",
                    ToolEventPayload {
                        phase: "start",
                        name: name.clone(),
                        detail: arguments.clone(),
                    },
                );
            }
            AgentEvent::ToolEnd { name, result } => {
                let _ = app.emit(
                    "agent://tool",
                    ToolEventPayload {
                        phase: "end",
                        name: name.clone(),
                        detail: result.clone(),
                    },
                );
            }
        })
        .await
        .map_err(|e| e.to_string())
}

/// Report the current session info, initializing the agent if needed.
#[tauri::command]
async fn session_info(app: AppHandle, state: State<'_, AppState>) -> Result<SessionInfo, String> {
    let mut guard = state.agent.lock().await;
    let agent = ensure_initialized(&app, state.pending.clone(), &mut guard)?;
    Ok(SessionInfo {
        model: agent.model().to_string(),
        workspace: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        session_id: agent.session_id().to_string(),
    })
}

/// List local models with their on-disk status and total disk usage.
#[tauri::command]
async fn list_models() -> Result<ModelsView, String> {
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    Ok(ModelsView {
        models: store.statuses(),
        total_disk_bytes: store.total_disk_used(),
        dir: store.dir().display().to_string(),
        llama_installed: llama_server_path().is_some(),
        can_auto_install: can_auto_install(),
        install_hint: install_hint(),
    })
}

/// Install `llama-server` for the user, streaming progress via `llama://install`.
#[tauri::command]
async fn install_llama(app: AppHandle) -> Result<(), String> {
    install_llama_server(|line| {
        let _ = app.emit("llama://install", line.to_string());
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Download a model's weights, emitting `models://progress` as it streams.
#[tauri::command]
async fn pull_model(app: AppHandle, id: String) -> Result<(), String> {
    let spec = harness_local::find(&id).ok_or_else(|| format!("unknown model `{id}`"))?;
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    store
        .pull(spec, |p| {
            let _ = app.emit(
                "models://progress",
                DownloadEvent {
                    id: id.clone(),
                    downloaded: p.downloaded,
                    total: p.total,
                    fraction: p.fraction(),
                },
            );
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a downloaded model.
#[tauri::command]
async fn remove_model(id: String) -> Result<(), String> {
    let spec = harness_local::find(&id).ok_or_else(|| format!("unknown model `{id}`"))?;
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    store.remove(spec).map_err(|e| e.to_string())?;
    Ok(())
}

/// Switch the session to a downloaded local model: start `llama-server` and
/// rebuild the agent against it. The model must already be downloaded.
#[tauri::command]
async fn use_local_model(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<SessionInfo, String> {
    let spec = harness_local::find(&id).ok_or_else(|| format!("unknown model `{id}`"))?;
    if llama_server_path().is_none() {
        return Err(format!("llama-server isn't installed. {}", install_hint()));
    }
    let store = ModelStore::open().map_err(|e| e.to_string())?;
    if !store.is_installed(spec) {
        return Err(format!("{id} isn't downloaded yet — pull it first"));
    }

    let server = LocalServer::start(&store.path_for(spec), &id)
        .await
        .map_err(|e| e.to_string())?;
    let agent = new_agent(
        &app,
        state.pending.clone(),
        OxenClient::new(server.base_url(), "local", &id),
        &id,
    )?;
    let session_id = agent.session_id().to_string();

    // Swap in the new server + agent (dropping any previous local server).
    *state.local_server.lock().await = Some(server);
    *state.agent.lock().await = Some(agent);

    Ok(SessionInfo {
        model: id,
        workspace: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        session_id,
    })
}

// ===========================================================================
// Themes — list, switch, import/export, and vibe-code a new one via the model.
// ===========================================================================

fn theme_store() -> Result<harness_theme::Store, String> {
    harness_theme::Store::open().map_err(|e| e.to_string())
}

/// All available themes (built-in + installed), with the active one marked.
#[tauri::command]
async fn list_themes() -> Result<Vec<harness_theme::store::ThemeSummary>, String> {
    Ok(theme_store()?.list())
}

/// The full active theme (palette + voice) for the UI to apply.
#[tauri::command]
async fn active_theme() -> Result<harness_theme::Theme, String> {
    Ok(theme_store()?.load_active())
}

/// Switch the active theme; returns the resolved theme so the UI can re-skin.
#[tauri::command]
async fn use_theme(name: String) -> Result<harness_theme::Theme, String> {
    theme_store()?.set_active(&name).map_err(|e| e.to_string())
}

/// Install a theme from pasted/loaded TOML or JSON, then activate it.
#[tauri::command]
async fn import_theme(contents: String) -> Result<harness_theme::Theme, String> {
    let store = theme_store()?;
    let theme = store
        .install_from_str(&contents)
        .map_err(|e| e.to_string())?;
    store
        .set_active(&theme.meta.name)
        .map_err(|e| e.to_string())
}

/// Export a theme as a shareable TOML document.
#[tauri::command]
async fn export_theme(name: String) -> Result<String, String> {
    let theme = theme_store()?.resolve(&name).map_err(|e| e.to_string())?;
    theme.to_toml().map_err(|e| e.to_string())
}

/// Remove an installed theme (built-ins always remain).
#[tauri::command]
async fn remove_theme(name: String) -> Result<(), String> {
    theme_store()?.remove(&name).map_err(|e| e.to_string())
}

/// Vibe-code a new theme: send the brief to the model, parse its output, save
/// and activate it. Reuses the session's model + endpoint.
#[tauri::command]
async fn new_theme(
    app: AppHandle,
    state: State<'_, AppState>,
    brief: String,
) -> Result<harness_theme::Theme, String> {
    let raw = {
        let mut guard = state.agent.lock().await;
        let agent = ensure_initialized(&app, state.pending.clone(), &mut guard)?;
        agent
            .complete(&harness_theme::Theme::generation_system_prompt(), &brief)
            .await
            .map_err(|e| e.to_string())?
    };
    let theme = harness_theme::Theme::from_model_output(&raw).map_err(|e| e.to_string())?;
    let store = theme_store()?;
    store.save(&theme).map_err(|e| e.to_string())?;
    store
        .set_active(&theme.meta.name)
        .map_err(|e| e.to_string())
}

/// Deliver the user's answer to a pending `ask_user_question`, unblocking the
/// agent. Unknown ids are ignored (the question may have been cancelled).
#[tauri::command]
async fn answer_question(
    state: State<'_, AppState>,
    id: String,
    answers: Vec<QuestionAnswer>,
) -> Result<(), String> {
    let sender = state
        .pending
        .lock()
        .expect("pending mutex poisoned")
        .remove(&id);
    if let Some(tx) = sender {
        let _ = tx.send(answers);
    }
    Ok(())
}

/// Entry point shared by the binary and mobile targets.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            run_turn,
            session_info,
            list_models,
            install_llama,
            pull_model,
            remove_model,
            use_local_model,
            answer_question,
            list_themes,
            active_theme,
            use_theme,
            import_theme,
            export_theme,
            remove_theme,
            new_theme
        ])
        .run(tauri::generate_context!())
        .expect("error while running oxen-harness desktop app");
}
