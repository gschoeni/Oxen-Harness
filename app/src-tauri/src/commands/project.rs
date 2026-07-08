//! Projects — chats are grouped by their working directory. A "project" is a
//! directory the agent runs in; entering one roots new chats there. The set of
//! known projects (plus the active one) is persisted to `projects.json`, and
//! merged with the distinct workspaces found across existing chats so directories
//! that already have history always show up.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::{active_root, open_history_store, AppState};

/// A friendly display name for a project directory (its last path segment).
fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}

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
        .map(|p| ProjectView {
            name: project_name(&p),
            session_count: counts.get(&p).copied().unwrap_or(0),
            active: p == active,
            path: p,
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
    Ok(ProjectView {
        name: project_name(&canonical),
        session_count: 0,
        active: true,
        path: canonical,
    })
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
