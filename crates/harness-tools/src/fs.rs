//! Filesystem tools: read, write, edit, find (glob), and search (grep).
//!
//! These mirror the essential file primitives a strong coding agent expects
//! (read with line numbers + offset/limit, exact-string edit, glob file
//! discovery, and ripgrep-style regex search), all confined to the
//! [`Workspace`] sandbox.

use std::path::Path;

use async_trait::async_trait;
use globset::GlobBuilder;
use regex::RegexBuilder;

use crate::sandbox::Workspace;
use crate::{Tool, ToolError};

/// `read_file` reads at most this many lines when no `limit` is given.
const DEFAULT_READ_LIMIT: usize = 2000;
/// Lines longer than this are truncated in `read_file` output.
const MAX_LINE_LEN: usize = 2000;
/// Default cap on `find_files` / `search_files` results.
const DEFAULT_MAX_RESULTS: usize = 200;

fn require_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing string `{key}`")))
}

fn opt_u64(args: &serde_json::Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

fn opt_bool(args: &serde_json::Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Read a UTF-8 text file with `cat -n`-style line numbers.
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
        "Read a UTF-8 text file relative to the workspace root. Output is line-numbered \
         in `cat -n` format (right-aligned number, a tab, then the line content); reads up \
         to 2000 lines from the start by default. Use `offset` (1-based start line) and \
         `limit` for large files. NOTE: when editing, never include the line-number/tab \
         prefix in `edit_file` arguments — match only the content after the tab."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path relative to the workspace root." },
                "offset": { "type": "integer", "description": "1-based line to start reading from." },
                "limit": { "type": "integer", "description": "Maximum number of lines to read." }
            },
            "required": ["path"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let path = self.workspace.resolve(require_str(&args, "path")?)?;
        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::Execution(format!("read {}: {e}", path.display())))?;
        Ok(number_lines(
            &contents,
            opt_u64(&args, "offset").unwrap_or(1).max(1) as usize,
            opt_u64(&args, "limit").unwrap_or(DEFAULT_READ_LIMIT as u64) as usize,
        ))
    }
}

/// Render `contents` with `cat -n` line numbers, applying `offset`/`limit` and
/// truncating overly long lines. `offset` is 1-based.
fn number_lines(contents: &str, offset: usize, limit: usize) -> String {
    if contents.is_empty() {
        return "(file is empty)".to_string();
    }
    let lines: Vec<&str> = contents.lines().collect();
    let total = lines.len();
    let start = offset.saturating_sub(1);

    let mut out = String::new();
    for (i, line) in lines.iter().skip(start).take(limit).enumerate() {
        let n = start + i + 1;
        let shown = if line.chars().count() > MAX_LINE_LEN {
            let kept: String = line.chars().take(MAX_LINE_LEN).collect();
            format!("{kept}… [line truncated]")
        } else {
            (*line).to_string()
        };
        out.push_str(&format!("{n:>6}\t{shown}\n"));
    }

    let shown_end = (start + limit).min(total);
    if shown_end < total {
        out.push_str(&format!(
            "… [showing lines {}-{} of {total}; pass offset to read more]\n",
            start + 1,
            shown_end
        ));
    }
    out
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
         `old_string` must match exactly once unless `replace_all` is true. Match only \
         the real file content — do NOT include the line-number/tab prefix that \
         `read_file` adds to its output."
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
        let replace_all = opt_bool(&args, "replace_all");

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

/// Find files by glob pattern (the agent's `Glob`), respecting `.gitignore`.
pub struct FindFilesTool {
    workspace: Workspace,
}

impl FindFilesTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for FindFilesTool {
    fn name(&self) -> &str {
        "find_files"
    }
    fn description(&self) -> &str {
        "Find files by glob pattern relative to the workspace root, e.g. `**/*.rs`, \
         `src/**/*.ts`, `*.toml`. `*` does not cross directory boundaries; use `**` to \
         recurse. Respects .gitignore. Returns paths, most-recently-modified first."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern, e.g. `**/*.rs`." },
                "max_results": { "type": "integer", "default": DEFAULT_MAX_RESULTS }
            },
            "required": ["pattern"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let pattern = require_str(&args, "pattern")?.to_string();
        let max_results =
            opt_u64(&args, "max_results").unwrap_or(DEFAULT_MAX_RESULTS as u64) as usize;
        let root = self.workspace.root().to_path_buf();

        let results =
            tokio::task::spawn_blocking(move || find_blocking(&root, &pattern, max_results))
                .await
                .map_err(|e| ToolError::Execution(format!("find task: {e}")))??;

        if results.is_empty() {
            Ok("no files match the pattern".to_string())
        } else {
            Ok(results.join("\n"))
        }
    }
}

fn find_blocking(root: &Path, pattern: &str, max_results: usize) -> Result<Vec<String>, ToolError> {
    let matcher = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| ToolError::InvalidArguments(format!("invalid glob `{pattern}`: {e}")))?
        .compile_matcher();

    // Collect (modified-time, relative-path) so we can sort newest-first.
    let mut hits: Vec<(std::time::SystemTime, String)> = Vec::new();
    for entry in ignore::Walk::new(root).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
        if !matcher.is_match(rel) {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        hits.push((mtime, rel.display().to_string()));
    }

    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.truncate(max_results);
    Ok(hits.into_iter().map(|(_, p)| p).collect())
}

