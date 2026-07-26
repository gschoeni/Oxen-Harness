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

use harness_llm::types::ChatMessage;
use harness_llm::ChatRequest;
use tauri::State;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

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

/// Write a rule from a description, reusing the session's model and endpoint.
///
/// The draft is checked before it comes back — it must compile, catch the
/// example the model says it catches, and leave the near-miss alone — so a
/// rule that would never fire is retried rather than handed over. One retry,
/// with the failure fed back, since the second attempt is usually the fix and
/// a third rarely is.
#[tauri::command]
pub(crate) async fn draft_rule(
    state: State<'_, AppState>,
    description: String,
) -> Result<harness_agent::rules::DraftedRule, String> {
    let mut ask = description.clone();
    let mut last = String::new();
    for attempt in 0..2 {
        let raw = complete_oneshot(&state, harness_agent::rules::DRAFT_SYSTEM, &ask).await?;
        match harness_agent::rules::DraftedRule::from_model_output(&raw) {
            Ok(drafted) => return Ok(drafted),
            Err(why) => {
                last = why.clone();
                if attempt == 0 {
                    ask = format!(
                        "{description}\n\nYour previous attempt was unusable: {why}. \
                         Try again, and check the pattern against your own examples first."
                    );
                }
            }
        }
    }
    Err(last)
}

async fn complete_oneshot(state: &AppState, system: &str, user: &str) -> Result<String, String> {
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
        .stream_chat(&request, &CancellationToken::new(), |_| {})
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
