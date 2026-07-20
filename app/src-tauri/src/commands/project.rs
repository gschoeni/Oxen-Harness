//! Projects — chats are grouped by their working directory. A "project" is a
//! directory the agent runs in; entering one roots new chats there. The set of
//! known projects (plus the active one) is persisted to `projects.json`, and
//! merged with the distinct workspaces found across existing chats so directories
//! that already have history always show up.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use harness_runtime::project::{self, ProjectConfig, ProjectContext};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{open_history_store, AppState};

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProjectsConfig {
    #[serde(default)]
    pub(crate) paths: Vec<String>,
    #[serde(default)]
    pub(crate) active: Option<String>,
    #[serde(default)]
    pub(crate) default_location: Option<String>,
    /// Workspaces the user explicitly removed. Their chat history stays in the
    /// store, but they are kept out of the project list until re-added — without
    /// this, the history-derived union would resurrect them on the next load.
    #[serde(default)]
    pub(crate) removed: Vec<String>,
}

/// A project shown in the UI: its directory, display name, chat count, whether
/// it's the active one, and when it last saw activity.
#[derive(Clone, Serialize)]
pub(crate) struct ProjectView {
    path: String,
    name: String,
    description: String,
    instructions: String,
    context: Vec<ProjectContext>,
    session_count: usize,
    active: bool,
    /// Unix seconds of the newest message in any of this project's chats;
    /// `None` for projects with no history yet.
    last_used_at: Option<i64>,
}

/// Schema version for `projects.json` (bump when the shape changes).
const PROJECTS_SCHEMA_VERSION: u32 = 3;

fn projects_config_path() -> Result<PathBuf, String> {
    harness_config::paths::projects_file().map_err(|e| e.to_string())
}

pub(crate) fn read_projects_config() -> ProjectsConfig {
    match projects_config_path() {
        Ok(path) => harness_config::read_versioned::<ProjectsConfig>(&path).1,
        Err(_) => ProjectsConfig::default(),
    }
}

fn write_projects_config(cfg: &ProjectsConfig) -> Result<(), String> {
    let path = projects_config_path()?;
    harness_config::write_versioned(&path, PROJECTS_SCHEMA_VERSION, cfg)
        .map_err(|e| e.to_string())?;
    harness_runtime::config_repo::snapshot("Update projects");
    Ok(())
}

/// Record `path` as a known project and make it active (persisted). Re-adding a
/// previously removed path un-hides it.
pub(crate) fn remember_project(path: &str) -> Result<(), String> {
    let mut cfg = read_projects_config();
    if !cfg.paths.iter().any(|p| p == path) {
        cfg.paths.push(path.to_string());
    }
    cfg.removed.retain(|p| p != path);
    cfg.active = Some(path.to_string());
    write_projects_config(&cfg)
}

/// Forget `path` as a known project (persisted). Chat history is untouched —
/// the directory stays on disk, it just no longer surfaces as a project. The
/// path is recorded in `removed` so the history-derived union does not bring it
/// back on the next load.
pub(crate) fn forget_project(path: &str) -> Result<(), String> {
    let mut cfg = read_projects_config();
    cfg.paths.retain(|p| p != path);
    if !cfg.removed.iter().any(|p| p == path) {
        cfg.removed.push(path.to_string());
    }
    if cfg.active.as_deref() == Some(path) {
        cfg.active = None;
    }
    write_projects_config(&cfg)
}

/// Return the saved parent directory for new projects when it still exists.
#[tauri::command]
pub(crate) fn get_default_project_location() -> Option<String> {
    read_projects_config()
        .default_location
        .filter(|path| Path::new(path).is_dir())
}

/// Persist the parent directory prefilled by future project creation flows.
#[tauri::command]
pub(crate) fn set_default_project_location(path: String) -> Result<String, String> {
    let canonical = canonical_directory(&path)?;
    let mut config = read_projects_config();
    config.default_location = Some(canonical.clone());
    write_projects_config(&config)?;
    Ok(canonical)
}

fn canonical_directory(path: &str) -> Result<String, String> {
    let directory = PathBuf::from(path);
    if !directory.is_dir() {
        return Err(format!("project location does not exist: {path}"));
    }
    Ok(directory
        .canonicalize()
        .unwrap_or(directory)
        .display()
        .to_string())
}

