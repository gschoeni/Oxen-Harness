//! Per-tool preferences, shared by the CLI and desktop app.
//!
//! The agent ships a fixed set of built-in tools (file read/write/edit, search,
//! shell, git, web search, planning). This module lets the user turn individual
//! tools off and override the description the model sees for a tool, persisted to
//! `~/.oxen-harness/tools.json` (versioned, safe to share — no secrets).
//!
//! Preferences are applied when an agent's [`ToolRegistry`] is built (see the
//! host's `agent_parts`): disabled tools are removed before the registry is
//! handed to the agent, and description overrides are layered onto the
//! definitions advertised to the model. Because they're applied at build time,
//! changes take effect for new (and resumed) chats rather than the live one.

use std::collections::BTreeMap;

use harness_config::io::{read_versioned, write_versioned};
use harness_config::paths;
use harness_tools::ToolRegistry;
use serde::{Deserialize, Serialize};

use crate::{config_repo, RuntimeError};

/// Schema version for `tools.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// Persisted tool preferences. Empty by default — every tool starts enabled with
/// its built-in description, so a fresh install behaves exactly as before.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ToolPrefs {
    /// Tool names the user has turned off; removed from the registry at build time.
    #[serde(default)]
    pub disabled: Vec<String>,
    /// Tool name → a custom description that replaces the built-in one in the
    /// definitions sent to the model.
    #[serde(default)]
    pub descriptions: BTreeMap<String, String>,
}

impl ToolPrefs {
    pub fn is_enabled(&self, name: &str) -> bool {
        !self.disabled.iter().any(|n| n == name)
    }

    /// Turn a tool on or off, returning whether anything changed.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> bool {
        let present = self.disabled.iter().any(|n| n == name);
        match (enabled, present) {
            // Enabling a currently-disabled tool drops it from the list.
            (true, true) => {
                self.disabled.retain(|n| n != name);
                true
            }
            // Disabling a currently-enabled tool adds it.
            (false, false) => {
                self.disabled.push(name.to_string());
                true
            }
            _ => false,
        }
    }

    /// Set (`Some`, non-blank) or clear (`None`/blank) a tool's description
    /// override. Returns whether anything changed.
    pub fn set_description(&mut self, name: &str, description: Option<&str>) -> bool {
        match description.map(str::trim) {
            Some(d) if !d.is_empty() => {
                self.descriptions.insert(name.to_string(), d.to_string()) != Some(d.to_string())
            }
            _ => self.descriptions.remove(name).is_some(),
        }
    }

    /// Apply these preferences to a freshly-built registry: drop disabled tools
    /// and layer description overrides onto the ones that remain.
    pub fn apply(&self, registry: &mut ToolRegistry) {
        for name in &self.disabled {
            registry.remove(name);
        }
        for (name, description) in &self.descriptions {
            // Only override tools that are actually registered (and not disabled).
            if registry.get(name).is_some() {
                registry.set_description_override(name, description);
            }
        }
    }
}

/// One tool as shown on the Tools settings page: its identity, the description
/// currently advertised to the model, the built-in default (so the UI can show
/// what an override replaced), its JSON schema, and whether it's enabled.
#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub default_description: String,
    pub parameters: serde_json::Value,
    pub enabled: bool,
    /// Always true today — every tool is a shipped built-in. Reserved for when
    /// custom / MCP tools land.
    pub builtin: bool,
}

/// Read the saved tool preferences (defaults to "everything enabled" on a fresh
/// install or unreadable file).
pub fn load() -> ToolPrefs {
    let Ok(path) = paths::tools_file() else {
        return ToolPrefs::default();
    };
    let (_version, prefs) = read_versioned::<ToolPrefs>(&path);
    prefs
}

/// Atomically persist the tool preferences and snapshot the config repo.
pub fn save(prefs: &ToolPrefs) -> Result<(), RuntimeError> {
    let path = paths::tools_file()?;
    write_versioned(&path, SCHEMA_VERSION, prefs)?;
    config_repo::snapshot("Update tool preferences");
    Ok(())
}

/// Enumerate every built-in tool with its current enabled/override state, for the
/// settings page. Built from the supplied full registry (which must contain all
/// shippable tools) overlaid with the saved preferences — so disabled tools still
/// appear (toggled off) rather than vanishing.
pub fn list(full_registry: &ToolRegistry, prefs: &ToolPrefs) -> Vec<ToolInfo> {
    full_registry
        .specs()
        .into_iter()
        .map(|(name, default_description, parameters)| {
            let description = prefs
                .descriptions
                .get(&name)
                .cloned()
                .unwrap_or_else(|| default_description.clone());
            let enabled = prefs.is_enabled(&name);
            ToolInfo {
                name,
                description,
                default_description,
                parameters,
                enabled,
                builtin: true,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_disable_round_trips() {
        let mut prefs = ToolPrefs::default();
        assert!(prefs.is_enabled("shell"));
        assert!(prefs.set_enabled("shell", false));
        assert!(!prefs.is_enabled("shell"));
        // No-op when already in the requested state.
        assert!(!prefs.set_enabled("shell", false));
        assert!(prefs.set_enabled("shell", true));
        assert!(prefs.is_enabled("shell"));
    }

    #[test]
    fn description_override_set_and_clear() {
        let mut prefs = ToolPrefs::default();
        assert!(prefs.set_description("shell", Some("Run a command")));
        assert_eq!(
            prefs.descriptions.get("shell").map(String::as_str),
            Some("Run a command")
        );
        // Blank clears it.
        assert!(prefs.set_description("shell", Some("  ")));
        assert!(!prefs.descriptions.contains_key("shell"));
        // Clearing an absent override is a no-op.
        assert!(!prefs.set_description("shell", None));
    }
}
