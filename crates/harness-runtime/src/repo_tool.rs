//! `create_repository` — give the project a remote Oxen repository on the
//! hub and make it the project's default (see
//! [`crate::project::ProjectConfig::remote_repo`]).
//!
//! The tool never creates silently: it resolves the namespace, checks
//! whether `namespace/name` already exists, and asks the user through the
//! host's question picker (the same bridge `ask_user_question` uses) before
//! creating or adopting anything. With no interactive user (a subagent, a
//! piped session) it declines and says how to set the remote by hand.
//!
//! On success the remote lands in `<root>/.oxen-harness/project.json`, so
//! the desktop settings page, `oxen-harness project show`, and the media
//! tools all see it; when the project folder is itself an Oxen repo and the
//! `oxen` CLI is installed, its `origin` remote is pointed at the new
//! repository too (best-effort — a failure is reported, not fatal).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use harness_media::{repo_web_url, HubRepos, MediaApi, NewRepo};
use harness_tools::{Choice, Concurrency, Question, QuestionAsker, ToolError, TypedTool};
use serde::Deserialize;

use crate::project;

pub const CREATE_REPOSITORY_TOOL: &str = "create_repository";

/// What the model says when there is no key to talk to the hub with.
pub const REPO_NO_KEY: &str = "Creating a repository needs your Oxen API key. \
    Add it under Settings → Connection (desktop) or set OXEN_API_KEY, then try again.";

/// Arguments to `create_repository`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateRepositoryArgs {
    /// Repository name (letters, digits, `-`, `_`, `.`), e.g. `my-app`.
    /// Defaults to the project folder's name.
    #[serde(default)]
    pub name: Option<String>,
    /// The hub namespace (a user or an organization the user belongs to)
    /// to create it under, e.g. `ox`. Defaults to the API key's own user.
    #[serde(default)]
    pub namespace: Option<String>,
    /// One-line description for the repository page.
    #[serde(default)]
    pub description: Option<String>,
    /// Make the repository public. Defaults to private.
    #[serde(default)]
    pub public: Option<bool>,
}

/// The model-facing tool. One per session, built by the host.
pub struct CreateRepositoryTool {
    root: PathBuf,
    /// `None` when no API key resolves → the tool answers [`REPO_NO_KEY`].
    api: Option<MediaApi>,
    asker: Arc<dyn QuestionAsker>,
    /// How to reach the hub for a given API handle — swapped in tests.
    repos: Arc<dyn Fn(&MediaApi) -> HubRepos + Send + Sync>,
}

impl CreateRepositoryTool {
    pub fn new(root: impl Into<PathBuf>, api: Option<MediaApi>, asker: Arc<dyn QuestionAsker>) -> Self {
        Self {
            root: root.into(),
            api,
            asker,
            repos: Arc::new(|api| HubRepos::connect(&api.base_url, api.api_key.clone())),
        }
    }

    /// Route hub calls elsewhere (a mock server in tests).
    pub fn with_hub(mut self, repos: impl Fn(&MediaApi) -> HubRepos + Send + Sync + 'static) -> Self {
        self.repos = Arc::new(repos);
        self
    }

    /// Ask for a go-ahead. `Ok(None)` = nobody to ask.
    async fn confirm(&self, question: String, go: &str, go_detail: String) -> Result<Option<bool>, ToolError> {
        let q = Question {
            question,
            header: "Repository".to_string(),
            options: vec![
                Choice {
                    label: go.to_string(),
                    description: go_detail,
                },
                Choice {
                    label: "Cancel".to_string(),
                    description: "Leave the project without a remote repository".to_string(),
                },
            ],
            multi_select: false,
        };
        let Some(answers) = self.asker.ask(std::slice::from_ref(&q)).await? else {
            return Ok(None);
        };
        let picked = answers
            .first()
            .and_then(|a| a.selected.first())
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        Ok(Some(
            picked == go.to_ascii_lowercase() || matches!(picked.as_str(), "yes" | "y" | "ok"),
        ))
    }
}

#[async_trait]
impl TypedTool for CreateRepositoryTool {
    const NAME: &'static str = CREATE_REPOSITORY_TOOL;
    type Args = CreateRepositoryArgs;

    fn description(&self) -> &str {
        "Create a remote Oxen repository for this project on the hub (or adopt one that already \
         exists) and make it the project's default repository. Asks the user to confirm before \
         creating anything. Use it when the user wants somewhere to push the project, share \
         generated media, or asks to set up / connect a repository. Afterwards the project's \
         remote is `namespace/name`; generated images and videos keep copies there."
    }

    fn concurrency(&self) -> Concurrency {
        Concurrency::Exclusive
    }

