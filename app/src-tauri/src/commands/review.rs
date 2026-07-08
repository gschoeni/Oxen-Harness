//! The code-review commands: run the configurable review pipeline against a
//! chat's workspace, and read/save/reset the pipeline configuration for the
//! Settings page. The pipeline itself lives in `harness_review`; this module
//! streams its progress to the webview and settles the findings into the
//! session as a normal exchange so follow-up turns can act on them.

use harness_agent::AgentEvent;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::bridges::emit_fleet_event;
use crate::commands::session::bump_total_tokens;
use crate::events::{
    CodeReviewResult, FleetStartedPayload, ReviewProgressPayload, ReviewTokenPayload,
    ReviewToolPayload, SessionPayload,
};
use crate::state::{agent_or_build, evict_idle, session_workspace, AppState};

/// Run the configurable code-review pipeline for a chat's workspace: uncommitted
/// changes by default, or PR-style against `base_branch`. Streams progress via
/// `review://progress` / `review://token` / `review://tool`, then injects the
/// findings into the session (as a settled user/assistant exchange) so follow-up
/// turns can act on them ("fix 1 and 3"). Holds the session's agent lock for the
/// duration, so it can't interleave with a running turn; `cancel_turn` stops it.
#[tauri::command]
pub(crate) async fn run_code_review(
    app: AppHandle,
    state: State<'_, AppState>,
    session: String,
    base_branch: Option<String>,
) -> Result<CodeReviewResult, String> {
    use harness_agent::fleet::FleetEvent;
    use harness_review::{ReviewError, ReviewEvent};

    let arc = agent_or_build(&app, &state, &session).await?;
    let root = session_workspace(&session);
    let target = match base_branch.filter(|b| !b.trim().is_empty()) {
        Some(branch) => harness_review::ReviewTarget::BaseBranch(branch.trim().to_string()),
        None => harness_review::ReviewTarget::Uncommitted,
    };

    let cancel = CancellationToken::new();
    {
        // Registering under the session's key is also the mutual-exclusion
        // check: an existing entry means a turn (or another review) is already
        // in flight, and overwriting its token would orphan its stop button.
        let mut cancels = state.cancels.lock().await;
        if cancels.contains_key(&session) {
            return Err("a turn is already running in this chat".to_string());
        }
        cancels.insert(session.clone(), cancel.clone());
    }

    let result = {
        let mut agent = arc.lock().await;
        let runner = harness_review::ReviewRunner::new(
            harness_review::ReviewConfig::load(),
            target.clone(),
            &root,
        )
        .with_cancel(cancel);
        let sid = session.clone();
        let emitter = app.clone();
        let mut pipeline_tokens = 0usize;
        let run = runner
            .run(&agent, |event| match event {
                ReviewEvent::StepStarted {
                    index,
                    total,
                    name,
                    agents,
                } => {
                    let _ = emitter.emit(
                        "review://progress",
                        ReviewProgressPayload {
                            session: sid.clone(),
                            step: name.clone(),
                            index: *index,
                            total: *total,
                            agents: agents.clone(),
                        },
                    );
                    // A fan-out step opens a lanes panel like spawn_agents does.
                    if agents.len() > 1 {
                        let _ = emitter.emit(
                            "fleet://started",
                            FleetStartedPayload {
                                session: sid.clone(),
                                agents: agents.clone(),
                                source: "review",
                            },
                        );
                    }
                }
                ReviewEvent::Agent(AgentEvent::Token(t)) => {
                    let _ = emitter.emit(
                        "review://token",
                        ReviewTokenPayload {
                            session: sid.clone(),
                            token: t.clone(),
                        },
                    );
                }
                ReviewEvent::Agent(AgentEvent::ToolStart { name, .. }) => {
                    let _ = emitter.emit(
                        "review://tool",
                        ReviewToolPayload {
                            session: sid.clone(),
                            name: name.clone(),
                        },
                    );
                }
                // A fan-out step's lanes ARE a fleet; map the review's
                // subagent events to FleetEvent and route them through the one
                // fleet emitter, so review lanes and spawn_agents lanes can't
                // drift on the wire format.
                ReviewEvent::SubagentStarted { agent, name } => emit_fleet_event(
                    &emitter,
                    &sid,
                    &FleetEvent::TaskStarted {
                        index: *agent,
                        label: name.clone(),
                    },
                ),
                ReviewEvent::Subagent { agent, event } => emit_fleet_event(
                    &emitter,
                    &sid,
                    &FleetEvent::Agent {
                        index: *agent,
                        event: event.clone(),
                    },
                ),
                ReviewEvent::SubagentCompleted {
                    agent,
                    name,
                    ok,
                    tokens_used,
                    summary,
                } => emit_fleet_event(
                    &emitter,
                    &sid,
                    &FleetEvent::TaskCompleted {
                        index: *agent,
                        label: name.clone(),
                        ok: *ok,
                        tokens_used: *tokens_used,
                        summary: summary.clone(),
                    },
                ),
                ReviewEvent::StepCompleted { .. } => {
                    let _ = emitter.emit(
                        "fleet://completed",
                        SessionPayload {
                            session: sid.clone(),
                        },
                    );
                }
                ReviewEvent::Completed { tokens_used, .. } => pipeline_tokens = *tokens_used,
                _ => {}
            })
            .await;
        match run {
            Ok(report) => {
                let (user, assistant) = harness_review::session_exchange(&target, &report);
                agent
                    .inject_exchange(user.clone(), assistant.clone())
                    .map_err(|e| e.to_string())?;
                Ok(CodeReviewResult {
                    status: "ok",
                    user,
                    assistant,
                    findings: report.findings.len(),
                    tokens_used: pipeline_tokens,
                })
            }
            Err(ReviewError::NothingToReview) => Ok(CodeReviewResult {
                status: "nothing",
                user: String::new(),
                assistant: String::new(),
                findings: 0,
                tokens_used: 0,
            }),
            Err(ReviewError::Cancelled { tokens_used }) => Ok(CodeReviewResult {
                status: "cancelled",
                user: String::new(),
                assistant: String::new(),
                findings: 0,
                // Reviewers that ran before the stop spent real tokens; report
                // them so the all-time counter below doesn't undercount.
                tokens_used,
            }),
            Err(e) => Err(e.to_string()),
        }
    };
    state.cancels.lock().await.remove(&session);
    // Reviewer agents are real spend: roll their tokens into the all-time
    // total (the same counter turns feed).
    if let Ok(res) = &result {
        let _ = bump_total_tokens(res.tokens_used);
    }
    evict_idle(&state).await;
    result
}

/// The saved code-review pipeline (steps + findings cap) for the Settings page.
#[tauri::command]
pub(crate) fn get_code_review_config() -> harness_review::ReviewConfig {
    harness_review::ReviewConfig::load()
}

/// Persist the code-review pipeline and snapshot the versioned config repo.
/// Applies to the next review — a running one keeps the steps it started with.
#[tauri::command]
pub(crate) fn save_code_review_config(config: harness_review::ReviewConfig) -> Result<(), String> {
    config.save().map_err(|e| e.to_string())?;
    harness_runtime::config_repo::snapshot("Update code review settings");
    Ok(())
}

/// The built-in default pipeline, for the Settings page's "reset to defaults".
#[tauri::command]
pub(crate) fn default_code_review_config() -> harness_review::ReviewConfig {
    harness_review::ReviewConfig::default()
}