fn project_view(
    path: String,
    session_count: usize,
    active: bool,
    last_used_at: Option<i64>,
) -> ProjectView {
    let metadata = project::load(Path::new(&path));
    ProjectView {
        path,
        name: metadata.name,
        description: metadata.description,
        instructions: metadata.instructions,
        context: metadata.context,
        session_count,
        active,
        last_used_at,
    }
}

/// List known projects — the persisted set unioned with every directory that
/// already has chats — with chat counts and the active one flagged.
#[tauri::command]
pub(crate) async fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectView>, String> {
    let active = state.active_root().await.display().to_string();

    // Chats per workspace, so each directory with history shows up as a
    // project. Native chats only: imported transcripts (Claude Code / Cursor)
    // carry their original cwd as workspace and would mint phantom projects —
    // the sidebar and chat lists hide them too (`source === ""`).
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut last_used: HashMap<String, i64> = HashMap::new();
    if let Ok(store) = open_history_store() {
        if let Ok(sessions) = store.list_sessions() {
            for s in sessions.into_iter().filter(|s| s.source.is_empty()) {
                *counts.entry(s.workspace).or_default() += 1;
            }
        }
        if let Ok(activity) = store.workspace_last_used() {
            last_used = activity;
        }
    }

    let cfg = read_projects_config();
    let paths = union_project_paths(&cfg, &active, counts.keys());

    let mut projects: Vec<ProjectView> = paths
        .into_iter()
        .map(|p| {
            let count = counts.get(&p).copied().unwrap_or(0);
            let is_active = p == active;
            let used = last_used.get(&p).copied();
            project_view(p, count, is_active, used)
        })
        .collect();
    // Most recently used first (projects without history last), then
    // alphabetical. The frontend offers its own sort control on top of this.
    projects.sort_by(|a, b| {
        b.last_used_at
            .cmp(&a.last_used_at)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(projects)
}

/// Add a directory as a project and make it active. New chats root here.
#[tauri::command]
pub(crate) async fn open_project(
    state: State<'_, AppState>,
    path: String,
) -> Result<ProjectView, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    let canonical = dir
        .canonicalize()
        .map(|c| c.display().to_string())
        .unwrap_or(path);
    remember_project(&canonical)?;
    *state.active_project.lock().await = PathBuf::from(&canonical);
    Ok(project_view(canonical, 0, true, None))
}

/// Create a project folder or adopt an existing one, persist its repo-local
/// identity, and make it the active project for the next chat.
#[tauri::command]
pub(crate) async fn start_project(
    state: State<'_, AppState>,
    name: String,
    description: String,
    directory: String,
    create_directory: bool,
) -> Result<ProjectView, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("project name is required".into());
    }
    let chosen = PathBuf::from(&directory);
    let root = if create_directory {
        if !chosen.is_dir() {
            return Err(format!("parent folder does not exist: {directory}"));
        }
        if !valid_folder_name(name) {
            return Err("project name cannot contain path separators".into());
        }
        let root = chosen.join(name);
        if root.exists() {
            return Err(format!(
                "{} already exists; choose it as an existing project instead",
                root.display()
            ));
        }
        std::fs::create_dir(&root).map_err(|error| {
            format!(
                "could not create project folder {}: {error}",
                root.display()
            )
        })?;
        root
    } else {
        if !chosen.is_dir() {
            return Err(format!("project folder does not exist: {directory}"));
        }
        chosen
    };
    let created_root = create_directory.then(|| root.clone());
    let canonical = root.canonicalize().unwrap_or(root);
    let setup = (|| -> Result<(), String> {
        let existing = project::load(&canonical);
        let config = ProjectConfig {
            name: name.to_string(),
            description,
            instructions: existing.instructions,
            context: existing.context,
        };
        project::save(&canonical, &config).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if let Err(error) = setup {
        if let Some(created_root) = created_root {
            if let Err(cleanup_error) = std::fs::remove_dir_all(&created_root) {
                return Err(format!(
                    "{error}; could not remove incomplete project folder {}: {cleanup_error}",
                    created_root.display()
                ));
            }
        }
        return Err(error);
    }
    let canonical = canonical.display().to_string();
    remember_project(&canonical)?;
    *state.active_project.lock().await = PathBuf::from(&canonical);
    Ok(project_view(canonical, 0, true, None))
}

/// Edit durable project metadata. The frontend starts a fresh chat afterward
/// so the updated prompt takes effect without mutating existing transcripts.
#[tauri::command]
pub(crate) async fn update_project(
    path: String,
    name: String,
    description: String,
    instructions: String,
) -> Result<ProjectView, String> {
    let root = PathBuf::from(&path);
    let existing = project::load(&root);
    let config = ProjectConfig {
        name,
        description,
        instructions,
        context: existing.context,
    };
    project::save(&root, &config).map_err(|error| error.to_string())?;
    Ok(project_view(path, 0, false, None))
}

/// Remove a project from the known set without touching its files on disk.
/// Chat history in the store is left intact; the directory simply stops being
/// listed as a project. If it was active, the active project is cleared.
#[tauri::command]
pub(crate) async fn delete_project(state: State<'_, AppState>, path: String) -> Result<(), String> {
    forget_project(&path)?;
    if state.active_root().await == Path::new(&path) {
        *state.active_project.lock().await = PathBuf::new();
    }
    Ok(())
}

/// Copy files into the project's durable context directory and return the
/// refreshed project metadata.
#[tauri::command]
pub(crate) async fn add_project_context(
    path: String,
    context_paths: Vec<String>,
) -> Result<ProjectView, String> {
    let root = PathBuf::from(&path);
    let sources = context_paths
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    project::add_context(&root, &sources).map_err(|error| error.to_string())?;
    Ok(project_view(path, 0, false, None))
}

/// Remove one manifest entry and its repository-local context copy.
#[tauri::command]
pub(crate) async fn remove_project_context(
    path: String,
    context_path: String,
) -> Result<ProjectView, String> {
    let root = PathBuf::from(&path);
    project::remove_context(&root, &context_path).map_err(|error| error.to_string())?;
    Ok(project_view(path, 0, false, None))
}

/// The project list: persisted paths unioned with directories that have chat
/// history and the active one — minus any path the user explicitly removed.
fn union_project_paths<'a>(
    cfg: &ProjectsConfig,
    active: &str,
    history: impl Iterator<Item = &'a String>,
) -> Vec<String> {
    let mut paths = cfg.paths.clone();
    let keep = |p: &str| !cfg.removed.iter().any(|r| r == p);
    for k in history {
        if !paths.contains(k) && keep(k) {
            paths.push(k.clone());
        }
    }
    let active = active.to_string();
    if !paths.contains(&active) && keep(&active) {
        paths.push(active);
    }
    paths
}

