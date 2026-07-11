//! Where themes live on disk and how they're selected, shared, and persisted.
//!
//! Everything lives under `~/.oxen-harness/` (alongside history + models):
//! - `config.toml` records the active theme by slug, plus the user's location
//!   (the "Departing" line in the CLI banner and the hero screen's first
//!   status row — user data, so it survives theme switches).
//! - `themes/<slug>.toml` holds installed/imported/created themes.
//!
//! Resolution prefers an installed file over a built-in of the same slug, so a
//! user can fork and override a built-in just by saving a theme with its name.
//! The location is overlaid onto the *active* theme as it loads — never onto
//! saved or exported theme files, which stay shareable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{builtins, Theme, ThemeError};

/// The default active theme slug when none is configured.
pub const DEFAULT_SLUG: &str = "oregon-trail";

/// A normalized, filesystem-safe identifier derived from a theme's name.
///
/// Thin wrapper over [`harness_core::text::slug`] that pins the empty-name
/// fallback to `"theme"`.
pub fn slug(name: &str) -> String {
    harness_core::text::slug(name, "theme")
}

#[derive(Default, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    theme: Option<String>,
    /// The user's location, overlaid on the active theme's first flavor row
    /// (the CLI banner's "Departing" line and the hero screen's status rows).
    #[serde(default)]
    location: Option<String>,
}

/// A one-line description of an available theme, for listings/pickers.
#[derive(Clone, Debug, Serialize)]
pub struct ThemeSummary {
    pub name: String,
    pub slug: String,
    pub description: String,
    /// Ships with the harness.
    pub builtin: bool,
    /// Has a saved file under `themes/`.
    pub installed: bool,
    /// Currently the active theme.
    pub active: bool,
}

