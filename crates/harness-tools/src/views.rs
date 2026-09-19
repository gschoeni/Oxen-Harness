//! Host-neutral view discovery, opening, and inspection for the agent.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ToolError, TypedTool};

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct OpenViewArgs {
    /// View ID from list_views. Omit to choose the best renderer for path.
    pub view: Option<String>,
    /// Existing workspace-relative file, e.g. workflows/product.graph.json.
    pub path: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct EmptyViewArgs {}

#[async_trait]
pub trait ViewHost: Send + Sync {
    async fn list(&self) -> Result<Value, ToolError>;
    async fn open(&self, args: OpenViewArgs) -> Result<Value, ToolError>;
    async fn inspect(&self) -> Result<Value, ToolError>;
    async fn develop(&self, _args: DevelopViewArgs) -> Result<Value, ToolError> {
        Err(ToolError::Execution(
            "view authoring is unavailable on this host".into(),
        ))
    }
    async fn run_workflow(&self, _args: RunWorkflowArgs) -> Result<Value, ToolError> {
        Err(ToolError::Execution(
            "workflow execution is unavailable on this host".into(),
        ))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DevelopAction {
    Scaffold,
    Check,
    Preview,
    Status,
    Test,
    Install,
    Pause,
    Resume,
    Stop,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DevelopViewArgs {
    /// Scaffold, validate, preview, inspect, test, or install a work view without rebuilding the app.
    pub action: DevelopAction,
    /// Workspace-relative package folder with view.json (or the folder to scaffold).
    pub source: String,
    /// Lowercase package id for scaffold, e.g. my.storyboard.
    pub id: Option<String>,
    /// Display title for scaffold.
    pub title: Option<String>,
    /// Exact digest from check/status, required for preview and install.
    pub digest: Option<String>,
}

pub struct DevelopViewTool(pub Arc<dyn ViewHost>);
#[async_trait]
impl TypedTool for DevelopViewTool {
    const NAME: &'static str = "develop_view";
    type Args = DevelopViewArgs;
    fn description(&self) -> &str {
        "Build work views inside the app: scaffold a no-build package, edit its files, check, preview, test, and install. Read the generated AGENTS.md and SDK types. Preview refreshes code edits while retaining the approved permissions. Status returns revision-scoped runtime errors and browser test results plus a report_path on disk. Test requires the live desktop preview; a successful check is not a browser test. Preview/install require the digest returned by check. Install only when the user wants to keep that version. No desktop rebuild or restart."
    }
    fn concurrency(&self) -> crate::Concurrency {
        crate::Concurrency::Exclusive
    }
    async fn run(&self, args: DevelopViewArgs) -> Result<String, ToolError> {
        result(self.0.develop(args).await?)
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct RunWorkflowArgs {
    /// Workspace-relative .graph.json file. The graph must already be saved.
    pub path: String,
    /// Revision returned by inspect_view or the document API. Omit to use disk.
    pub revision: Option<String>,
}

pub struct RunWorkflowTool(pub Arc<dyn ViewHost>);

#[async_trait]
impl TypedTool for RunWorkflowTool {
    const NAME: &'static str = "run_workflow";
    type Args = RunWorkflowArgs;
    fn description(&self) -> &str {
        "Explicitly run a saved Oxen node workflow. This can incur image, video, \
         and language-model charges; only run when the user wants execution. \
         Validates the complete graph first, uses media spend approvals, and \
         returns a durable run record path. Read that JSON for progress/results. \
         Editing or opening a workflow never runs it. Never retry a failed run \
         automatically: earlier nodes may already have been billed."
    }
    fn concurrency(&self) -> crate::Concurrency {
        crate::Concurrency::Exclusive
    }
    async fn run(&self, args: RunWorkflowArgs) -> Result<String, ToolError> {
        result(self.0.run_workflow(args).await?)
    }
}

pub struct ListViewsTool(pub Arc<dyn ViewHost>);
pub struct OpenViewTool(pub Arc<dyn ViewHost>);
pub struct InspectViewTool(pub Arc<dyn ViewHost>);

fn result(value: Value) -> Result<String, ToolError> {
    serde_json::to_string_pretty(&value)
        .map_err(|e| ToolError::Execution(format!("serialize view result: {e}")))
}

#[async_trait]
impl TypedTool for ListViewsTool {
    const NAME: &'static str = "list_views";
    type Args = EmptyViewArgs;
    fn description(&self) -> &str {
        "List the work views available for this conversation, their file formats, \
         and the current view. Views read project files: use normal file tools to \
         edit their content. Discover the workflow graph schema here."
    }
    async fn run(&self, _: EmptyViewArgs) -> Result<String, ToolError> {
        result(self.0.list().await?)
    }
}

#[async_trait]
impl TypedTool for OpenViewTool {
    const NAME: &'static str = "open_view";
    type Args = OpenViewArgs;
    fn description(&self) -> &str {
        "Request a work view beside this conversation. Write a new file first, \
         then open its path (e.g. a .graph.json Oxen workflow). Opening never runs \
         a workflow or spends money. Background conversations keep their own view."
    }
    async fn run(&self, args: OpenViewArgs) -> Result<String, ToolError> {
        result(self.0.open(args).await?)
    }
}

#[async_trait]
impl TypedTool for InspectViewTool {
    const NAME: &'static str = "inspect_view";
    type Args = EmptyViewArgs;
    fn description(&self) -> &str {
        "Inspect this conversation's work view: source path, disk revision, \
         validation errors, display status and reported selection/unsaved edits. \
         Read the source with read_file; an absent renderer has no live UI state."
    }
    async fn run(&self, _: EmptyViewArgs) -> Result<String, ToolError> {
        result(self.0.inspect().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingHost;
    #[async_trait]
    impl ViewHost for FailingHost {
        async fn list(&self) -> Result<Value, ToolError> {
            Ok(serde_json::json!({"views":[]}))
        }
        async fn open(&self, _: OpenViewArgs) -> Result<Value, ToolError> {
            Err(ToolError::Execution("open workflow: file missing".into()))
        }
        async fn inspect(&self) -> Result<Value, ToolError> {
            Ok(serde_json::json!({"status":"unavailable"}))
        }
    }

    #[tokio::test]
    async fn reports_real_host_status_and_preserves_open_failures() {
        let host = Arc::new(FailingHost);
        let error = OpenViewTool(host.clone())
            .invoke(serde_json::json!({"path":"work.graph.json"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("file missing"));
        assert!(InspectViewTool(host)
            .invoke(serde_json::json!({}))
            .await
            .unwrap()
            .contains("unavailable"));
    }
}
