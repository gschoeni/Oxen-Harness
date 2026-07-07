//! Where loops live on disk: `~/.oxen-harness/loops/<slug>.toml` for definitions
//! and `~/.oxen-harness/loops/runs/<slug>.json` for the latest run journal.
//!
//! Resolution prefers an installed file over a built-in of the same slug, so a
//! user can fork and override a built-in just by saving a loop with its name.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::journal::LoopJournal;
use crate::spec::{slug, LoopSpec};
use crate::{builtins, LoopError};

/// A one-line description of an available loop, for listings.
#[derive(Clone, Debug, Serialize)]
pub struct LoopSummary {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub verify: String,
    pub builtin: bool,
    pub installed: bool,
}

/// Reads/writes loop definitions and run journals under a root directory.
pub struct LoopStore {
    root: PathBuf,
}

impl LoopStore {
    /// Open the standard store at `~/.oxen-harness/loops/`, creating it if needed.
    pub fn open() -> Result<Self, LoopError> {
        let dir = harness_config::paths::loops_dir().map_err(|_| LoopError::NoHome)?;
        Self::with_root(dir)
    }

    /// Open a store rooted at an explicit directory (used in tests).
    pub fn with_root(root: impl Into<PathBuf>) -> Result<Self, LoopError> {
        let root = root.into();
        std::fs::create_dir_all(root.join("runs"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn spec_path(&self, slug: &str) -> PathBuf {
        self.root.join(format!("{slug}.toml"))
    }

    fn journal_path(&self, slug: &str) -> PathBuf {
        self.root.join("runs").join(format!("{slug}.json"))
    }

    /// Where the journal for `name`'s next run should be written.
    pub fn journal_path_for(&self, name: &str) -> PathBuf {
        self.journal_path(&slug(name))
    }

    /// Resolve a loop by name or slug: an installed file wins over a built-in.
    pub fn resolve(&self, name: &str) -> Result<LoopSpec, LoopError> {
        let sl = slug(name);
        let path = self.spec_path(&sl);
        if path.exists() {
            return Ok(LoopSpec::from_toml_str(&std::fs::read_to_string(path)?)?);
        }
        builtins::by_slug(&sl).ok_or_else(|| LoopError::NotFound(name.to_string()))
    }

    /// Save a loop to `<slug>.toml`, returning its path.
    pub fn save(&self, spec: &LoopSpec) -> Result<PathBuf, LoopError> {
        let path = self.spec_path(&slug(&spec.name));
        std::fs::write(&path, spec.to_toml()?)?;
        Ok(path)
    }

    /// Import a loop from a TOML file, installing it under `loops/`.
    pub fn import(&self, path: impl AsRef<Path>) -> Result<LoopSpec, LoopError> {
        let spec = LoopSpec::from_toml_str(&std::fs::read_to_string(path.as_ref())?)?;
        self.save(&spec)?;
        Ok(spec)
    }

    /// Export a resolved loop to a destination path as TOML.
    pub fn export(&self, name: &str, dest: impl AsRef<Path>) -> Result<PathBuf, LoopError> {
        let spec = self.resolve(name)?;
        let dest = dest.as_ref().to_path_buf();
        std::fs::write(&dest, spec.to_toml()?)?;
        Ok(dest)
    }

    /// Remove an installed loop file (built-ins always remain).
    pub fn remove(&self, name: &str) -> Result<(), LoopError> {
        let path = self.spec_path(&slug(name));
        if path.exists() {
            std::fs::remove_file(path)?;
            Ok(())
        } else {
            Err(LoopError::NotFound(name.to_string()))
        }
    }

    /// Persist a run journal (overwrites the loop's previous run).
    pub fn save_journal(&self, journal: &LoopJournal) -> Result<PathBuf, LoopError> {
        let path = self.journal_path(&slug(&journal.loop_name));
        std::fs::write(&path, serde_json::to_string_pretty(journal)?)?;
        Ok(path)
    }

    /// Load the latest run journal for a loop, if one exists.
    pub fn load_journal(&self, name: &str) -> Option<LoopJournal> {
        let path = self.journal_path(&slug(name));
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// All available loops (built-ins + installed files), built-ins first,
    /// installed files overriding built-ins of the same slug.
    pub fn list(&self) -> Vec<LoopSummary> {
        let mut out: Vec<LoopSummary> = Vec::new();
        let mut seen: Vec<String> = Vec::new();

        for spec in builtins::all() {
            let sl = slug(&spec.name);
            let installed = self.spec_path(&sl).exists();
            seen.push(sl.clone());
            let verify = spec.gate_summary();
            out.push(LoopSummary {
                name: spec.name,
                description: spec.description,
                verify,
                builtin: true,
                installed,
                slug: sl,
            });
        }

        if let Ok(entries) = std::fs::read_dir(&self.root) {
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
                if let Ok(spec) =
                    LoopSpec::from_toml_str(&std::fs::read_to_string(&path).unwrap_or_default())
                {
                    let verify = spec.gate_summary();
                    out.push(LoopSummary {
                        name: spec.name,
                        description: spec.description,
                        verify,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Attempt, VerifyOutcome};
    use crate::runner::StopReason;

    fn store() -> (tempfile::TempDir, LoopStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = LoopStore::with_root(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn resolves_builtin_default() {
        let (_d, store) = store();
        let spec = store.resolve("default").unwrap();
        assert_eq!(spec.name, "default");
        assert!(spec.resolved_gates().iter().all(|g| g.verify.is_command()));
    }

    #[test]
    fn save_resolve_and_remove_user_loop() {
        let (_d, store) = store();
        let spec = LoopSpec::from_goal("ship it");
        let mut spec = spec;
        spec.name = "My Loop".into();
        store.save(&spec).unwrap();

        assert_eq!(store.resolve("my loop").unwrap().goal, "ship it");
        assert!(store
            .list()
            .iter()
            .any(|s| s.slug == "my-loop" && !s.builtin));

        store.remove("My Loop").unwrap();
        assert!(store.resolve("My Loop").is_err());
    }

    #[test]
    fn installed_file_overrides_builtin_of_same_slug() {
        let (_d, store) = store();
        let mut spec = builtins::default_coding_loop();
        spec.max_iterations = 99;
        store.save(&spec).unwrap();
        assert_eq!(store.resolve("default").unwrap().max_iterations, 99);
        // Still listed once, now marked installed.
        let defaults: Vec<_> = store
            .list()
            .into_iter()
            .filter(|s| s.slug == "default")
            .collect();
        assert_eq!(defaults.len(), 1);
        assert!(defaults[0].installed);
    }

    #[test]
    fn journal_persists_and_reloads() {
        let (_d, store) = store();
        let mut j = LoopJournal::new("default", "be green");
        j.record(Attempt {
            iteration: 1,
            summary: "did a thing".into(),
            verify: VerifyOutcome::Failed {
                detail: "nope".into(),
            },
            gates: Vec::new(),
        });
        j.finish(StopReason::MaxIterations);
        store.save_journal(&j).unwrap();

        let back = store.load_journal("default").unwrap();
        assert_eq!(back, j);
        assert!(store.load_journal("never-run").is_none());
    }
}
