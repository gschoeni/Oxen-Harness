//! The cloud model catalog, shared by the CLI and desktop app.
//!
//! The catalog is entirely user-curated: models are added from the configured
//! endpoint's hosted catalog (or manually by id) and live, along with the
//! currently selected default, in `~/.oxen-harness/models.json` (versioned,
//! safe to share — no secrets). Nothing ships pre-seeded; an empty catalog
//! falls back to [`harness_core::DEFAULT_MODEL`] so a fresh install can still
//! chat before configuring one.

use harness_config::paths;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

/// Schema version for `models.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// A model the user can pick. `id` is the string sent to the inference API;
/// `name` is a friendly label for the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
}

/// A catalog entry as rendered in the UI: a model plus whether it's the
/// currently selected default.
#[derive(Debug, Clone, Serialize)]
pub struct CloudModel {
    pub id: String,
    pub name: String,
    pub selected: bool,
}

/// Persisted state: the selected default model id (blank = the first built-in)
/// and the user's custom additions on top of the built-ins.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    /// The selected cloud model id (the default for new cloud chats).
    #[serde(default)]
    pub selected: String,
    #[serde(default)]
    pub custom: Vec<ModelEntry>,
    /// The local model the user last activated, when local is their current
    /// choice. Empty means a cloud model is active. Persisted so a chosen local
    /// model is restored on the next launch (desktop and CLI alike).
    #[serde(default)]
    pub active_local: String,
}

/// Read the persisted config (empty if the file is absent or unreadable).
pub fn load() -> ModelsConfig {
    crate::config::load_or_default(paths::models_file())
}

/// Atomically persist the config and snapshot the config repo.
fn write(cfg: &ModelsConfig) -> Result<(), RuntimeError> {
    crate::config::write_and_snapshot(&paths::models_file()?, SCHEMA_VERSION, cfg, "Update models")
}

/// The selected default model id: the persisted choice, else the first model
/// the user added, else the system fallback ([`harness_core::DEFAULT_MODEL`])
/// so a fresh install can still chat before curating a catalog.
pub fn selected() -> String {
    let cfg = load();
    let s = cfg.selected.trim();
    if !s.is_empty() {
        return s.to_string();
    }
    cfg.custom
        .into_iter()
        .next()
        .map(|m| m.id)
        .unwrap_or_else(|| harness_core::DEFAULT_MODEL.to_string())
}

/// The full catalog — every model the user has added, in the order added —
/// with the selected flag set for the UI. Empty until the user adds one.
pub fn catalog() -> Vec<CloudModel> {
    let cfg = load();
    let sel = selected();
    cfg.custom
        .into_iter()
        .map(|c| CloudModel {
            selected: c.id == sel,
            id: c.id,
            name: c.name,
        })
        .collect()
}

/// Add a model (or rename an existing one). Returns the updated catalog. The
/// first model added becomes the default via [`selected`]'s fallback.
pub fn add(id: &str, name: &str) -> Result<Vec<CloudModel>, RuntimeError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(RuntimeError::Invalid("model id cannot be empty".into()));
    }
    let name = if name.trim().is_empty() {
        id
    } else {
        name.trim()
    };
    let mut cfg = load();
    if let Some(existing) = cfg.custom.iter_mut().find(|e| e.id == id) {
        existing.name = name.to_string();
    } else {
        cfg.custom.push(ModelEntry {
            id: id.to_string(),
            name: name.to_string(),
        });
    }
    write(&cfg)?;
    Ok(catalog())
}

/// Remove a model. If it was selected, the selection falls back to the first
/// remaining model (or the system default). Returns the updated catalog.
pub fn remove(id: &str) -> Result<Vec<CloudModel>, RuntimeError> {
    let mut cfg = load();
    cfg.custom.retain(|e| e.id != id);
    if cfg.selected == id {
        cfg.selected.clear();
    }
    write(&cfg)?;
    Ok(catalog())
}

/// Persist the selected cloud model id (blank clears it to the default). Picking
/// a cloud model also deactivates any persisted local model.
pub fn set_selected(id: &str) -> Result<(), RuntimeError> {
    let mut cfg = load();
    cfg.selected = id.trim().to_string();
    cfg.active_local.clear();
    write(&cfg)
}

/// The local model the user last activated, if local is their current choice.
pub fn active_local() -> Option<String> {
    let v = load().active_local.trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// Record the active local model so it's restored next launch (empty clears it,
/// reverting to the selected cloud model).
pub fn set_active_local(id: &str) -> Result<(), RuntimeError> {
    let mut cfg = load();
    cfg.active_local = id.trim().to_string();
    write(&cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_temp_home;

    #[test]
    fn empty_catalog_falls_back_to_the_system_default() {
        with_temp_home(|| {
            assert!(catalog().is_empty());
            assert_eq!(selected(), harness_core::DEFAULT_MODEL);
        });
    }

    #[test]
    fn first_added_model_becomes_the_default() {
        with_temp_home(|| {
            add("my-model", "My Model").unwrap();
            add("other-model", "Other").unwrap();
            // No explicit selection yet — the first addition is the default.
            assert_eq!(selected(), "my-model");
            assert!(catalog().iter().any(|m| m.id == "my-model" && m.selected));
        });
    }

    #[test]
    fn add_select_remove_model() {
        with_temp_home(|| {
            add("my-model", "My Model").unwrap();
            assert!(catalog().iter().any(|m| m.id == "my-model"));

            set_selected("my-model").unwrap();
            assert_eq!(selected(), "my-model");
            assert!(catalog().iter().any(|m| m.id == "my-model" && m.selected));

            remove("my-model").unwrap();
            assert!(!catalog().iter().any(|m| m.id == "my-model"));
            // Selection fell back to the system default once its model was removed.
            assert_eq!(selected(), harness_core::DEFAULT_MODEL);
        });
    }

    #[test]
    fn active_local_persists_and_cloud_selection_clears_it() {
        with_temp_home(|| {
            assert_eq!(active_local(), None);
            set_active_local("qwen3-8b-q4-k-m").unwrap();
            assert_eq!(active_local().as_deref(), Some("qwen3-8b-q4-k-m"));
            // Selecting a cloud model deactivates the local one.
            set_selected("claude-sonnet-4-6").unwrap();
            assert_eq!(active_local(), None);
            assert_eq!(selected(), "claude-sonnet-4-6");
        });
    }

    #[test]
    fn re_adding_a_model_renames_instead_of_duplicating() {
        with_temp_home(|| {
            add("my-model", "My Model").unwrap();
            add("my-model", "Renamed").unwrap();
            let cat = catalog();
            assert_eq!(cat.len(), 1);
            assert_eq!(cat[0].name, "Renamed");
        });
    }
}
