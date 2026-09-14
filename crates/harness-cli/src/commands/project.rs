//! `oxen-harness project` — inspect and set a project's durable metadata from
//! the shell: today its remote Oxen repository (`namespace/name` on the hub),
//! the same setting the desktop's project page edits and the agent's
//! `create_repository` tool fills in. Reads and writes
//! `<project>/.oxen-harness/project.json`; the project is the current
//! directory unless `--path` says otherwise.

use std::path::PathBuf;

use anyhow::{Context, Result};
use harness_runtime::project;

use crate::theme::Ui;

#[derive(Debug, clap::Subcommand)]
pub(crate) enum ProjectAction {
    /// Show the project's name, goal, and remote repository.
    Show {
        /// The project directory (defaults to the current one).
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
    },
    /// Set the project's remote Oxen repository (`namespace/name`).
    SetRepo {
        /// The repository on the hub, e.g. `ox/my-app`.
        repo: String,
        /// The project directory (defaults to the current one).
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
    },
    /// Forget the project's remote Oxen repository.
    ClearRepo {
        /// The project directory (defaults to the current one).
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
    },
}

pub(crate) fn run_project(action: ProjectAction, ui: &Ui) -> Result<()> {
    match action {
        ProjectAction::Show { path } => {
            let root = resolve_root(path)?;
            let config = project::load(&root);
            println!("  {} {}", ui.dim("project"), config.name);
            println!("  {} {}", ui.dim("folder "), root.display());
            if !config.description.is_empty() {
                println!("  {} {}", ui.dim("goal   "), config.description);
            }
            match &config.remote_repo {
                Some(repo) => println!("  {} {} {}", ui.dim("remote "), repo, ui.dim(&remote_url(repo))),
                None => println!(
                    "  {} {}",
                    ui.dim("remote "),
                    ui.dim("none — set one with: oxen-harness project set-repo <namespace/name>")
                ),
            }
        }
        ProjectAction::SetRepo { repo, path } => {
            let root = resolve_root(path)?;
            let saved = project::set_remote_repo(&root, Some(&repo))?;
            let repo = saved.remote_repo.unwrap_or(repo);
            println!(
                "  {} {}",
                ui.green(&format!("✓ remote set to {repo}")),
                ui.dim(&remote_url(&repo))
            );
        }
        ProjectAction::ClearRepo { path } => {
            let root = resolve_root(path)?;
            let had = project::load(&root).remote_repo;
            project::set_remote_repo(&root, None)?;
            match had {
                Some(repo) => println!("  {}", ui.green(&format!("✓ remote {repo} cleared"))),
                None => println!("  {}", ui.dim("no remote was set")),
            }
        }
    }
    Ok(())
}

/// The project directory: `--path`, else the current directory. Must exist.
fn resolve_root(path: Option<PathBuf>) -> Result<PathBuf> {
    let root = match path {
        Some(p) => p,
        None => std::env::current_dir().context("could not read the current directory")?,
    };
    anyhow::ensure!(root.is_dir(), "project folder does not exist: {}", root.display());
    Ok(root.canonicalize().unwrap_or(root))
}

/// The repository's page on the configured hub.
fn remote_url(repo: &str) -> String {
    let cfg = harness_runtime::connection::load();
    let base = harness_runtime::connection::effective_base_url(&cfg);
    match repo.split_once('/') {
        Some((ns, name)) => harness_media::repo_web_url(&base, ns, name),
        None => String::new(),
    }
}
