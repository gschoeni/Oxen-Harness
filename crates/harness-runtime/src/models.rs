//! The cloud model catalog, shared by the CLI and desktop app.
//!
//! A short list of built-in models is always offered; on top of that the user
//! can add their own (any model id the configured Oxen endpoint serves). The
//! custom additions and the currently selected default live in
//! `~/.oxen-harness/models.json` (versioned, safe to share — no secrets).
//!
//! Built-ins are merged in at read time rather than written to disk, so the set
//! the app ships with can change without rewriting every user's file.

use harness_config::io::{read_versioned, write_versioned};
use harness_config::paths;
use serde::{Deserialize, Serialize};

use crate::{config_repo, RuntimeError};

/// Schema version for `models.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// A model the user can pick. `id` is the string sent to the inference API;
/// `name` is a friendly label for the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
}

/// A catalog entry as rendered in the UI: a model plus whether it's a built-in
/// (so it can't be removed) and whether it's the currently selected default.
#[derive(Debug, Clone, Serialize)]
pub struct CloudModel {
    pub id: String,
    pub name: String,
    pub builtin: bool,
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

/// The built-in cloud models, always offered. The first is the default.
pub fn builtins() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            id: "claude-opus-4-8".into(),
            name: "Claude Opus 4.8".into(),
        },
        ModelEntry {
            id: "claude-sonnet-4-6".into(),
            name: "Claude Sonnet 4.6".into(),
        },
        ModelEntry {
            id: "claude-haiku-4-5-20251001".into(),
            name: "Claude Haiku 4.5".into(),
        },
    ]
}

/// Read the persisted config (empty if the file is absent or unreadable).
pub fn load() -> ModelsConfig {
    match paths::models_file() {
        Ok(p) => read_versioned::<ModelsConfig>(&p).1,
        Err(_) => ModelsConfig::default(),
    }
}

/// Atomically persist the config and snapshot the config repo.
fn write(cfg: &ModelsConfig) -> Result<(), RuntimeError> {
    let path = paths::models_file()?;
    write_versioned(&path, SCHEMA_VERSION, cfg)?;
    config_repo::snapshot("Update models");
    Ok(())
}

/// The selected default model id, falling back to the first built-in when none
/// has been chosen (or the chosen one was removed).
pub fn selected() -> String {
    let cfg = load();
    let s = cfg.selected.trim();
    if s.is_empty() {
        builtins()
            .into_iter()
            .next()
            .map(|m| m.id)
            .unwrap_or_default()
    } else {
        s.to_string()
    }
}

/// The full catalog — built-ins first, then custom — with the builtin/selected
/// flags set for the UI.
pub fn catalog() -> Vec<CloudModel> {
    let cfg = load();
    let sel = selected();
    let mut out: Vec<CloudModel> = Vec::new();
    for b in builtins() {
        out.push(CloudModel {
            selected: b.id == sel,
            id: b.id,
            name: b.name,
            builtin: true,
        });
    }
    for c in cfg.custom {
        // A custom entry that shadows a built-in id is ignored — the built-in wins.
        if out.iter().any(|m| m.id == c.id) {
            continue;
        }
        out.push(CloudModel {
            selected: c.id == sel,
            id: c.id,
            name: c.name,
            builtin: false,
        });
    }
    out
}

/// Add a custom model (or rename an existing one). Built-in ids are left to the
/// built-in list. Returns the updated catalog.
pub fn add(id: &str, name: &str) -> Result<Vec<CloudModel>, RuntimeError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(RuntimeError::Invalid("model id cannot be empty".into()));
    }
    // Don't persist a custom that just shadows a built-in.
    if builtins().iter().any(|b| b.id == id) {
        return Ok(catalog());
    }
    let name = if name.trim().is_empty() { id } else { name.trim() };
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

/// Remove a custom model (built-ins can't be removed). If it was selected, the
/// selection falls back to the default. Returns the updated catalog.
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

    fn with_temp_home<T>(f: impl FnOnce() -> T) -> T {
        let _lock = crate::TEST_ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(paths::BASE_DIR_ENV, tmp.path());
        let out = f();
        std::env::remove_var(paths::BASE_DIR_ENV);
        out
    }

    #[test]
    fn defaults_to_first_builtin_and_lists_builtins() {
        with_temp_home(|| {
            assert_eq!(selected(), builtins()[0].id);
            let cat = catalog();
            assert!(cat.iter().all(|m| m.builtin));
            assert!(cat.iter().any(|m| m.selected));
        });
    }

    #[test]
    fn add_select_remove_custom_model() {
        with_temp_home(|| {
            add("my-model", "My Model").unwrap();
            assert!(catalog().iter().any(|m| m.id == "my-model" && !m.builtin));

            set_selected("my-model").unwrap();
            assert_eq!(selected(), "my-model");
            assert!(catalog().iter().any(|m| m.id == "my-model" && m.selected));

            remove("my-model").unwrap();
            assert!(!catalog().iter().any(|m| m.id == "my-model"));
            // Selection fell back to the default once its model was removed.
            assert_eq!(selected(), builtins()[0].id);
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
    fn custom_cannot_shadow_a_builtin() {
        with_temp_home(|| {
            let before = catalog().len();
            add(&builtins()[0].id, "Shadow").unwrap();
            assert_eq!(catalog().len(), before);
        });
    }
}