    async fn run(&self, args: Self::Args) -> Result<String, ToolError> {
        let Some(api) = &self.api else {
            return Ok(REPO_NO_KEY.to_string());
        };
        let hub = (self.repos)(api);

        let name = match args.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.to_string(),
            None => folder_slug(&self.root),
        };
        let namespace = match args.namespace.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(ns) => ns.to_string(),
            None => hub.whoami().await.map_err(hub_err)?,
        };
        let full = format!("{namespace}/{name}");
        let Some((namespace, name)) = project::parse_remote_repo(&full) else {
            return Ok(format!(
                "`{full}` isn't a valid repository name: use letters, digits, `-`, `_`, or `.` \
                 for both the namespace and the name."
            ));
        };
        let url = repo_web_url(&api.base_url, &namespace, &name);
        let current = project::load(&self.root).remote_repo;
        let replacing = current
            .as_deref()
            .filter(|c| *c != full)
            .map(|c| format!(" It replaces the current default `{c}`."))
            .unwrap_or_default();

        let exists = hub.repo_exists(&namespace, &name).await.map_err(hub_err)?;
        let public = args.public.unwrap_or(false);
        let (question, go, detail) = if exists {
            (
                format!("`{full}` already exists on the hub. Use it as this project's repository?{replacing}"),
                "Use it",
                format!("Set {url} as the project's default repository"),
            )
        } else {
            (
                format!(
                    "Create the {} repository `{full}` on the hub and make it this project's repository?{replacing}",
                    if public { "public" } else { "private" }
                ),
                "Create",
                format!("Create {url} and set it as the project's default"),
            )
        };
        match self.confirm(question, go, detail).await? {
            Some(true) => {}
            Some(false) => {
                return Ok(format!(
                    "The user declined; `{full}` was not {} and the project's repository is unchanged.",
                    if exists { "adopted" } else { "created" }
                ))
            }
            None => {
                return Ok(format!(
                    "No interactive user is available to confirm, so nothing was created. The user can \
                     set the project's repository with `oxen-harness project set-repo {full}` or on the \
                     project's settings page."
                ))
            }
        }

        if !exists {
            let description = args
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("{} — managed with oxen-harness.", project::load(&self.root).name));
            hub.create_repo(NewRepo {
                name: &name,
                namespace: Some(&namespace),
                description: &description,
                public,
                workbench: false,
            })
            .await
            .map_err(hub_err)?;
        }
        project::set_remote_repo(&self.root, Some(&full))
            .map_err(|e| ToolError::Execution(format!("could not save the project's repository: {e}")))?;

        let mut out = format!(
            "{} {url}{} and set it as this project's default repository (`{full}`).",
            if exists { "Adopted" } else { "Created" },
            if exists { "" } else if public { " (public)" } else { " (private)" },
        );
        out.push_str(&point_oxen_remote(&self.root, &url));
        out.push_str(
            "\nGenerated media will keep copies there. Tell the user the URL; the setting can be \
             changed later on the project's settings page or with `oxen-harness project set-repo`.",
        );
        Ok(out)
    }
}

/// When the project folder is an Oxen repo and the CLI is around, point its
/// `origin` at `url`. Reports what happened as a sentence for the model.
fn point_oxen_remote(root: &Path, url: &str) -> String {
    let oxen = harness_oxen::Oxen::new();
    if !oxen.is_repo(root) {
        return String::new();
    }
    if !oxen.is_available() {
        return format!(
            "\nThe project folder is an Oxen repo but the `oxen` CLI isn't installed, so its \
             `origin` remote was not set; the user can run `oxen config --set-remote origin {url}`."
        );
    }
    match oxen.set_remote(root, "origin", url) {
        Ok(()) => "\nThe project's Oxen `origin` remote now points there (`oxen push origin main` will publish it).".to_string(),
        Err(e) => format!("\nSetting the Oxen `origin` remote failed ({e}); the user can run `oxen config --set-remote origin {url}`."),
    }
}

fn hub_err(e: harness_media::hub::HubError) -> ToolError {
    ToolError::Execution(format!("hub: {e}"))
}

