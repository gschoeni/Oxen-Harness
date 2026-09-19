//! Image/video generation preferences, shared by the CLI and desktop app.
//!
//! Persisted to `~/.oxen-harness/media.json` (versioned, no secrets): the
//! default image and video models, the project-relative output folder, and
//! the two spend limits (per generation, per run) under which a generation
//! runs without asking. Read when an agent is built, so a change takes
//! effect for new (and resumed) chats rather than the live one.

use harness_config::paths;
pub use harness_media::MediaPrefs;

use crate::RuntimeError;

/// Schema version for `media.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// Read the saved preferences (defaults on a fresh install / unreadable file).
pub fn load() -> MediaPrefs {
    crate::config::load_or_default(paths::media_file())
}

/// The preferences a session under `root` runs with: the saved global ones,
/// with the project's own remote repository (when it has one) standing in
/// for the global "hub repo" — a project that knows where it lives keeps
/// its generations there, not in a shared playground.
pub fn prefs_for(root: &std::path::Path) -> MediaPrefs {
    let mut prefs = load();
    if let Some(repo) = crate::project::load(root).remote_repo {
        prefs.hub_repo = Some(repo);
    }
    prefs
}

/// Atomically persist the preferences and snapshot the config repo.
pub fn save(prefs: &MediaPrefs) -> Result<(), RuntimeError> {
    crate::config::write_and_snapshot(
        &paths::media_file()?,
        SCHEMA_VERSION,
        prefs,
        "Update media generation preferences",
    )
}

/// The hub handle the media tools call with: the connection's base URL and
/// the one API key the user configured for the harness (`None` when no key
/// resolves — the tools then answer with their "needs a key" sentence).
pub fn api() -> Option<harness_media::MediaApi> {
    let cfg = crate::connection::load();
    api_for(&crate::connection::effective_base_url(&cfg))
}

/// [`api`] for an explicit base URL — the CLI's `--base-url`/`--host`
/// override, so media generation talks to the same endpoint as the chat.
pub fn api_for(base_url: &str) -> Option<harness_media::MediaApi> {
    let api_key = crate::connection::effective_api_key(base_url);
    (!api_key.is_empty()).then_some(harness_media::MediaApi {
        base_url: base_url.to_string(),
        api_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_temp_home;

    #[test]
    fn defaults_then_round_trip() {
        with_temp_home(|| {
            let fresh = load();
            assert_eq!(fresh, MediaPrefs::default());
            assert_eq!(fresh.per_run_usd, Some(harness_media::DEFAULT_PER_RUN_USD));

            let mut prefs = fresh.clone();
            prefs.default_image_model = "nano-banana-2".into();
            prefs.output_dir = "art".into();
            prefs.per_generation_usd = None;
            prefs.per_run_usd = Some(3.5);
            save(&prefs).unwrap();

            let loaded = load();
            assert_eq!(loaded, prefs);
            assert_eq!(loaded.per_generation_usd, None);
        });
    }

    #[test]
    fn a_projects_remote_repo_overrides_the_global_hub_repo() {
        with_temp_home(|| {
            let mut prefs = load();
            prefs.hub_repo = Some("ox/playground".into());
            save(&prefs).unwrap();

            let tmp = tempfile::tempdir().unwrap();
            // No project remote → the global one.
            assert_eq!(
                prefs_for(tmp.path()).hub_repo.as_deref(),
                Some("ox/playground")
            );
            crate::project::set_remote_repo(tmp.path(), Some("ox/my-app")).unwrap();
            assert_eq!(prefs_for(tmp.path()).hub_repo.as_deref(), Some("ox/my-app"));
            assert_eq!(
                prefs_for(tmp.path()).hub_target(),
                Some(("ox".into(), "my-app".into()))
            );
        });
    }
}