/// Reads/writes themes and the active selection under a root directory.
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Open the standard store at `~/.oxen-harness/`, creating it if needed.
    pub fn open() -> Result<Self, ThemeError> {
        let base = harness_config::base_dir().map_err(|_| ThemeError::NoConfigDir)?;
        Self::with_root(base)
    }

    /// Open a store rooted at an explicit directory (used in tests).
    pub fn with_root(root: impl Into<PathBuf>) -> Result<Self, ThemeError> {
        let root = root.into();
        std::fs::create_dir_all(root.join("themes"))?;
        Ok(Self { root })
    }

    pub fn themes_dir(&self) -> PathBuf {
        self.root.join("themes")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    fn theme_path(&self, slug: &str) -> PathBuf {
        self.themes_dir().join(format!("{slug}.toml"))
    }

    fn read_config(&self) -> Config {
        std::fs::read_to_string(self.config_path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// The slug of the active theme (defaults to Oregon Trail).
    pub fn active_slug(&self) -> String {
        self.read_config()
            .theme
            .map(|t| slug(&t))
            .unwrap_or_else(|| DEFAULT_SLUG.to_string())
    }

    /// Resolve a theme by name or slug: an installed file wins over a built-in.
    pub fn resolve(&self, name: &str) -> Result<Theme, ThemeError> {
        let slug = slug(name);
        let path = self.theme_path(&slug);
        if path.exists() {
            return Theme::from_toml_str(&std::fs::read_to_string(path)?);
        }
        builtins::by_name(&slug).ok_or_else(|| ThemeError::NotFound(name.to_string()))
    }

    /// The active theme (with the user's location overlaid), falling back to
    /// the default if anything is amiss.
    pub fn load_active(&self) -> Theme {
        let mut theme = self.resolve(&self.active_slug()).unwrap_or_default();
        self.overlay_location(&mut theme);
        theme
    }

    /// Set the active theme (must resolve). Stores its slug in `config.toml`
    /// (preserving the other settings there) and returns the theme with the
    /// user's location overlaid, ready to render.
    pub fn set_active(&self, name: &str) -> Result<Theme, ThemeError> {
        let mut theme = self.resolve(name)?;
        let mut cfg = self.read_config();
        cfg.theme = Some(slug(&theme.meta.name));
        self.write_config(&cfg)?;
        self.overlay_location(&mut theme);
        Ok(theme)
    }

    fn write_config(&self, cfg: &Config) -> Result<(), ThemeError> {
        std::fs::write(self.config_path(), toml::to_string_pretty(cfg)?)?;
        Ok(())
    }

    /// The user's saved location, if set.
    pub fn location(&self) -> Option<String> {
        self.read_config()
            .location
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
    }

    /// Persist (or, with `None`/blank, clear) the user's location.
    pub fn set_location(&self, location: Option<&str>) -> Result<(), ThemeError> {
        let mut cfg = self.read_config();
        cfg.location = location
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty());
        self.write_config(&cfg)
    }

    /// Write the saved location into a theme's first `flavor_top` row — the
    /// CLI banner's "Departing" line and the hero screen's first status row.
    /// The row's themed label is kept; a theme with no rows gains a
    /// `Location` one. A no-op when no location is saved.
    fn overlay_location(&self, theme: &mut Theme) {
        let Some(location) = self.location() else {
            return;
        };
        match theme.voice.flavor_top.first_mut() {
            Some(row) => row[1] = location,
            None => theme
                .voice
                .flavor_top
                .push(["Location".to_string(), location]),
        }
    }

    /// Save a theme to `themes/<slug>.toml`, returning its path.
    pub fn save(&self, theme: &Theme) -> Result<PathBuf, ThemeError> {
        let path = self.theme_path(&slug(&theme.meta.name));
        std::fs::write(&path, theme.to_toml()?)?;
        Ok(path)
    }

    /// Import a theme from a TOML or JSON file, installing it under `themes/`.
    pub fn import(&self, path: impl AsRef<Path>) -> Result<Theme, ThemeError> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)?;
        let theme = parse_theme(&raw, path)?;
        self.save(&theme)?;
        Ok(theme)
    }

    /// Install a theme parsed from raw TOML/JSON text (used by the app + LLM gen).
    pub fn install_from_str(&self, raw: &str) -> Result<Theme, ThemeError> {
        let theme = parse_theme(raw, Path::new("theme.toml"))?;
        self.save(&theme)?;
        Ok(theme)
    }

    /// Export the resolved theme to a destination path as TOML.
    pub fn export(&self, name: &str, dest: impl AsRef<Path>) -> Result<PathBuf, ThemeError> {
        let theme = self.resolve(name)?;
        let dest = dest.as_ref().to_path_buf();
        std::fs::write(&dest, theme.to_toml()?)?;
        Ok(dest)
    }

    /// Remove an installed theme file (built-ins remain available).
    pub fn remove(&self, name: &str) -> Result<(), ThemeError> {
        let path = self.theme_path(&slug(name));
        if path.exists() {
            std::fs::remove_file(path)?;
            Ok(())
        } else {
            Err(ThemeError::NotFound(name.to_string()))
        }
    }

    /// All available themes (built-ins + installed files), default first,
    /// installed files overriding built-ins of the same slug.
    pub fn list(&self) -> Vec<ThemeSummary> {
        let active = self.active_slug();
        let mut out: Vec<ThemeSummary> = Vec::new();
        let mut seen: Vec<String> = Vec::new();

        for t in builtins::all() {
            let sl = slug(&t.meta.name);
            let installed = self.theme_path(&sl).exists();
            seen.push(sl.clone());
            out.push(ThemeSummary {
                active: sl == active,
                name: t.meta.name,
                description: t.meta.description,
                builtin: true,
                installed,
                slug: sl,
            });
        }

        // Installed-only (user/imported/generated) themes.
        if let Ok(entries) = std::fs::read_dir(self.themes_dir()) {
            let mut files: Vec<_> = entries.flatten().collect();
            files.sort_by_key(|e| e.file_name());
            for entry in files {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                let sl = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if sl.is_empty() || seen.iter().any(|s| s == sl) {
                    continue;
                }
                if let Ok(theme) =
                    Theme::from_toml_str(&std::fs::read_to_string(&path).unwrap_or_default())
                {
                    out.push(ThemeSummary {
                        active: sl == active,
                        name: theme.meta.name,
                        description: theme.meta.description,
                        builtin: false,
                        installed: true,
                        slug: sl.to_string(),
                    });
                }
            }
        }
        out
    }
}

