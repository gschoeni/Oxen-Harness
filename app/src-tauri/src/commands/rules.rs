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

use tauri::State;

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

/// What `pattern` matches in `sample`, through the agent's own regex engine.
#[tauri::command]
pub(crate) async fn check_rule_pattern(
    pattern: String,
    sample: String,
) -> Result<harness_agent::rules::PatternCheck, String> {
    Ok(harness_agent::rules::check_pattern(&pattern, &sample))
}