/// Search file contents with a regular expression (the agent's `Grep`).
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
        "Search workspace file contents with a regular expression (ripgrep-style; respects \
         .gitignore). `output_mode` is `content` (default — `path:line:text`), \
         `files_with_matches` (just paths), or `count` (per-file match counts). Optionally \
         restrict with `path` (a subdir or file) and `glob` (a filename filter like `*.rs`)."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regular expression to search for." },
                "path": { "type": "string", "description": "Subdirectory or file to restrict the search to." },
                "glob": { "type": "string", "description": "Filename filter, e.g. `*.rs` or `**/*.ts`." },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "default": "content"
                },
                "case_insensitive": { "type": "boolean", "default": false },
                "max_results": { "type": "integer", "default": DEFAULT_MAX_RESULTS }
            },
            "required": ["pattern"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let pattern = require_str(&args, "pattern")?.to_string();
        let glob = args
            .get("glob")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let mode = args
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("content")
            .to_string();
        let case_insensitive = opt_bool(&args, "case_insensitive");
        let max_results =
            opt_u64(&args, "max_results").unwrap_or(DEFAULT_MAX_RESULTS as u64) as usize;
        let root = self.workspace.root().to_path_buf();
        // `path` is resolved through the sandbox so it cannot escape the workspace.
        let search_root = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => self.workspace.resolve(p)?,
            None => root.clone(),
        };

        let results = tokio::task::spawn_blocking(move || {
            grep_blocking(GrepOpts {
                root: &root,
                search_root: &search_root,
                pattern: &pattern,
                glob: glob.as_deref(),
                mode: &mode,
                case_insensitive,
                max_results,
            })
        })
        .await
        .map_err(|e| ToolError::Execution(format!("search task: {e}")))??;

        if results.is_empty() {
            Ok("no matches".to_string())
        } else {
            Ok(results.join("\n"))
        }
    }
}

struct GrepOpts<'a> {
    root: &'a Path,
    search_root: &'a Path,
    pattern: &'a str,
    glob: Option<&'a str>,
    mode: &'a str,
    case_insensitive: bool,
    max_results: usize,
}

