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

use harness_config::paths;
use harness_tools::ToolRegistry;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

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
    /// User-added HTTP tools. These are safe to share: endpoint URLs and schemas,
    /// but no secrets.
    #[serde(default)]
    pub custom: Vec<harness_tools::CustomToolSpec>,
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

    /// Add a custom tool, returning an error message when the spec is incomplete
    /// or would shadow an existing tool.
    pub fn add_custom(
        &mut self,
        registry: &ToolRegistry,
        spec: harness_tools::CustomToolSpec,
    ) -> Result<(), String> {
        let name = spec.name.trim().to_string();
        if !is_valid_tool_name(&name) {
            return Err(
                "Use a name like `lookup_customer` — letters, numbers, and underscores only."
                    .into(),
            );
        }
        if spec.description.trim().is_empty() {
            return Err("Add a short description so the model knows when to use this tool.".into());
        }
        match &spec.action {
            harness_tools::CustomToolAction::HttpPost { url } if is_valid_http_url(url) => {}
            harness_tools::CustomToolAction::HttpPost { .. } => {
                return Err("Enter an http:// or https:// endpoint URL.".into())
            }
        }
        if !spec.parameters.is_object() {
            return Err("Parameters must be a JSON Schema object.".into());
        }
        if registry.get(&name).is_some() && !self.custom.iter().any(|t| t.name == name) {
            return Err(format!(
                "`{name}` is a built-in tool name. Choose a unique name."
            ));
        }

        let mut spec = spec;
        spec.name = name.clone();
        spec.description = spec.description.trim().to_string();
        let action = match spec.action {
            harness_tools::CustomToolAction::HttpPost { url } => {
                harness_tools::CustomToolAction::HttpPost {
                    url: url.trim().to_string(),
                }
            }
        };
        spec.action = action;

        if let Some(existing) = self.custom.iter_mut().find(|t| t.name == spec.name) {
            *existing = spec;
        } else {
            self.custom.push(spec);
        }
        self.set_enabled(&name, true);
        Ok(())
    }

    pub fn remove_custom(&mut self, name: &str) -> bool {
        let before = self.custom.len();
        self.custom.retain(|t| t.name != name);
        self.descriptions.remove(name);
        self.disabled.retain(|n| n != name);
        self.custom.len() != before
    }

    /// Apply these preferences to a freshly-built registry: register custom
    /// tools, drop disabled tools, and layer description overrides onto the ones
    /// that remain.
    pub fn apply(&self, registry: &mut ToolRegistry) {
        for spec in &self.custom {
            registry.register_custom(spec.clone());
        }
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

fn is_valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_valid_http_url(url: &str) -> bool {
    let trimmed = url.trim();
    (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        && trimmed.len() > "http://".len()
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
    pub builtin: bool,
    pub config: BTreeMap<String, serde_json::Value>,
}

/// Read the saved tool preferences (defaults to "everything enabled" on a fresh
/// install or unreadable file).
pub fn load() -> ToolPrefs {
    crate::config::load_or_default(paths::tools_file())
}

/// Atomically persist the tool preferences and snapshot the config repo.
pub fn save(prefs: &ToolPrefs) -> Result<(), RuntimeError> {
    crate::config::write_and_snapshot(
        &paths::tools_file()?,
        SCHEMA_VERSION,
        prefs,
        "Update tool preferences",
    )
}

/// Enumerate every built-in and custom tool with its current enabled/override
/// state, for the settings page. Built from the supplied full registry (which
/// must contain all shippable tools) overlaid with the saved preferences — so
/// disabled tools still appear (toggled off) rather than vanishing.
pub fn list(full_registry: &ToolRegistry, prefs: &ToolPrefs) -> Vec<ToolInfo> {
    let mut out: Vec<ToolInfo> = full_registry
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
                config: BTreeMap::new(),
            }
        })
        .collect();

    out.extend(prefs.custom.iter().map(|spec| {
        let description = prefs
            .descriptions
            .get(&spec.name)
            .cloned()
            .unwrap_or_else(|| spec.description.clone());
        let mut config = BTreeMap::new();
        let harness_tools::CustomToolAction::HttpPost { url } = &spec.action;
        config.insert("type".into(), serde_json::json!("HTTP POST"));
        config.insert("url".into(), serde_json::json!(url));
        ToolInfo {
            name: spec.name.clone(),
            description,
            default_description: spec.description.clone(),
            parameters: spec.parameters.clone(),
            enabled: prefs.is_enabled(&spec.name),
            builtin: false,
            config,
        }
    }));

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn add_custom(
    spec: harness_tools::CustomToolSpec,
    full_registry: &ToolRegistry,
) -> Result<(), RuntimeError> {
    let mut prefs = load();
    prefs
        .add_custom(full_registry, spec)
        .map_err(RuntimeError::Invalid)?;
    save(&prefs)
}