fn valid_folder_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// Switch the active project to an already-known directory.
#[tauri::command]
pub(crate) async fn set_active_project(
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    *state.active_project.lock().await = PathBuf::from(&path);
    remember_project(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn history_workspaces_union_in_unless_removed() {
        let cfg = ProjectsConfig {
            paths: strings(&["/a"]),
            removed: strings(&["/b"]),
            ..Default::default()
        };
        let history = strings(&["/b", "/c"]);
        // /b has history but was removed → hidden; /c has history → surfaces.
        let paths = union_project_paths(&cfg, "/active", history.iter());
        assert_eq!(paths, strings(&["/a", "/c", "/active"]));
    }

    #[test]
    fn a_removed_active_project_is_not_resurrected() {
        let cfg = ProjectsConfig {
            paths: strings(&[]),
            active: None,
            removed: strings(&["/gone"]),
            ..Default::default()
        };
        let history = strings(&["/gone"]);
        let paths = union_project_paths(&cfg, "/gone", history.iter());
        assert!(paths.is_empty());
    }

    #[test]
    fn new_project_names_are_single_safe_path_components() {
        assert!(valid_folder_name("Demo App"));
        assert!(valid_folder_name("demo-app"));
        assert!(!valid_folder_name("../demo"));
        assert!(!valid_folder_name("nested/demo"));
        assert!(!valid_folder_name("."));
    }

    #[test]
    fn project_location_config_is_backward_compatible_and_requires_a_directory() {
        let legacy: ProjectsConfig = serde_json::from_value(serde_json::json!({
            "paths": ["/work/demo"],
            "active": "/work/demo"
        }))
        .unwrap();
        assert_eq!(legacy.default_location, None);

        let tmp = std::env::temp_dir().join(format!(
            "oxen-harness-project-location-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir(&tmp).unwrap();
        assert_eq!(
            canonical_directory(&tmp.display().to_string()).unwrap(),
            tmp.canonicalize().unwrap().display().to_string()
        );
        assert!(canonical_directory(&tmp.join("missing").display().to_string()).is_err());
        std::fs::remove_dir(&tmp).unwrap();
    }
}
