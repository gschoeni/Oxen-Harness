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

use crate::state::{active_root, open_history_store, AppState};

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct ProjectsConfig {
    #[serde(default)]
    pub(crate) paths: Vec<String>,
    #[serde(default)]
    pub(crate) active: Option<String>,
}

/// A project shown in the UI: its directory, display name, chat count, whether
/// it's the active one.
#[derive(Clone, Serialize)]
pub(crate) struct ProjectView {
    path: String,
    name: String,
    description: String,
    instructions: String,
    context: Vec<ProjectContext>,
    session_count: usize,
    active: bool,
}

/// Schema version for `projects.json` (bump when the shape changes).
const PROJECTS_SCHEMA_VERSION: u32 = 1;

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

/// Record `path` as a known project and make it active (persisted).
pub(crate) fn remember_project(path: &str) -> Result<(), String> {
    let mut cfg = read_projects_config();
    if !cfg.paths.iter().any(|p| p == path) {
        cfg.paths.push(path.to_string());
    }
    cfg.active = Some(path.to_string());
    write_projects_config(&cfg)
}

fn project_view(
    path: String,
    session_count: usize,
    active: bool,
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
    }
}

/// List known projects — the persisted set unioned with every directory that
/// already has chats — with chat counts and the active one flagged.
#[tauri::command]
pub(crate) async fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectView>, String> {
    let active = active_root(&state).await.display().to_string();

    // Chats per workspace, so each directory with history shows up as a project.
    let mut counts: HashMap<String, usize> = HashMap::new();
    if let Ok(store) = open_history_store() {
        if let Ok(sessions) = store.list_sessions() {
            for s in sessions {
                *counts.entry(s.workspace).or_default() += 1;
            }
        }
    }

    let mut paths = read_projects_config().paths;
    for k in counts.keys() {
        if !paths.contains(k) {
            paths.push(k.clone());
        }
    }
    if !paths.contains(&active) {
        paths.push(active.clone());
    }

    let mut projects: Vec<ProjectView> = paths
        .into_iter()
        .map(|p| {
            let count = counts.get(&p).copied().unwrap_or(0);
            let is_active = p == active;
            project_view(p, count, is_active)
        })
        .collect();
    // Active first, then busiest, then alphabetical.
    projects.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.session_count.cmp(&a.session_count))
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
    Ok(project_view(canonical, 0, true))
}

/// Create a project folder or adopt an existing one, persist its repo-local
/// metadata/context, and make it the active project for the next chat.
#[tauri::command]
pub(crate) async fn start_project(
    state: State<'_, AppState>,
    name: String,
    description: String,
    instructions: String,
    directory: String,
    create_directory: bool,
    context_paths: Vec<String>,
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
            format!("could not create project folder {}: {error}", root.display())
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
            instructions,
            context: existing.context,
        };
        project::save(&canonical, &config).map_err(|error| error.to_string())?;
        let sources = context_paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
        if !sources.is_empty() {
            project::add_context(&canonical, &sources).map_err(|error| error.to_string())?;
        }
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
    Ok(project_view(canonical, 0, true))
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
    Ok(project_view(path, 0, false))
}

/// Copy files into the project's durable context directory and return the
/// refreshed project metadata.
#[tauri::command]
pub(crate) async fn add_project_context(
    path: String,
    context_paths: Vec<String>,
) -> Result<ProjectView, String> {
    let root = PathBuf::from(&path);
    let sources = context_paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
    project::add_context(&root, &sources).map_err(|error| error.to_string())?;
    Ok(project_view(path, 0, false))
}

/// Remove one manifest entry and its repository-local context copy.
#[tauri::command]
pub(crate) async fn remove_project_context(
    path: String,
    context_path: String,
) -> Result<ProjectView, String> {
    let root = PathBuf::from(&path);
    project::remove_context(&root, &context_path).map_err(|error| error.to_string())?;
    Ok(project_view(path, 0, false))
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

    #[test]
    fn new_project_names_are_single_safe_path_components() {
        assert!(valid_folder_name("Demo App"));
        assert!(valid_folder_name("demo-app"));
        assert!(!valid_folder_name("../demo"));
        assert!(!valid_folder_name("nested/demo"));
        assert!(!valid_folder_name("."));
    }
}