/// The project folder's name, as a repository name the hub accepts.
fn folder_slug(root: &Path) -> String {
    let raw = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    let slug: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        "project".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_tools::QuestionAnswer;
    use std::sync::Mutex;

    /// Answers every question with a fixed label and remembers what was asked.
    struct ScriptedAsker {
        answer: Option<&'static str>,
        asked: Mutex<Vec<Question>>,
    }

    #[async_trait]
    impl QuestionAsker for ScriptedAsker {
        async fn ask(&self, questions: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
            self.asked.lock().unwrap().extend(questions.iter().cloned());
            Ok(self.answer.map(|label| {
                questions
                    .iter()
                    .map(|q| QuestionAnswer {
                        header: q.header.clone(),
                        question: q.question.clone(),
                        selected: vec![label.to_string()],
                    })
                    .collect()
            }))
        }
    }

    fn tool(root: &Path, server: &mockito::Server, answer: Option<&'static str>) -> (CreateRepositoryTool, Arc<ScriptedAsker>) {
        let asker = Arc::new(ScriptedAsker { answer, asked: Mutex::new(Vec::new()) });
        let api = MediaApi { base_url: format!("{}/api/ai", server.url()), api_key: "key".into() };
        let t = CreateRepositoryTool::new(root, Some(api), asker.clone());
        (t, asker)
    }

    #[tokio::test]
    async fn creates_after_the_user_says_yes_and_records_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("My App");
        std::fs::create_dir(&root).unwrap();
        let mut server = mockito::Server::new_async().await;
        let _me = server.mock("GET", "/api/users/me").with_body(r#"{"user":{"username":"greg"}}"#).create_async().await;
        let _exists = server.mock("GET", "/api/repos/greg/My-App").with_status(404).create_async().await;
        let create = server
            .mock("POST", "/api/repos/")
            .match_body(mockito::Matcher::PartialJsonString(r#"{"name":"My-App","namespace":"greg","visibility":"private"}"#.into()))
            .with_status(201)
            .with_body("{}")
            .create_async()
            .await;

        let (tool, asker) = tool(&root, &server, Some("Create"));
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        create.assert_async().await;
        assert!(out.contains("Created"), "{out}");
        assert!(out.contains(&format!("{}/greg/My-App", server.url())), "{out}");
        assert_eq!(project::load(&root).remote_repo.as_deref(), Some("greg/My-App"));
        let asked = asker.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].question.contains("Create the private repository `greg/My-App`"), "{}", asked[0].question);
    }

    #[tokio::test]
    async fn adopts_an_existing_repository_without_posting() {
        let tmp = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new_async().await;
        let _exists = server.mock("GET", "/api/repos/ox/art").with_status(200).with_body("{}").create_async().await;
        let create = server.mock("POST", "/api/repos/").expect(0).create_async().await;
        project::set_remote_repo(tmp.path(), Some("ox/old")).unwrap();

        let (tool, asker) = tool(tmp.path(), &server, Some("Use it"));
        let out = tool.invoke(serde_json::json!({"name": "art", "namespace": "ox"})).await.unwrap();
        create.assert_async().await;
        assert!(out.contains("Adopted"), "{out}");
        assert_eq!(project::load(tmp.path()).remote_repo.as_deref(), Some("ox/art"));
        let asked = asker.asked.lock().unwrap();
        assert!(asked[0].question.contains("already exists"), "{}", asked[0].question);
        assert!(asked[0].question.contains("replaces the current default `ox/old`"), "{}", asked[0].question);
    }

    #[tokio::test]
    async fn a_decline_or_no_user_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new_async().await;
        let _exists = server.mock("GET", "/api/repos/ox/art").with_status(404).create_async().await;
        let create = server.mock("POST", "/api/repos/").expect(0).create_async().await;

        let (declined, _) = tool(tmp.path(), &server, Some("Cancel"));
        let out = declined.invoke(serde_json::json!({"name": "art", "namespace": "ox"})).await.unwrap();
        assert!(out.contains("declined"), "{out}");

        let (nobody, _) = tool(tmp.path(), &server, None);
        let out = nobody.invoke(serde_json::json!({"name": "art", "namespace": "ox"})).await.unwrap();
        assert!(out.contains("oxen-harness project set-repo ox/art"), "{out}");

        create.assert_async().await;
        assert_eq!(project::load(tmp.path()).remote_repo, None);
    }

    #[tokio::test]
    async fn without_a_key_it_says_so_and_never_asks() {
        let tmp = tempfile::tempdir().unwrap();
        let asker = Arc::new(ScriptedAsker { answer: Some("Create"), asked: Mutex::new(Vec::new()) });
        let tool = CreateRepositoryTool::new(tmp.path(), None, asker.clone());
        let out = tool.invoke(serde_json::json!({"name": "art"})).await.unwrap();
        assert_eq!(out, REPO_NO_KEY);
        assert!(asker.asked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_bad_name_is_refused_before_any_hub_call() {
        let tmp = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new_async().await;
        let any = server.mock("GET", mockito::Matcher::Any).expect(0).create_async().await;
        let (tool, _) = tool(tmp.path(), &server, Some("Create"));
        let out = tool.invoke(serde_json::json!({"name": "a/b", "namespace": "ox"})).await.unwrap();
        assert!(out.contains("isn't a valid repository name"), "{out}");
        any.assert_async().await;
    }

    #[test]
    fn folder_names_become_hub_safe_slugs() {
        assert_eq!(folder_slug(Path::new("/x/My App")), "My-App");
        assert_eq!(folder_slug(Path::new("/x/my_app.v2")), "my_app.v2");
        assert_eq!(folder_slug(Path::new("/x/---")), "project");
    }
}
