//! Built-in agent tools for oxen-harness.
//!
//! This crate defines the [`Tool`] trait every capability implements, a
//! [`ToolRegistry`] for dispatching model tool calls by name, and the concrete
//! tools the agent uses: file read/write/edit, glob file discovery, and regex
//! content search ([`fs`]), sandboxed shell execution ([`shell`]), and git
//! operations ([`git`]). All file access is confined to a [`sandbox::Workspace`].

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

pub mod fs;
pub mod git;
pub mod sandbox;
pub mod shell;

pub use sandbox::Workspace;

/// Errors a tool can return while running.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("execution failed: {0}")]
    Execution(String),
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A capability the agent can invoke during the loop.
///
/// The `parameters_schema` is a JSON Schema object describing `invoke`'s
/// arguments; it is sent to the model as part of the OpenAI-compatible tool
/// definition so the model knows how to call it.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable identifier the model uses to call this tool.
    fn name(&self) -> &str;

    /// Human-readable description shown to the model.
    fn description(&self) -> &str;

    /// JSON Schema describing the arguments accepted by [`Tool::invoke`].
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with model-provided arguments, returning a string
    /// result that is appended to the transcript as a `tool` message.
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError>;
}

/// Build the OpenAI-compatible tool definition for a tool.
///
/// Shape: `{ "type": "function", "function": { name, description, parameters } }`.
pub fn tool_definition(tool: &dyn Tool) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name(),
            "description": tool.description(),
            "parameters": tool.parameters_schema(),
        }
    })
}

/// A set of tools, addressable by name, that the agent loop dispatches into.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool, returning the registry for chaining.
    pub fn with(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.insert(tool.name().to_string(), tool);
        self
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// The OpenAI-compatible `tools` array to send with a chat request.
    pub fn definitions(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|t| tool_definition(t.as_ref()))
            .collect()
    }

    /// Dispatch a model tool call to the matching tool.
    pub async fn invoke(&self, name: &str, args: serde_json::Value) -> Result<String, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?;
        tool.invoke(args).await
    }

    /// Construct the default tool set rooted at a workspace: fs read/write/edit,
    /// find (glob), search (grep), shell, and git.
    pub fn default_for_workspace(workspace: Workspace) -> Self {
        Self::new()
            .with(Arc::new(fs::ReadFileTool::new(workspace.clone())))
            .with(Arc::new(fs::WriteFileTool::new(workspace.clone())))
            .with(Arc::new(fs::EditFileTool::new(workspace.clone())))
            .with(Arc::new(fs::FindFilesTool::new(workspace.clone())))
            .with(Arc::new(fs::SearchTool::new(workspace.clone())))
            .with(Arc::new(shell::ShellTool::new(workspace.clone())))
            .with(Arc::new(git::GitTool::new(workspace)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echo the provided text back."
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            })
        }
        async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
            args.get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| ToolError::InvalidArguments("missing `text`".into()))
        }
    }

    #[test]
    fn tool_definition_has_openai_function_shape() {
        let def = tool_definition(&EchoTool);
        assert_eq!(def["type"], "function");
        assert_eq!(def["function"]["name"], "echo");
        assert_eq!(def["function"]["parameters"]["type"], "object");
    }

    #[tokio::test]
    async fn registry_dispatches_by_name() {
        let registry = ToolRegistry::new().with(Arc::new(EchoTool));
        assert_eq!(registry.len(), 1);
        let out = registry
            .invoke("echo", serde_json::json!({"text": "moo"}))
            .await
            .unwrap();
        assert_eq!(out, "moo");
    }

    #[tokio::test]
    async fn registry_errors_on_unknown_tool() {
        let registry = ToolRegistry::new();
        let err = registry
            .invoke("nope", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::UnknownTool(_)));
    }
}
