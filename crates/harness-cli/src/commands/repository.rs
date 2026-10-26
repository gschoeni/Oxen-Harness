//! `/repository` — see and switch the Oxen repository this project's assets
//! go to on the hub (today: its generated images and videos).
//!
//! - `/repository` shows it, then picks a namespace and a repository from
//!   what the API key can write to (typing `✎` a new name works where the
//!   hub will create it).
//! - `/repository <namespace/name>` checks it against the hub and switches.
//! - `/repository clear` drops the project's choice, falling back to the
//!   project remote, the global default, or the account's `playground`.
//!
//! The switch reaches the open chat's next generation; see
//! `harness_runtime::media::destination_resolver`.

use std::path::Path;

use anyhow::Result;
use harness_media::{HubNamespace, NamespaceKind};
use harness_runtime::media::{Destination, DestinationSource};

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// The row that clears the project's choice.
const DEFAULT_ROW: &str = "default";

/// `root` is the session's workspace (where its media tools read the
/// choice back), and `hub_base_url` the endpoint they generate on — both
/// from the running session, never the shell's cwd or the saved host.
pub(crate) async fn handle_repl(
    rest: Option<String>,
    root: &Path,
    hub_base_url: &str,
    ui: &Ui,
) -> Result<()> {
    let api = harness_runtime::media::api_for(hub_base_url);
    match rest.as_deref().map(str::trim).unwrap_or("") {
        "" => pick(root, api.as_ref(), ui).await,
        "clear" | DEFAULT_ROW => switch(root, None, api.as_ref(), ui).await,
        repo => switch(root, Some(repo), api.as_ref(), ui).await,
    }
}

async fn pick(root: &Path, api: Option<&harness_media::MediaApi>, ui: &Ui) -> Result<()> {
    show(&harness_runtime::media::destination_for(root), api, ui).await;
    let Some(api) = api else {
        println!(
            "  {}",
            ui.dim("Set an Oxen API key (/auth) to choose a hub repository.")
        );
        return Ok(());
    };
    let namespaces = match harness_media::HubRepos::connect(&api.base_url, api.api_key.clone())
        .destinations()
        .await
    {
        Ok(namespaces) => namespaces,
        Err(e) => {
            println!(
                "  {}",
                ui.dim(&format!("could not list hub repositories: {e}"))
            );
            return Ok(());
        }
    };

    let mut options = vec![Choice::new(
        DEFAULT_ROW,
        "clear this project's choice (project remote, global default, or your playground)",
    )];
    options.extend(
        namespaces
            .iter()
            .map(|ns| Choice::new(&ns.name, describe(ns))),
    );
    let Some(namespace) = first(picker::select(
        ui,
        "Repository",
        "Send this project's generations to which namespace? (or ✎ type namespace/name)",
        &options,
        false,
    )?) else {
        return Ok(());
    };
    if namespace == DEFAULT_ROW {
        return switch(root, None, Some(api), ui).await;
    }
    // A typed `namespace/name` skips the second step.
    let Some(ns) = namespaces.iter().find(|ns| ns.name == namespace) else {
        return switch(root, Some(&namespace), Some(api), ui).await;
    };

    let repos: Vec<Choice> = ns
        .repos
        .iter()
        .map(|repo| Choice::new(repo, format!("{}/{repo}", ns.name)))
        .collect();
    let question = if ns.can_create {
        format!(
            "Which repository in {}? (✎ type a new name to create it)",
            ns.name
        )
    } else {
        format!("Which repository in {}?", ns.name)
    };
    let Some(repo) = first(picker::select(ui, "Repository", &question, &repos, false)?) else {
        return Ok(());
    };
    let repo = repo.trim().trim_matches('/');
    let full = if repo.contains('/') {
        repo.to_string()
    } else {
        format!("{}/{repo}", ns.name)
    };
    switch(root, Some(&full), Some(api), ui).await
}

/// Check `repo` against the hub (when given), save it, and say where
/// generations go now. A refusal is printed, not raised: it's an answer.
async fn switch(
    root: &Path,
    repo: Option<&str>,
    api: Option<&harness_media::MediaApi>,
    ui: &Ui,
) -> Result<()> {
    match harness_runtime::media::choose_destination(root, repo, api).await {
        Ok(destination) => {
            print!("  {} ", ui.green("✓"));
            show(&destination, api, ui).await;
        }
        Err(e) => println!("  {} {}", ui.brown("✗"), ui.dim(&e.to_string())),
    }
    Ok(())
}

async fn show(destination: &Destination, api: Option<&harness_media::MediaApi>, ui: &Ui) {
    // Nothing set: name the account's playground, or say why it can't be.
    let repo = match &destination.repo {
        Some(repo) => repo.clone(),
        None => match api {
            Some(api) => match harness_runtime::media::hub_default_repo(api).await {
                Ok(repo) => repo,
                Err(e) => format!("your playground (couldn't ask Oxen.ai who you are: {e})"),
            },
            None => "your playground (no API key — /auth sets one)".into(),
        },
    };
    println!(
        "  {} {}  {}",
        ui.brown("Generations go to"),
        ui.cream(&repo),
        ui.dim(&format!("({})", source_label(destination.source)))
    );
}

fn source_label(source: DestinationSource) -> &'static str {
    match source {
        DestinationSource::Project => "picked for this project",
        DestinationSource::ProjectRemote => "the project's remote repository",
        DestinationSource::Global => "the global default in settings",
        DestinationSource::HubDefault => "the hub's default",
    }
}

fn describe(ns: &HubNamespace) -> String {
    let count = match ns.repos.len() {
        1 => "1 repository".to_string(),
        n => format!("{n} repositories"),
    };
    match ns.kind {
        NamespaceKind::User => format!("your account · {count}"),
        NamespaceKind::Org => format!(
            "organization ({}) · {count}",
            ns.role.as_deref().unwrap_or("member")
        ),
        NamespaceKind::Shared => format!("shared with you · {count}"),
    }
}

fn first(picked: Option<Vec<String>>) -> Option<String> {
    picked.and_then(|p| p.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_each_kind_of_namespace() {
        let ns = |kind, role: Option<&str>, repos: &[&str]| HubNamespace {
            name: "x".into(),
            kind,
            role: role.map(str::to_string),
            can_create: kind != NamespaceKind::Shared,
            repos: repos.iter().map(|r| r.to_string()).collect(),
        };
        assert_eq!(
            describe(&ns(NamespaceKind::User, None, &["a"])),
            "your account · 1 repository"
        );
        assert_eq!(
            describe(&ns(NamespaceKind::Org, Some("editor"), &["a", "b"])),
            "organization (editor) · 2 repositories"
        );
        assert_eq!(
            describe(&ns(NamespaceKind::Shared, None, &[])),
            "shared with you · 0 repositories"
        );
    }
}
