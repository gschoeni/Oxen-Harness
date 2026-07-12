//! Desktop bridge for the same saved, shareable verification loops as `/loop`
//! in the CLI. Management calls are deliberately small; `run_loop` owns the
//! session lock and cancellation token for the whole discover/verify cycle.

use harness_agent::AgentEvent;
use harness_loop::{LoopEvent, LoopJournal, LoopRunner, LoopSpec, LoopStore, LoopSummary};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::events::{TokenPayload, ToolEventPayload};
use crate::state::{agent_or_build, evict_idle, session_workspace, AppState};

#[derive(Serialize)]
pub(crate) struct LoopRunResult {
    succeeded: bool,
    iterations: u32,
    summary: String,
}

#[tauri::command]
pub(crate) fn list_loops() -> Result<Vec<LoopSummary>, String> {
    Ok(LoopStore::open().map_err(|e| e.to_string())?.list())
}

#[tauri::command]
pub(crate) fn loops_path() -> Result<String, String> {
    Ok(LoopStore::open()
        .map_err(|e| e.to_string())?
        .root()
        .display()
        .to_string())
}

#[tauri::command]
pub(crate) fn get_loop(name: String) -> Result<LoopSpec, String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .resolve(&name)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn save_loop(spec: LoopSpec) -> Result<(), String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .save(&spec)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn import_loop(path: String) -> Result<LoopSpec, String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .import(path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn export_loop(name: String, path: String) -> Result<String, String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .export(&name, path)
        .map(|p| p.display().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn remove_loop(name: String) -> Result<(), String> {
    LoopStore::open()
        .map_err(|e| e.to_string())?
        .remove(&name)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn run_loop(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
    name: Option<String>,
    goal: Option<String>,
) -> Result<LoopRunResult, String> {
    let store = LoopStore::open().map_err(|e| e.to_string())?;
    let spec = if let Some(goal) = goal.filter(|s| !s.trim().is_empty()) {
        LoopSpec::from_goal(goal)
    } else {
        store
            .resolve(name.as_deref().unwrap_or("default"))
            .map_err(|e| e.to_string())?
    };
    let arc = agent_or_build(&app, &state, &session).await?;
    let cancel = CancellationToken::new();
    {
        let mut cancels = state.cancels.lock().await;
        if cancels.contains_key(&session) {
            return Err("a turn is already running in this chat".to_string());
        }
        cancels.insert(session.clone(), cancel.clone());
    }
    let root = session_workspace(&session);
    let runner =
        LoopRunner::new(spec.clone(), root).persisting_to(store.journal_path_for(&spec.name));
    let sid = session.clone();
    let emitter = app.clone();
    let result = {
        let mut agent = arc.lock().await;
        agent.set_cancel_token(cancel);
        runner
            .run(&mut agent, |event| emit_loop_event(&emitter, &sid, event))
            .await
            .map(|journal| loop_result(&journal))
            .map_err(|e| e.to_string())
    };
    state.cancels.lock().await.remove(&session);
    evict_idle(&state).await;
    result
}

fn emit_loop_event(app: &AppHandle, session: &str, event: &LoopEvent) {
    match event {
        LoopEvent::Agent(AgentEvent::Token(token)) => {
            let _ = app.emit(
                "agent://token",
                TokenPayload {
                    session: session.to_string(),
                    token: token.clone(),
                },
            );
        }
        LoopEvent::Agent(AgentEvent::ToolStart { name, arguments }) => {
            let _ = app.emit(
                "agent://tool",
                ToolEventPayload {
                    session: session.to_string(),
                    phase: "start",
                    name: name.clone(),
                    detail: arguments.clone(),
                },
            );
        }
        LoopEvent::Agent(AgentEvent::ToolEnd { name, result }) => {
            let _ = app.emit(
                "agent://tool",
                ToolEventPayload {
                    session: session.to_string(),
                    phase: "end",
                    name: name.clone(),
                    detail: result.clone(),
                },
            );
        }
        _ => {}
    }
}

fn loop_result(journal: &LoopJournal) -> LoopRunResult {
    let succeeded = journal.succeeded();
    let iterations = journal.iterations();
    let summary = if succeeded {
        format!("Loop complete: all gates passed after {iterations} iteration(s).")
    } else {
        let reason = journal
            .stop
            .clone()
            .map(|s| s.headline())
            .unwrap_or_else(|| "stopped".into());
        format!("Loop stopped after {iterations} iteration(s): {reason}.")
    };
    LoopRunResult {
        succeeded,
        iterations,
        summary,
    }
}
