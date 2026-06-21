//! Filesystem tools: read, write, edit, and search files within the workspace.

use async_trait::async_trait;

use crate::sandbox::Workspace;
use crate::{Tool, ToolError};

fn require_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing string `{key}`")))
}

/// Read a UTF-8 text file relative to the workspace root.
pub struct ReadFileTool {
    workspace: Workspace,
}

impl ReadFileTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read a UTF-8 text file at a path relative to the workspace root."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path relative to the workspace root." }
            },
            "required": ["path"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let path = self.workspace.resolve(require_str(&args, "path")?)?;
        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::Execution(format!("read {}: {e}", path.display())))?;
        Ok(contents)
    }
}

/// Create or overwrite a text file, creating parent directories as needed.
pub struct WriteFileTool {
    workspace: Workspace,
}

impl WriteFileTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Create or overwrite a text file at a path relative to the workspace root."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "contents": { "type": "string" }
            },
            "required": ["path", "contents"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let path = self.workspace.resolve(require_str(&args, "path")?)?;
        let contents = require_str(&args, "contents")?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, contents)
            .await
            .map_err(|e| ToolError::Execution(format!("write {}: {e}", path.display())))?;
        Ok(format!(
            "wrote {} bytes to {}",
            contents.len(),
            path.display()
        ))
    }
}

/// Replace an exact, unique string in a file (like a precise patch).
pub struct EditFileTool {
    workspace: Workspace,
}

impl EditFileTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }
    fn description(&self) -> &str {
        "Replace an exact occurrence of `old_string` with `new_string` in a file. \
         `old_string` must match exactly once unless `replace_all` is true."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" },
                "replace_all": { "type": "boolean", "default": false }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let path = self.workspace.resolve(require_str(&args, "path")?)?;
        let old = require_str(&args, "old_string")?;
        let new = require_str(&args, "new_string")?;
        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let original = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::Execution(format!("read {}: {e}", path.display())))?;

        let count = original.matches(old).count();
        if count == 0 {
            return Err(ToolError::InvalidArguments(
                "`old_string` not found in file".into(),
            ));
        }
        if count > 1 && !replace_all {
            return Err(ToolError::InvalidArguments(format!(
                "`old_string` matches {count} times; pass replace_all=true or add more context"
            )));
        }

        let updated = if replace_all {
            original.replace(old, new)
        } else {
            original.replacen(old, new, 1)
        };
        tokio::fs::write(&path, &updated)
            .await
            .map_err(|e| ToolError::Execution(format!("write {}: {e}", path.display())))?;
        Ok(format!(
            "edited {} ({count} replacement(s))",
            path.display()
        ))
    }
}

/// Search file contents for a substring, respecting .gitignore.
pub struct SearchTool {
    workspace: Workspace,
}

impl SearchTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search_files"
    }
    fn description(&self) -> &str {
        "Search workspace file contents for a literal substring (respects .gitignore). \
         Returns matching `relative/path:line:text` lines."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Literal substring to find." },
                "max_results": { "type": "integer", "default": 100 }
            },
            "required": ["query"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let query = require_str(&args, "query")?.to_string();
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as usize;
        let root = self.workspace.root().to_path_buf();

        // ignore::Walk is synchronous; run it off the async runtime.
        let results =
            tokio::task::spawn_blocking(move || search_blocking(&root, &query, max_results))
                .await
                .map_err(|e| ToolError::Execution(format!("search task: {e}")))??;

        if results.is_empty() {
            Ok("no matches".to_string())
        } else {
            Ok(results.join("\n"))
        }
    }
}

fn search_blocking(
    root: &std::path::Path,
    query: &str,
    max_results: usize,
) -> Result<Vec<String>, ToolError> {
    let mut out = Vec::new();
    for entry in ignore::Walk::new(root).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue; // skip binary / unreadable files
        };
        let rel = path.strip_prefix(root).unwrap_or(path);
        for (i, line) in contents.lines().enumerate() {
            if line.contains(query) {
                out.push(format!("{}:{}:{}", rel.display(), i + 1, line.trim_end()));
                if out.len() >= max_results {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        (dir, ws)
    }

    #[tokio::test]
    async fn write_then_read_round_trips() {
        let (_dir, ws) = workspace().await;
        let writer = WriteFileTool::new(ws.clone());
        writer
            .invoke(serde_json::json!({"path": "src/a.txt", "contents": "hello"}))
            .await
            .unwrap();

        let reader = ReadFileTool::new(ws);
        let got = reader
            .invoke(serde_json::json!({"path": "src/a.txt"}))
            .await
            .unwrap();
        assert_eq!(got, "hello");
    }

    #[tokio::test]
    async fn edit_replaces_unique_string() {
        let (_dir, ws) = workspace().await;
        WriteFileTool::new(ws.clone())
            .invoke(serde_json::json!({"path": "f.txt", "contents": "foo bar baz"}))
            .await
            .unwrap();

        EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({"path": "f.txt", "old_string": "bar", "new_string": "qux"}))
            .await
            .unwrap();

        let got = ReadFileTool::new(ws)
            .invoke(serde_json::json!({"path": "f.txt"}))
            .await
            .unwrap();
        assert_eq!(got, "foo qux baz");
    }

    #[tokio::test]
    async fn edit_refuses_ambiguous_match_without_replace_all() {
        let (_dir, ws) = workspace().await;
        WriteFileTool::new(ws.clone())
            .invoke(serde_json::json!({"path": "f.txt", "contents": "x x x"}))
            .await
            .unwrap();
        let err = EditFileTool::new(ws)
            .invoke(serde_json::json!({"path": "f.txt", "old_string": "x", "new_string": "y"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn search_finds_matching_lines() {
        let (_dir, ws) = workspace().await;
        WriteFileTool::new(ws.clone())
            .invoke(
                serde_json::json!({"path": "code.rs", "contents": "fn main() {}\nlet ox = 1;\n"}),
            )
            .await
            .unwrap();

        let out = SearchTool::new(ws)
            .invoke(serde_json::json!({"query": "ox"}))
            .await
            .unwrap();
        assert!(out.contains("code.rs:2:"));
    }
}
