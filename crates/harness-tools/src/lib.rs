//! Built-in agent tools for oxen-harness.
//!
//! Phase 2 fills in concrete tools (read/write/edit/search files, sandboxed
//! shell execution, and git status/diff/log/commit). This module defines the
//! `Tool` trait every tool implements and the error type they share.

use async_trait::async_trait;

/// Errors a tool can return while running.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("execution failed: {0}")]
    Execution(String),
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
    fn tool_is_object_safe_and_exposes_metadata() {
        let tool: Box<dyn Tool> = Box::new(EchoTool);
        assert_eq!(tool.name(), "echo");
        assert_eq!(tool.parameters_schema()["type"], "object");
    }
}