fn grep_blocking(opts: GrepOpts<'_>) -> Result<Vec<String>, ToolError> {
    let re = RegexBuilder::new(opts.pattern)
        .case_insensitive(opts.case_insensitive)
        .build()
        .map_err(|e| ToolError::InvalidArguments(format!("invalid regex: {e}")))?;

    let glob_matcher = match opts.glob {
        Some(g) => Some(
            GlobBuilder::new(g)
                .literal_separator(true)
                .build()
                .map_err(|e| ToolError::InvalidArguments(format!("invalid glob `{g}`: {e}")))?
                .compile_matcher(),
        ),
        None => None,
    };

    let mut out = Vec::new();
    for entry in ignore::Walk::new(opts.search_root).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(opts.root).unwrap_or(path);
        if let Some(m) = &glob_matcher {
            if !m.is_match(rel) {
                continue;
            }
        }
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue; // skip binary / unreadable files
        };

        match opts.mode {
            "files_with_matches" => {
                if contents.lines().any(|l| re.is_match(l)) {
                    out.push(rel.display().to_string());
                }
            }
            "count" => {
                let n = contents.lines().filter(|l| re.is_match(l)).count();
                if n > 0 {
                    out.push(format!("{}:{n}", rel.display()));
                }
            }
            _ => {
                for (i, line) in contents.lines().enumerate() {
                    if re.is_match(line) {
                        out.push(format!("{}:{}:{}", rel.display(), i + 1, line.trim_end()));
                        if out.len() >= opts.max_results {
                            return Ok(out);
                        }
                    }
                }
            }
        }
        if opts.mode != "content" && out.len() >= opts.max_results {
            return Ok(out);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::new(dir.path()).unwrap();
        (dir, ws)
    }

    async fn write(ws: &Workspace, path: &str, contents: &str) {
        WriteFileTool::new(ws.clone())
            .invoke(serde_json::json!({ "path": path, "contents": contents }))
            .await
            .unwrap();
    }

    async fn read_raw(ws: &Workspace, path: &str) -> String {
        tokio::fs::read_to_string(ws.resolve(path).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn read_returns_numbered_lines() {
        let (_dir, ws) = workspace();
        write(&ws, "a.txt", "first\nsecond\n").await;
        let out = ReadFileTool::new(ws)
            .invoke(serde_json::json!({"path": "a.txt"}))
            .await
            .unwrap();
        assert_eq!(out, "     1\tfirst\n     2\tsecond\n");
    }

    #[tokio::test]
    async fn read_honors_offset_and_limit() {
        let (_dir, ws) = workspace();
        write(&ws, "a.txt", "l1\nl2\nl3\nl4\n").await;
        let out = ReadFileTool::new(ws)
            .invoke(serde_json::json!({"path": "a.txt", "offset": 2, "limit": 2}))
            .await
            .unwrap();
        assert!(out.contains("     2\tl2"));
        assert!(out.contains("     3\tl3"));
        assert!(!out.contains("l1"));
        assert!(out.contains("showing lines 2-3 of 4"));
    }

    #[tokio::test]
    async fn write_then_read_round_trips_raw() {
        let (_dir, ws) = workspace();
        write(&ws, "src/a.txt", "hello").await;
        assert_eq!(read_raw(&ws, "src/a.txt").await, "hello");
    }

    #[tokio::test]
    async fn edit_replaces_unique_string() {
        let (_dir, ws) = workspace();
        write(&ws, "f.txt", "foo bar baz").await;
        EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({"path": "f.txt", "old_string": "bar", "new_string": "qux"}))
            .await
            .unwrap();
        assert_eq!(read_raw(&ws, "f.txt").await, "foo qux baz");
    }

    #[tokio::test]
    async fn edit_refuses_ambiguous_match_without_replace_all() {
        let (_dir, ws) = workspace();
        write(&ws, "f.txt", "x x x").await;
        let err = EditFileTool::new(ws)
            .invoke(serde_json::json!({"path": "f.txt", "old_string": "x", "new_string": "y"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn find_files_matches_glob_recursively() {
        let (_dir, ws) = workspace();
        write(&ws, "src/main.rs", "fn main() {}").await;
        write(&ws, "src/lib/util.rs", "// util").await;
        write(&ws, "README.md", "# readme").await;

        let out = FindFilesTool::new(ws.clone())
            .invoke(serde_json::json!({"pattern": "**/*.rs"}))
            .await
            .unwrap();
        assert!(out.contains("src/main.rs"));
        assert!(out.contains("src/lib/util.rs"));
        assert!(!out.contains("README.md"));
    }

    #[tokio::test]
    async fn find_files_star_does_not_cross_directories() {
        let (_dir, ws) = workspace();
        write(&ws, "top.rs", "// top").await;
        write(&ws, "src/deep.rs", "// deep").await;
        let out = FindFilesTool::new(ws)
            .invoke(serde_json::json!({"pattern": "*.rs"}))
            .await
            .unwrap();
        assert!(out.contains("top.rs"));
        assert!(!out.contains("deep.rs"));
    }

    #[tokio::test]
    async fn search_content_mode_uses_regex() {
        let (_dir, ws) = workspace();
        write(&ws, "code.rs", "fn main() {}\nlet ox = 1;\n").await;
        let out = SearchTool::new(ws)
            .invoke(serde_json::json!({"pattern": r"\blet\b"}))
            .await
            .unwrap();
        assert!(out.contains("code.rs:2:"));
        assert!(!out.contains("code.rs:1:"));
    }

    #[tokio::test]
    async fn search_files_with_matches_mode_returns_paths() {
        let (_dir, ws) = workspace();
        write(&ws, "a.rs", "needle here").await;
        write(&ws, "b.txt", "needle there").await;
        let out = SearchTool::new(ws)
            .invoke(serde_json::json!({
                "pattern": "needle",
                "output_mode": "files_with_matches",
                "glob": "*.rs"
            }))
            .await
            .unwrap();
        assert!(out.contains("a.rs"));
        assert!(!out.contains("b.txt")); // filtered out by glob
    }

    #[tokio::test]
    async fn search_count_mode_reports_per_file_counts() {
        let (_dir, ws) = workspace();
        write(&ws, "c.rs", "ox\nox\nno\n").await;
        let out = SearchTool::new(ws)
            .invoke(serde_json::json!({"pattern": "ox", "output_mode": "count"}))
            .await
            .unwrap();
        assert!(out.contains("c.rs:2"));
    }

    #[tokio::test]
    async fn search_case_insensitive() {
        let (_dir, ws) = workspace();
        write(&ws, "c.rs", "Oxen\n").await;
        let out = SearchTool::new(ws)
            .invoke(serde_json::json!({"pattern": "oxen", "case_insensitive": true}))
            .await
            .unwrap();
        assert!(out.contains("c.rs:1:"));
    }
}
