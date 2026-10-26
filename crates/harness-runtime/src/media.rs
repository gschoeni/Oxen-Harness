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
/// with the hub repository resolved for the project ([`destination_for`]).
pub fn prefs_for(root: &std::path::Path) -> MediaPrefs {
    let mut prefs = load();
    prefs.hub_repo = destination_for(root).repo;
    prefs
}

/// Which setting decided where a project's generations go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationSource {
    /// The project's own media repository (`project.json` `media_repo`).
    Project,
    /// The project's remote repository.
    ProjectRemote,
    /// The global default (Settings → Media → Hub repo).
    Global,
    /// Nothing set: the hub's default, the account's `playground`.
    HubDefault,
}

impl DestinationSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::ProjectRemote => "project_remote",
            Self::Global => "global",
            Self::HubDefault => "hub_default",
        }
    }
}

/// Where generations made under a project go on the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// `namespace/name`; `None` for the hub's default.
    pub repo: Option<String>,
    pub source: DestinationSource,
}

/// Resolve a project's media destination, most specific first: the
/// project's media repository, its remote repository (a project that knows
/// where it lives keeps its generations there), the global default, else
/// the hub's own default.
pub fn destination_for(root: &std::path::Path) -> Destination {
    let project = crate::project::load(root);
    let well_formed =
        |repo: Option<String>| repo.filter(|r| harness_media::parse_destination(r).is_some());
    let candidates = [
        (well_formed(project.media_repo), DestinationSource::Project),
        (
            well_formed(project.remote_repo),
            DestinationSource::ProjectRemote,
        ),
        (well_formed(load().hub_repo), DestinationSource::Global),
    ];
    candidates
        .into_iter()
        .find_map(|(repo, source)| {
            repo.map(|repo| Destination {
                repo: Some(repo),
                source,
            })
        })
        .unwrap_or(Destination {
            repo: None,
            source: DestinationSource::HubDefault,
        })
}

/// Send the project's generations to `repo` (`namespace/name`) once the
/// hub confirms the key can write there or will create it, saved in the
/// hub's casing; `None` (or blank) clears the project's choice. The one
/// write path `/media-repo` and the desktop picker share.
pub async fn choose_destination(
    root: &std::path::Path,
    repo: Option<&str>,
    api: Option<&harness_media::MediaApi>,
) -> Result<Destination, RuntimeError> {
    let checked = match repo.map(str::trim).filter(|r| !r.is_empty()) {
        Some(raw) => {
            let api = api.ok_or_else(|| {
                RuntimeError::Invalid(
                    "set an Oxen API key (Settings → Connection) to choose a hub repository".into(),
                )
            })?;
            let check = harness_media::HubRepos::connect(&api.base_url, api.api_key.clone())
                .check_destination(raw)
                .await?;
            Some(format!("{}/{}", check.namespace, check.repo))
        }
        None => None,
    };
    crate::project::set_media_repo(root, checked.as_deref())?;
    Ok(destination_for(root))
}

/// The repository the hub falls back to when nothing is set: the key's
/// own `playground`.
pub async fn hub_default_repo(
    api: &harness_media::MediaApi,
) -> Result<String, harness_media::UploadError> {
    let user = harness_media::HubRepos::connect(&api.base_url, api.api_key.clone())
        .whoami()
        .await?;
    Ok(format!(
        "{user}/{}",
        harness_media::upload::DEFAULT_UPLOAD_REPO
    ))
}