/// Parse theme text as TOML, then JSON (by extension hint, then by fallback).
fn parse_theme(raw: &str, path: &Path) -> Result<Theme, ThemeError> {
    let is_json = path.extension().and_then(|e| e.to_str()) == Some("json");
    if is_json {
        return Theme::from_json_str(raw);
    }
    match Theme::from_toml_str(raw) {
        Ok(t) => Ok(t),
        Err(_) => Theme::from_json_str(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_root(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn slugs_are_filesystem_safe() {
        assert_eq!(slug("Oregon Trail"), "oregon-trail");
        assert_eq!(slug("  My!! Cool   Theme  "), "my-cool-theme");
        assert_eq!(slug("SYNTHWAVE"), "synthwave");
        assert_eq!(slug("***"), "theme");
    }

    #[test]
    fn active_defaults_to_oregon_trail_then_persists() {
        let (_d, store) = store();
        assert_eq!(store.active_slug(), DEFAULT_SLUG);
        assert_eq!(store.load_active().meta.name, "Oregon Trail");

        store.set_active("Midnight").unwrap();
        assert_eq!(store.active_slug(), "midnight");
        assert_eq!(store.load_active().meta.name, "Midnight");
    }

    #[test]
    fn location_persists_and_overlays_the_active_theme() {
        let (_d, store) = store();
        // The default theme ships its own departure row.
        let default_row = store.load_active().voice.flavor_top[0].clone();
        assert_eq!(default_row[0], "Departing");

        store.set_location(Some("Fort Laramie, Wyoming")).unwrap();
        assert_eq!(store.location().as_deref(), Some("Fort Laramie, Wyoming"));
        let theme = store.load_active();
        // The themed label survives; only the value is the user's.
        assert_eq!(theme.voice.flavor_top[0][0], "Departing");
        assert_eq!(theme.voice.flavor_top[0][1], "Fort Laramie, Wyoming");

        // Switching themes keeps the location (returned theme included) …
        let midnight = store.set_active("Midnight").unwrap();
        assert!(midnight
            .voice
            .flavor_top
            .iter()
            .any(|r| r[1] == "Fort Laramie, Wyoming"));
        assert_eq!(store.location().as_deref(), Some("Fort Laramie, Wyoming"));

        // … and clearing reverts to the theme's own row.
        store.set_location(None).unwrap();
        assert_eq!(store.location(), None);
        store.set_active("Oregon Trail").unwrap();
        assert_eq!(store.load_active().voice.flavor_top[0], default_row);
    }

    #[test]
    fn location_never_bakes_into_exported_themes() {
        let (_d, store) = store();
        store.set_location(Some("Independence Rock")).unwrap();
        let dest = store.themes_dir().join("exported.toml");
        store.export("Oregon Trail", &dest).unwrap();
        let raw = std::fs::read_to_string(&dest).unwrap();
        assert!(!raw.contains("Independence Rock"));
    }

    #[test]
    fn blank_location_reads_as_unset() {
        let (_d, store) = store();
        store.set_location(Some("   ")).unwrap();
        assert_eq!(store.location(), None);
    }

    #[test]
    fn save_resolve_and_remove_user_theme() {
        let (_d, store) = store();
        let mut theme = Theme::default();
        theme.meta.name = "My Custom".into();
        store.save(&theme).unwrap();

        let resolved = store.resolve("my custom").unwrap();
        assert_eq!(resolved.meta.name, "My Custom");
        assert!(store
            .list()
            .iter()
            .any(|s| s.slug == "my-custom" && !s.builtin));

        store.remove("My Custom").unwrap();
        assert!(store.resolve("My Custom").is_err());
    }

    #[test]
    fn export_then_import_round_trips() {
        let (_d, store) = store();
        let dest = store.themes_dir().join("exported.toml");
        store.export("Synthwave", &dest).unwrap();

        let imported = store.import(&dest).unwrap();
        assert_eq!(imported.meta.name, "Synthwave");
        assert_eq!(imported, builtins::by_name("synthwave").unwrap());
    }

    #[test]
    fn installed_file_overrides_builtin_of_same_slug() {
        let (_d, store) = store();
        let mut theme = builtins::by_name("midnight").unwrap();
        theme.voice.prompt_label = "custom ❯".into();
        store.save(&theme).unwrap();

        assert_eq!(
            store.resolve("midnight").unwrap().voice.prompt_label,
            "custom ❯"
        );
        // Still listed once, now marked installed.
        let midnight: Vec<_> = store
            .list()
            .into_iter()
            .filter(|s| s.slug == "midnight")
            .collect();
        assert_eq!(midnight.len(), 1);
        assert!(midnight[0].installed);
    }

    #[test]
    fn list_includes_builtins() {
        let (_d, store) = store();
        let list = store.list();
        assert!(list.iter().any(|s| s.slug == "oregon-trail" && s.active));
        assert!(list.iter().any(|s| s.slug == "midnight"));
        assert!(list.iter().any(|s| s.slug == "synthwave"));
    }
}
