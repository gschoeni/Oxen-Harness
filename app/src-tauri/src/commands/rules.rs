//! Stream rules — the Settings → Rules page.
//!
//! A rule watches the model's output and corrects it when a pattern matches
//! (see `harness_agent::rules`). These commands are the webview's entry
//! points: read the user's rules and the repository's, save the user's, and
//! check a pattern against a sample.
//!
//! That last one matters more than it looks. The editor must not test patterns
//! with the browser's own regex: JavaScript accepts constructs this engine
//! doesn't (lookahead, backreferences), so a browser-side preview would call a
//! pattern fine and the rule would then silently never fire. Every check goes
//! through the same engine the agent runs.

use harness_llm::stream::StreamEvent;
use harness_llm::types::ChatMessage;
use harness_llm::ChatRequest;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

/// Tokens from a rule being written, so the editor can show the model working
/// rather than a spinner. One channel — only one draft runs at a time.
const DRAFT_CHANNEL: &str = "rules://draft";

/// Both sets of rules for the active project: the user's own, and the ones
/// committed to this repository.
#[derive(serde::Serialize)]
pub(crate) struct RuleSets {
    user: Vec<harness_runtime::rules::RuleSpec>,
    project: Vec<harness_runtime::rules::RuleSpec>,
    /// Where the project's rules live, for the UI to name.
    project_path: String,
}

#[tauri::command]
pub(crate) async fn list_rules(state: State<'_, AppState>) -> Result<RuleSets, String> {
    let root = state.active_root().await;
    Ok(RuleSets {
        user: harness_runtime::rules::user_rules().rules,
        project: harness_runtime::rules::project_rules(&root).rules,
        project_path: harness_runtime::rules::PROJECT_RULES_FILE.to_string(),
    })
}

/// Replace the user's rules. The page always sends the whole set, so a
/// reorder, an edit, and a delete are one code path.
#[tauri::command]
pub(crate) async fn save_rules(rules: Vec<harness_runtime::rules::RuleSpec>) -> Result<(), String> {
    harness_runtime::rules::save(&harness_runtime::rules::Rules { rules })
        .map_err(|e| e.to_string())
}

/// Rules worth offering to someone who has none, with the words a person
/// needs to decide. Shared with the CLI so both surfaces suggest the same set.
#[tauri::command]
pub(crate) async fn list_rule_suggestions(
) -> Result<Vec<harness_runtime::rules::Suggestion>, String> {
    Ok(harness_runtime::rules::suggestions())
}

/// What a drafting turn produced: what the model said, the rule it wrote, and
/// the check that ran against its own examples.
#[derive(serde::Serialize)]
pub(crate) struct DraftOutcome {
    note: String,
    rule: harness_agent::rules::DraftedRule,
    /// How many attempts it took — surfaced so a retry is visible rather than
    /// hidden latency.
    attempts: u32,
}

/// Write (or revise) a rule from a description, reusing the session's model.
///
/// Tokens stream to `rules://draft` as they arrive, so the editor shows the
/// model's sentence forming. The rule itself is checked before it returns — it
/// must compile, catch the example the model gave, and leave the near-miss
/// alone — so a rule that would never fire is retried rather than handed over.
#[tauri::command]
pub(crate) async fn draft_rule(
    app: AppHandle,
    state: State<'_, AppState>,
    request: String,
    history: Vec<harness_agent::rules::DraftTurn>,
) -> Result<DraftOutcome, String> {
    let mut ask = harness_agent::rules::draft_prompt(&request, &history);
    let mut last = String::new();
    for attempt in 1..=2u32 {
        let raw = stream_oneshot(&app, &state, harness_agent::rules::DRAFT_SYSTEM, &ask).await?;
        match harness_agent::rules::DraftReply::from_model_output(&raw) {
            Ok(reply) => {
                return Ok(DraftOutcome {
                    note: reply.note,
                    rule: reply.rule,
                    attempts: attempt,
                })
            }
            Err(why) => {
                last = why.clone();
                if attempt == 1 {
                    // Say so in the conversation: a silent retry reads as lag.
                    let _ = app.emit(
                        DRAFT_CHANNEL,
                        serde_json::json!({ "retry": format!("That one wouldn't work ({why}). Trying again.") }),
                    );
                    ask = format!(
                        "{ask}\n\nYour previous attempt was unusable: {why}. Try again, and \
                         check the pattern against your own examples first."
                    );
                }
            }
        }
    }
    Err(last)
}

/// A one-shot completion whose tokens are forwarded to the webview as they
/// arrive.
async fn stream_oneshot(
    app: &AppHandle,
    state: &AppState,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let (client, model, _) = state.client_for().await?;
    let request = ChatRequest::new(
        &model,
        vec![
            ChatMessage::system(system.to_string()),
            ChatMessage::user(user.to_string()),
        ],
    )
    .streaming(true);
    let assembled = client
        .stream_chat(&request, &CancellationToken::new(), |event| {
            if let StreamEvent::Token(t) = event {
                let _ = app.emit(DRAFT_CHANNEL, serde_json::json!({ "delta": t }));
            }
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(assembled.content)
}

/// What `pattern` matches in `sample`, through the agent's own regex engine.
#[tauri::command]
pub(crate) async fn check_rule_pattern(
    pattern: String,
    sample: String,
) -> Result<harness_agent::rules::PatternCheck, String> {
    Ok(harness_agent::rules::check_pattern(&pattern, &sample))
}