/// A [`harness_media::HubTarget`] that re-reads the project's destination
/// for each generation, so switching it reaches chats already open.
pub fn destination_resolver(root: std::path::PathBuf) -> harness_media::HubTarget {
    std::sync::Arc::new(move || {
        destination_for(&root)
            .repo
            .as_deref()
            .and_then(harness_media::parse_destination)
    })
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

/// What a chat's session tree spent on media generation, per model, read
/// from the manifest of every workspace the tree ran in. Each manifest is
/// searched for every session in the tree, since a lane's generations may
/// land in its parent's project rather than its own worktree.
pub fn tree_spend<'a>(
    members: impl IntoIterator<Item = (&'a str, &'a std::path::Path)>,
) -> Vec<harness_media::MediaSpend> {
    let mut sessions: Vec<&str> = Vec::new();
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    for (id, workspace) in members {
        sessions.push(id);
        if workspace.as_os_str().is_empty() {
            continue;
        }
        let root = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let dir = load().output_dir_rel();
    let mut out: Vec<harness_media::MediaSpend> = Vec::new();
    for root in roots {
        for spend in harness_media::MediaLibrary::new(root, dir.clone()).spend(&sessions) {
            match out.iter_mut().find(|s| s.model == spend.model) {
                Some(row) => {
                    row.outputs += spend.outputs;
                    row.cost_usd += spend.cost_usd;
                }
                None => out.push(spend),
            }
        }
    }
    out
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

    #[test]
    fn the_projects_media_repo_wins_then_its_remote_then_the_global_default() {
        with_temp_home(|| {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            assert_eq!(
                destination_for(root),
                Destination {
                    repo: None,
                    source: DestinationSource::HubDefault
                }
            );

            let mut prefs = load();
            prefs.hub_repo = Some("ox/playground".into());
            save(&prefs).unwrap();
            assert_eq!(destination_for(root).source, DestinationSource::Global);

            crate::project::set_remote_repo(root, Some("ox/my-app")).unwrap();
            assert_eq!(
                destination_for(root),
                Destination {
                    repo: Some("ox/my-app".into()),
                    source: DestinationSource::ProjectRemote
                }
            );

            let resolve = destination_resolver(root.to_path_buf());
            crate::project::set_media_repo(root, Some(" studio/shots/ ")).unwrap();
            assert_eq!(
                destination_for(root),
                Destination {
                    repo: Some("studio/shots".into()),
                    source: DestinationSource::Project
                }
            );
            // A resolver made earlier sees the switch.
            assert_eq!(resolve(), Some(("studio".into(), "shots".into())));

            crate::project::set_media_repo(root, None).unwrap();
            assert_eq!(
                destination_for(root).source,
                DestinationSource::ProjectRemote
            );
            assert!(crate::project::set_media_repo(root, Some("no-slash")).is_err());
        });
    }

    #[test]
    fn tree_spend_reads_every_workspace_for_every_session_in_the_tree() {
        with_temp_home(|| {
            let project = tempfile::tempdir().unwrap();
            let worktree = tempfile::tempdir().unwrap();
            let row = |id: &str, session: &str, model: &str, cost: f64| {
                format!(
                    r#"{{"id":"{id}","session":"{session}","kind":"image","model":"{model}","prompt":"p","status":"succeeded","created_at":1,"cost_usd":{cost}}}"#
                )
            };
            let write = |root: &std::path::Path, rows: &[String]| {
                let dir = root.join(load().output_dir_rel());
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("manifest.jsonl"), rows.join("\n")).unwrap();
            };
            // The lane ran in a worktree but one of its generations landed in
            // the parent's project; a stranger's row is not this chat's bill.
            write(
                project.path(),
                &[
                    row("a", "root", "img", 0.25),
                    row("b", "lane", "img", 0.25),
                    row("c", "stranger", "img", 9.0),
                ],
            );
            write(worktree.path(), &[row("d", "lane", "vid", 1.0)]);

            let spend = tree_spend([
                ("root", project.path()),
                ("lane", worktree.path()),
                // A second spelling of the project is read once, not twice.
                ("leaf", &project.path().join(".")),
            ]);
            let find = |m: &str| spend.iter().find(|s| s.model == m).unwrap();
            assert_eq!(spend.len(), 2);
            assert_eq!(find("img").outputs, 2);
            assert!((find("img").cost_usd - 0.5).abs() < 1e-9);
            assert!((find("vid").cost_usd - 1.0).abs() < 1e-9);
        });
    }
}
