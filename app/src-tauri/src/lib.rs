//! Tauri desktop bridge for oxen-harness.
//!
//! Exposes the agent loop to the web UI: the `run_turn` command drives
//! [`harness_agent::Agent`], emitting `agent://token` and `agent://tool` events
//! as the turn streams, and returning the assistant's final text. The agent is
//! initialized lazily on first use so the window always opens, even without an
//! API key configured.

use std::sync::Arc;

use harness_agent::{Agent, AgentConfig, AgentEvent};
use harness_core::DEFAULT_MODEL;
use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{ToolRegistry, Workspace};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

/// Lazily-initialized agent shared across commands.
#[derive(Default)]
pub struct AppState {
    agent: Mutex<Option<Agent>>,
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

fn build_agent() -> Result<Agent, String> {
    let workspace_root = std::env::current_dir().map_err(|e| e.to_string())?;
    let workspace = Workspace::new(&workspace_root).map_err(|e| e.to_string())?;
    let client = OxenClient::from_default_config().map_err(|e| e.to_string())?;
    let tools = ToolRegistry::default_for_workspace(workspace.clone());

    let dir = dirs_home()?.join(".oxen-harness");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let store = Arc::new(HistoryStore::open(dir.join("history.sqlite")).map_err(|e| e.to_string())?);
    let session = store
        .create_session(&SessionMeta {
            workspace: workspace.root().display().to_string(),
            model: DEFAULT_MODEL.to_string(),
        })
        .map_err(|e| e.to_string())?;

    Agent::new(client, tools, store, session, AgentConfig::default()).map_err(|e| e.to_string())
}

fn dirs_home() -> Result<std::path::PathBuf, String> {
    #[allow(deprecated)]
    std::env::home_dir().ok_or_else(|| "could not determine home directory".to_string())
}

/// Run one user turn, streaming events to the UI; returns the final text.
#[tauri::command]
async fn run_turn(app: AppHandle, state: State<'_, AppState>, prompt: String) -> Result<String, String> {
    let mut guard = state.agent.lock().await;
    if guard.is_none() {
        *guard = Some(build_agent()?);
    }
    let agent = guard.as_mut().expect("agent initialized above");

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
async fn session_info(state: State<'_, AppState>) -> Result<SessionInfo, String> {
    let mut guard = state.agent.lock().await;
    if guard.is_none() {
        *guard = Some(build_agent()?);
    }
    let agent = guard.as_ref().expect("agent initialized above");
    Ok(SessionInfo {
        model: agent.model().to_string(),
        workspace: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        session_id: agent.session_id().to_string(),
    })
}

/// Entry point shared by the binary and mobile targets.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![run_turn, session_info])
        .run(tauri::generate_context!())
        .expect("error while running oxen-harness desktop app");
}
