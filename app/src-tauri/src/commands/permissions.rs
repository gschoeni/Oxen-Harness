//! Settings → Permissions: read and edit the permission mode and allow/deny
//! rules (global `~/.oxen-harness/permissions.json` + the active project's
//! `.oxen-harness/permissions.json`).
//!
//! Like tool preferences, rule edits are applied when an agent (and its gate)
//! is built — they reach new and resumed chats. The mode switch also updates
//! nothing live here; the CLI's `/permissions` flips its live gate because it
//! holds one, while the desktop treats settings as build-time config.

use harness_permissions::{policy, PermissionMode, PermissionsConfig};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

/// One scope's rules as shown on the page.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuleSet {
    pub(crate) mode: Option<String>,
    pub(crate) allow: Vec<String>,
    pub(crate) allow_exact: Vec<String>,
    pub(crate) deny: Vec<String>,
}

impl From<PermissionsConfig> for RuleSet {
    fn from(config: PermissionsConfig) -> Self {
        Self {
            mode: config.mode.map(|m| m.label().to_string()),
            allow: config.allow,
            allow_exact: config.allow_exact,
            deny: config.deny,
        }
    }
}

/// Everything the Permissions settings page renders.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PermissionsView {
    /// The effective default mode (project override, else global, else relaxed).
    pub(crate) mode: String,
    pub(crate) global: RuleSet,
    pub(crate) project: RuleSet,
    pub(crate) project_path: String,
}

#[tauri::command]
pub(crate) async fn get_permissions(state: State<'_, AppState>) -> Result<PermissionsView, String> {
    let root = state.active_root().await;
    let global = policy::load_global();
    let project = policy::load_project(&root);
    let mode = project
        .mode
        .or(global.mode)
        .unwrap_or_default()
        .label()
        .to_string();
    Ok(PermissionsView {
        mode,
        global: global.into(),
        project: project.into(),
        project_path: root.display().to_string(),
    })
}

fn parse_mode(mode: &str) -> Result<PermissionMode, String> {
    match mode {
        "relaxed" => Ok(PermissionMode::Relaxed),
        "cautious" => Ok(PermissionMode::Cautious),
        "bypass" => Ok(PermissionMode::Bypass),
        other => Err(format!("unknown permission mode `{other}`")),
    }
}

/// Set the global default mode. Applies to new and resumed chats.
#[tauri::command]
pub(crate) async fn set_permission_mode(mode: String) -> Result<(), String> {
    policy::persist_global_mode(parse_mode(&mode)?).map_err(|e| e.to_string())
}

/// Add one rule to a scope ("global" | "project") and kind
/// ("allow" | "allow_exact" | "deny").
#[tauri::command]
pub(crate) async fn add_permission_rule(
    state: State<'_, AppState>,
    scope: String,
    kind: String,
    value: String,
) -> Result<(), String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err("enter a command prefix".to_string());
    }
    edit_rules(&state, &scope, |config| {
        let list = list_for(config, &kind)?;
        if !list.contains(&value) {
            list.push(value.clone());
        }
        Ok(())
    })
    .await
}

/// Remove one rule from a scope/kind.
#[tauri::command]
pub(crate) async fn remove_permission_rule(
    state: State<'_, AppState>,
    scope: String,
    kind: String,
    value: String,
) -> Result<(), String> {
    edit_rules(&state, &scope, |config| {
        list_for(config, &kind)?.retain(|r| r != &value);
        Ok(())
    })
    .await
}

fn list_for<'a>(
    config: &'a mut PermissionsConfig,
    kind: &str,
) -> Result<&'a mut Vec<String>, String> {
    match kind {
        "allow" => Ok(&mut config.allow),
        "allow_exact" => Ok(&mut config.allow_exact),
        "deny" => Ok(&mut config.deny),
        other => Err(format!("unknown rule kind `{other}`")),
    }
}

async fn edit_rules(
    state: &State<'_, AppState>,
    scope: &str,
    edit: impl FnOnce(&mut PermissionsConfig) -> Result<(), String>,
) -> Result<(), String> {
    match scope {
        "global" => {
            let mut config = policy::load_global();
            edit(&mut config)?;
            policy::save_global(&config).map_err(|e| e.to_string())
        }
        "project" => {
            let root = state.active_root().await;
            let mut config = policy::load_project(&root);
            edit(&mut config)?;
            policy::save_project(&root, &config).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown scope `{other}`")),
    }
}