pub fn remove_custom(name: &str) -> Result<(), RuntimeError> {
    let mut prefs = load();
    if prefs.remove_custom(name) {
        save(&prefs)?;
    }
    Ok(())
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

    fn spec(name: &str, url: &str) -> harness_tools::CustomToolSpec {
        harness_tools::CustomToolSpec {
            name: name.into(),
            description: "Look something up.".into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            action: harness_tools::CustomToolAction::HttpPost { url: url.into() },
        }
    }

    #[test]
    fn add_custom_validates_the_spec() {
        let registry = ToolRegistry::new();
        let mut prefs = ToolPrefs::default();
        // Bad name, blank description, bad URL, non-object schema — all rejected.
        assert!(prefs
            .add_custom(&registry, spec("Bad Name", "https://x.dev"))
            .is_err());
        let mut blank = spec("lookup", "https://x.dev");
        blank.description = "  ".into();
        assert!(prefs.add_custom(&registry, blank).is_err());
        assert!(prefs
            .add_custom(&registry, spec("lookup", "ftp://x.dev"))
            .is_err());
        let mut arr = spec("lookup", "https://x.dev");
        arr.parameters = serde_json::json!([]);
        assert!(prefs.add_custom(&registry, arr).is_err());
        assert!(prefs.custom.is_empty());
    }

    #[test]
    fn add_custom_rejects_builtin_names_and_upserts_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register_custom(spec("shipped", "https://builtin.dev"));
        let mut prefs = ToolPrefs::default();
        // Shadowing a registered (built-in) tool is an error…
        assert!(prefs
            .add_custom(&registry, spec("shipped", "https://x.dev"))
            .is_err());
        // …but re-adding one of *our* custom tools updates it in place.
        prefs
            .add_custom(&registry, spec("lookup", "https://v1.dev"))
            .unwrap();
        prefs
            .add_custom(&registry, spec(" lookup ", " https://v2.dev "))
            .unwrap();
        assert_eq!(prefs.custom.len(), 1);
        let harness_tools::CustomToolAction::HttpPost { url } = &prefs.custom[0].action;
        assert_eq!(url, "https://v2.dev");
    }

    #[test]
    fn add_custom_reenables_and_remove_clears_prefs() {
        let registry = ToolRegistry::new();
        let mut prefs = ToolPrefs::default();
        prefs
            .add_custom(&registry, spec("lookup", "https://x.dev"))
            .unwrap();
        prefs.set_enabled("lookup", false);
        // Saving the tool again turns it back on — an edited tool should work.
        prefs
            .add_custom(&registry, spec("lookup", "https://x.dev"))
            .unwrap();
        assert!(prefs.is_enabled("lookup"));

        prefs.set_enabled("lookup", false);
        prefs.set_description("lookup", Some("override"));
        assert!(prefs.remove_custom("lookup"));
        assert!(prefs.custom.is_empty());
        assert!(prefs.disabled.is_empty());
        assert!(prefs.descriptions.is_empty());
        // Removing an unknown tool reports nothing removed.
        assert!(!prefs.remove_custom("lookup"));
    }

    #[test]
    fn custom_tools_appear_in_list_and_apply() {
        let registry = ToolRegistry::new();
        let mut prefs = ToolPrefs::default();
        prefs
            .add_custom(&registry, spec("lookup", "https://x.dev"))
            .unwrap();

        let infos = list(&registry, &prefs);
        let info = infos.iter().find(|t| t.name == "lookup").expect("listed");
        assert!(!info.builtin);
        assert!(info.enabled);
        assert_eq!(
            info.config.get("url"),
            Some(&serde_json::json!("https://x.dev"))
        );

        let mut fresh = ToolRegistry::new();
        prefs.apply(&mut fresh);
        assert!(fresh.get("lookup").is_some());
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
