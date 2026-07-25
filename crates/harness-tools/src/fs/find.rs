//! `find_files` and `search_files` — locating things.
//!
//! Glob discovery and ripgrep-style content search, both in-process, both
//! honouring `.gitignore`, and both confined to the workspace sandbox.

use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

use globset::GlobBuilder;
use regex::RegexBuilder;

use super::{
    DEFAULT_MAX_RESULTS, FIND_FILES_TOOL, MAX_LINE_LEN, MAX_SEARCH_FILE_BYTES, SEARCH_FILES_TOOL,
};

/// Find files by glob pattern (the agent's `Glob`), respecting `.gitignore`.
pub struct FindFilesTool {
    workspace: Workspace,
}

impl FindFilesTool {
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

/// Arguments to `find_files`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct FindFilesArgs {
    /// Glob pattern, e.g. `**/*.rs`.
    pub pattern: String,
    /// Cap on the number of paths returned (default 200).
    pub max_results: Option<usize>,
}

#[async_trait]
impl TypedTool for FindFilesTool {
    const NAME: &'static str = FIND_FILES_TOOL;
    type Args = FindFilesArgs;

    fn description(&self) -> &str {
        "Find files by glob pattern relative to the workspace root, e.g. `**/*.rs`, \
         `src/**/*.ts`, `*.toml`. `*` does not cross directory boundaries; use `**` to \
         recurse. Respects .gitignore. Returns paths, most-recently-modified first."
    }

    async fn run(&self, args: FindFilesArgs) -> Result<String, ToolError> {
        let pattern = args.pattern;
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(DEFAULT_MAX_RESULTS);
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

    hits.sort_by_key(|hit| std::cmp::Reverse(hit.0));
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

/// Arguments to `search_files`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// Regular expression to search for.
    pub pattern: String,
    /// Subdirectory or file to restrict the search to.
    pub path: Option<String>,
    /// Filename filter, e.g. `*.rs` or `**/*.ts`.
    pub glob: Option<String>,
    /// How to report matches (default `content`).
    #[serde(default)]
    pub output_mode: OutputMode,
    /// Case-insensitive matching.
    #[serde(default)]
    pub case_insensitive: bool,
    /// Cap on the number of result lines (default 200).
    pub max_results: Option<usize>,
}

#[async_trait]
impl TypedTool for SearchTool {
    const NAME: &'static str = SEARCH_FILES_TOOL;
    type Args = SearchArgs;

    fn description(&self) -> &str {
        "Search workspace file contents with a regular expression (ripgrep-style; respects \
         .gitignore). `output_mode` is `content` (default — `path:line:text`), \
         `files_with_matches` (just paths), or `count` (per-file match counts). Optionally \
         restrict with `path` (a subdir or file) and `glob` (a filename filter like `*.rs`)."
    }

    async fn run(&self, args: SearchArgs) -> Result<String, ToolError> {
        let pattern = args.pattern;
        let glob = args.glob;
        let mode = args.output_mode;
        let case_insensitive = args.case_insensitive;
        let max_results = args.max_results.unwrap_or(DEFAULT_MAX_RESULTS);
        let root = self.workspace.root().to_path_buf();
        // `path` is resolved through the sandbox so it cannot escape the workspace.
        let search_root = match &args.path {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// `path:line:text` for every matching line (default).
    #[default]
    Content,
    /// Just the paths of files containing a match.
    FilesWithMatches,
    /// `path:count` of matching lines per file.
    Count,
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
        if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_SEARCH_FILE_BYTES) {
            continue;
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
                        let shown: String = line.trim_end().chars().take(MAX_LINE_LEN).collect();
                        out.push(format!("{}:{}:{shown}", rel.display(), i + 1));
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
    use crate::fs::testkit::*;
    use crate::TypedTool;

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
