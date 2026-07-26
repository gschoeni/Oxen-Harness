//! GitHub tool: check and open pull requests through the `gh` CLI.
//!
//! The trail's shipping stages — pushed, pr-reviewed, merged — are the
//! model's to verify: it decides when to check, runs this tool, reads the
//! answer, and only then marks a stage done on its charted trail. Like
//! [`crate::git`] this shells out to the system binary with an allow-list of
//! operations; `gh` brings the user's auth and repo mapping with it.

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

/// Tool name for [`GhTool`].
pub const GH_TOOL: &str = "gh";
const MAX_GH_CHARS: usize = 50_000;
/// `gh` talks to the network; give it more rope than local git.
const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// Perform a GitHub operation in the workspace.
pub struct GhTool {
    workspace: Workspace,
}

impl GhTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }

    async fn run_gh(&self, args: &[&str]) -> Result<String, ToolError> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        crate::process::run_cli("gh", &args, self.workspace.root(), GH_TIMEOUT, MAX_GH_CHARS).await
    }
}

/// The allow-listed GitHub operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GhOperation {
    /// The current branch's PR: state, review decision, merged, CI.
    PrView,
    /// CI check results for the current branch's PR.
    PrChecks,
    /// Open a PR for the current branch (uses `title`/`body`, else the
    /// commits fill them in).
    PrCreate,
}

/// Arguments to `gh`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct GhArgs {
    /// The operation to perform.
    pub operation: GhOperation,
    /// PR title (for `pr_create`; give with `body`, or omit both to fill from commits).
    pub title: Option<String>,
    /// PR body (for `pr_create`; give with `title`).
    pub body: Option<String>,
}

#[async_trait]
impl TypedTool for GhTool {
    const NAME: &'static str = GH_TOOL;
    type Args = GhArgs;

    fn description(&self) -> &str {
        "GitHub via the gh CLI: `pr_view` (the current branch's PR — state, \
         review decision, merged; USE THIS to verify the trail's shipping \
         stages before marking them done), `pr_checks` (CI results), and \
         `pr_create` (open a PR for the pushed branch; give `title` and \
         `body` together, or neither to fill both from the commits). Needs \
         gh installed and authenticated."
    }

    async fn run(&self, args: GhArgs) -> Result<String, ToolError> {
        match args.operation {
            GhOperation::PrView => {
                self.run_gh(&[
                    "pr",
                    "view",
                    "--json",
                    "number,url,title,state,reviewDecision,mergedAt,statusCheckRollup",
                ])
                .await
            }
            GhOperation::PrChecks => self.run_gh(&["pr", "checks"]).await,
            // Model-dialect leniency: an empty string is an unused form field,
            // not a value (see `tests/model_dialects.rs`).
            GhOperation::PrCreate => match (present(&args.title), present(&args.body)) {
                (Some(title), Some(body)) => {
                    self.run_gh(&["pr", "create", "--title", title, "--body", body])
                        .await
                }
                (None, None) => self.run_gh(&["pr", "create", "--fill"]).await,
                // `--fill` can't be combined with `--title`/`--body`, and
                // half-filled args used to silently drop the body or ship a
                // blank one. Teach instead of guessing.
                (Some(_), None) => Err(ToolError::InvalidArguments(
                    "`pr_create` got a `title` but no `body`. Give both (recommended: write a \
                     short summary body), or omit both to fill title and body from the branch's \
                     commits."
                        .into(),
                )),
                (None, Some(_)) => Err(ToolError::InvalidArguments(
                    "`pr_create` got a `body` but no `title`. Give both, or omit both to fill \
                     title and body from the branch's commits — a body alone would have been \
                     silently discarded."
                        .into(),
                )),
            },
        }
    }
}

/// A form field the model actually filled in: non-empty after trimming.
fn present(field: &Option<String>) -> Option<&str> {
    field.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_schema_shape() {
        assert_eq!(GhTool::NAME, GH_TOOL);
        let schema = crate::schema_for::<GhArgs>();
        assert_eq!(schema["required"][0], "operation");
        let op = &schema["properties"]["operation"];
        assert_eq!(op["enum"][0], "pr_view");
        assert_eq!(op["enum"][2], "pr_create");
    }

    /// Half-filled pr_create args must teach, not guess: a title-only call
    /// used to open a PR with a blank body, and a body-only call silently
    /// threw the body away behind `--fill`.
    #[tokio::test]
    async fn pr_create_with_half_filled_args_teaches_instead_of_guessing() {
        let tool = GhTool::new(Workspace::new(".").unwrap());
        for args in [
            serde_json::json!({ "operation": "pr_create", "title": "Fix the bug" }),
            serde_json::json!({ "operation": "pr_create", "body": "## Summary\nfixes" }),
            // An empty string is an unused form field, so this is body-only.
            serde_json::json!({ "operation": "pr_create", "title": "  ", "body": "text" }),
        ] {
            let err = tool.invoke(args.clone()).await.unwrap_err();
            match err {
                ToolError::InvalidArguments(msg) => {
                    assert!(msg.contains("omit both"), "unhelpful error for {args}: {msg}")
                }
                other => panic!("expected InvalidArguments for {args}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn arguments_outside_the_allowlist_are_rejected_at_parse() {
        let tool = GhTool::new(Workspace::new(".").unwrap());
        let err = tool
            .invoke(serde_json::json!({ "operation": "repo_delete" }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }
}
