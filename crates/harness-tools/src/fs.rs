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

use crate::args::{opt_bool, opt_str, opt_usize, require_str};
use crate::sandbox::Workspace;
use crate::{Tool, ToolError};

/// Tool name for [`ReadFileTool`].
pub const READ_FILE_TOOL: &str = "read_file";
/// Tool name for [`WriteFileTool`].
pub const WRITE_FILE_TOOL: &str = "write_file";
/// Tool name for [`EditFileTool`].
pub const EDIT_FILE_TOOL: &str = "edit_file";
/// Tool name for [`FindFilesTool`].
pub const FIND_FILES_TOOL: &str = "find_files";
/// Tool name for [`SearchTool`].
pub const SEARCH_FILES_TOOL: &str = "search_files";

/// `read_file` reads at most this many lines when no `limit` is given.
const DEFAULT_READ_LIMIT: usize = 2000;
/// Lines longer than this are truncated in `read_file` output.
const MAX_LINE_LEN: usize = 2000;
/// Default cap on `find_files` / `search_files` results.
const DEFAULT_MAX_RESULTS: usize = 200;

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
        READ_FILE_TOOL
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
            opt_usize(&args, "offset", 1).max(1),
            opt_usize(&args, "limit", DEFAULT_READ_LIMIT),
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

    // An offset past the end would otherwise render as an empty string, which
    // reads as "the file is empty" — tell the model what actually happened.
    if start >= total {
        return format!(
            "(offset {offset} is past the end of the file, which has {total} line{})",
            if total == 1 { "" } else { "s" }
        );
    }

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
        WRITE_FILE_TOOL
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
        EDIT_FILE_TOOL
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

        if old == new {
            return Err(ToolError::InvalidArguments(
                "`old_string` and `new_string` are identical; the edit would do nothing".into(),
            ));
        }

        let original = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::Execution(format!("read {}: {e}", path.display())))?;

        let count = original.matches(old).count();
        if count == 0 {
            return Err(ToolError::InvalidArguments(diagnose_no_match(
                &original, old,
            )));
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

/// Explain *why* an `edit_file` match failed, so the model can recover in one
/// step instead of blindly retrying. Checks the three mistakes LLM agents make
/// most often, in order of confidence, and falls back to a plain "not found".
fn diagnose_no_match(original: &str, old: &str) -> String {
    // 1. Did the model paste `read_file`'s `  123\t` line-number prefix into
    //    `old_string`? Very common, and unambiguous when the de-prefixed text
    //    then matches.
    if let Some(stripped) = strip_line_number_prefix(old) {
        if !stripped.is_empty() && original.contains(&stripped) {
            return "`old_string` not found — but it matches once the line-number/tab \
                    prefix is removed. `read_file` prepends `<number>\\t` to each line; \
                    pass only the real file content that follows the tab."
                .into();
        }
    }

    // 2. Right text, wrong whitespace (tabs vs spaces, indentation width,
    //    trailing spaces, or CRLF vs LF). Compare with all whitespace removed.
    if strip_whitespace(old).len() >= 4
        && strip_whitespace(original).contains(&strip_whitespace(old))
    {
        return "`old_string` not found — the same text exists but the whitespace differs \
                (tabs vs spaces, indentation, trailing spaces, or line endings). Re-read \
                the file and copy the exact indentation and spacing."
            .into();
    }

    // 3. Nothing close. If a single line of `old_string` does occur, point at it
    //    so the model knows its anchor drifted rather than being absent entirely.
    if let Some(anchor) = old.lines().find(|l| {
        let t = l.trim();
        t.len() >= 4 && original.contains(t)
    }) {
        return format!(
            "`old_string` not found. The line `{}` does appear in the file, but the \
             surrounding text does not match — re-read the file and copy the exact \
             current content around it.",
            anchor.trim()
        );
    }

    "`old_string` not found in file".into()
}

/// If every non-empty line of `s` carries `read_file`'s `<spaces><digits>\t`
/// prefix, return `s` with those prefixes removed; otherwise `None`.
fn strip_line_number_prefix(s: &str) -> Option<String> {
    let mut saw_prefixed = false;
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match line.split_once('\t') {
            Some((head, rest))
                if !head.is_empty()
                    && head.chars().all(|c| c.is_ascii_digit() || c == ' ')
                    && head.chars().any(|c| c.is_ascii_digit()) =>
            {
                saw_prefixed = true;
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
    }
    saw_prefixed.then_some(out)
}

/// `s` with all Unicode whitespace removed — used to test for whitespace-only
/// mismatches.
fn strip_whitespace(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
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
        FIND_FILES_TOOL
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
        let max_results = opt_usize(&args, "max_results", DEFAULT_MAX_RESULTS);
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
        SEARCH_FILES_TOOL
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
        let glob = opt_str(&args, "glob").map(str::to_string);
        let mode = OutputMode::parse(opt_str(&args, "output_mode").unwrap_or("content"));
        let case_insensitive = opt_bool(&args, "case_insensitive");
        let max_results = opt_usize(&args, "max_results", DEFAULT_MAX_RESULTS);
        let root = self.workspace.root().to_path_buf();
        // `path` is resolved through the sandbox so it cannot escape the workspace.
        let search_root = match opt_str(&args, "path") {
            Some(p) => self.workspace.resolve(p)?,
            None => root.clone(),
        };

        let results = tokio::task::spawn_blocking(move || {
            grep_blocking(GrepOpts {
                root: &root,
                search_root: &search_root,
                pattern: &pattern,
                glob: glob.as_deref(),
                mode,
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

/// How `search_files` reports matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    /// `path:line:text` for every matching line (default).
    Content,
    /// Just the paths of files containing a match.
    FilesWithMatches,
    /// `path:count` of matching lines per file.
    Count,
}

impl OutputMode {
    fn parse(s: &str) -> Self {
        match s {
            "files_with_matches" => Self::FilesWithMatches,
            "count" => Self::Count,
            _ => Self::Content,
        }
    }
}

struct GrepOpts<'a> {
    root: &'a Path,
    search_root: &'a Path,
    pattern: &'a str,
    glob: Option<&'a str>,
    mode: OutputMode,
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
            OutputMode::FilesWithMatches => {
                if contents.lines().any(|l| re.is_match(l)) {
                    out.push(rel.display().to_string());
                }
            }
            OutputMode::Count => {
                let n = contents.lines().filter(|l| re.is_match(l)).count();
                if n > 0 {
                    out.push(format!("{}:{n}", rel.display()));
                }
            }
            OutputMode::Content => {
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
        // Per-file modes append at most one line per file; cap once we have enough.
        if opts.mode != OutputMode::Content && out.len() >= opts.max_results {
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
    async fn read_past_end_of_file_explains_instead_of_returning_empty() {
        let (_dir, ws) = workspace();
        write(&ws, "a.txt", "l1\nl2\n").await;
        let out = ReadFileTool::new(ws)
            .invoke(serde_json::json!({"path": "a.txt", "offset": 50}))
            .await
            .unwrap();
        assert!(out.contains("past the end"), "got: {out}");
        assert!(out.contains("2 lines"), "got: {out}");
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
    async fn edit_rejects_noop_when_old_equals_new() {
        let (_dir, ws) = workspace();
        write(&ws, "f.txt", "hello").await;
        let err = EditFileTool::new(ws)
            .invoke(
                serde_json::json!({"path": "f.txt", "old_string": "hello", "new_string": "hello"}),
            )
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => assert!(m.contains("identical"), "got: {m}"),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_diagnoses_pasted_line_number_prefix() {
        let (_dir, ws) = workspace();
        write(&ws, "f.txt", "fn main() {\n    let x = 1;\n}\n").await;
        // Model copied read_file output verbatim, prefix and all.
        let err = EditFileTool::new(ws)
            .invoke(serde_json::json!({
                "path": "f.txt",
                "old_string": "     2\t    let x = 1;",
                "new_string": "     2\t    let x = 2;"
            }))
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => assert!(m.contains("line-number"), "got: {m}"),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_diagnoses_whitespace_mismatch() {
        let (_dir, ws) = workspace();
        // File is tab-indented; model matches with spaces.
        write(&ws, "f.rs", "fn f() {\n\treturn 42;\n}\n").await;
        let err = EditFileTool::new(ws)
            .invoke(serde_json::json!({
                "path": "f.rs",
                "old_string": "    return 42;",
                "new_string": "    return 43;"
            }))
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => assert!(m.contains("whitespace"), "got: {m}"),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_points_at_drifted_anchor_line() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "let total = compute_total();\n").await;
        // The anchor line exists but the model's surrounding context is stale.
        let err = EditFileTool::new(ws)
            .invoke(serde_json::json!({
                "path": "f.rs",
                "old_string": "let total = compute_total();\nprintln!(\"{total}\");",
                "new_string": "let total = compute_total() + 1;\nprintln!(\"{total}\");"
            }))
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => {
                assert!(m.contains("does appear"), "got: {m}");
                assert!(m.contains("compute_total"), "got: {m}");
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_plain_not_found_when_nothing_close() {
        let (_dir, ws) = workspace();
        write(&ws, "f.txt", "alpha beta gamma").await;
        let err = EditFileTool::new(ws)
            .invoke(serde_json::json!({
                "path": "f.txt",
                "old_string": "wholly unrelated content",
                "new_string": "x"
            }))
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => assert_eq!(m, "`old_string` not found in file"),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
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
