//! `read_file` — show the model a file.
//!
//! Line-numbered `cat -n` output, an optional window, and — for a whole-file
//! read of a large source file — a tree-sitter outline instead of every line
//! (see [`super::outline`]). Every read records what it displayed in
//! [`super::state`], which is what later lets an edit tell "you have seen
//! this" from "you are guessing".

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

use super::relative_to;
use super::state::FileState;
use super::{outline, state};
use super::{DEFAULT_READ_LIMIT, MAX_LINE_LEN, MAX_READ_CHARS, READ_FILE_TOOL};

/// Read a UTF-8 text file with `cat -n`-style line numbers.
pub struct ReadFileTool {
    workspace: Workspace,
    state: Arc<FileState>,
}

impl ReadFileTool {
    /// A standalone read tool with its own (ungated) session state.
    pub fn new(workspace: Workspace) -> Self {
        Self::with_state(workspace, FileState::ungated())
    }

    /// A read tool sharing session state with the write/edit tools, so what it
    /// shows the model is what their staleness checks compare against.
    pub fn with_state(workspace: Workspace, state: Arc<FileState>) -> Self {
        Self { workspace, state }
    }

    fn relative(&self, resolved: &Path) -> std::path::PathBuf {
        relative_to(&self.workspace, resolved)
    }
}

/// Arguments to `read_file`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct ReadFileArgs {
    /// Path relative to the workspace root.
    pub path: String,
    /// 1-based line to start reading from.
    pub offset: Option<usize>,
    /// Maximum number of lines to read.
    pub limit: Option<usize>,
}

#[async_trait]
impl TypedTool for ReadFileTool {
    const NAME: &'static str = READ_FILE_TOOL;
    type Args = ReadFileArgs;

    fn description(&self) -> &str {
        "Read a UTF-8 text file relative to the workspace root, line-numbered `cat -n` \
         style (number, tab, content); up to 2000 lines from the start by default. \
         `offset` (1-based) and `limit` return that window verbatim; a whole-file read of \
         a large source file returns an outline with function bodies elided, each marker \
         naming the offset to re-read — and you cannot edit inside an elided range without \
         reading it. NOTE: never include the line-number/tab prefix in `edit_file` \
         arguments — match only the content after the tab."
    }

    async fn run(&self, args: ReadFileArgs) -> Result<String, ToolError> {
        let path = self.workspace.resolve(&args.path)?;

        // A whole-file read of a big source file gets its shape rather than
        // every line. An explicit window is never outlined: the model asked
        // for those lines, so it gets exactly those lines.
        if args.offset.is_none() && args.limit.is_none() {
            if let Some((outline, hash)) = try_outline(&path).await {
                self.state
                    .record_parts(&path, hash, outline.total_lines, outline.seen.clone());
                let rules = self.state.take_rules_for(&self.relative(&path));
                return Ok(format!(
                    "{}… [{} of {} lines shown; bodies elided as marked]\n{rules}",
                    outline.text,
                    outline.total_lines - outline.elided_lines(),
                    outline.total_lines,
                ));
            }
        }

        let (rendered, stats) = read_numbered(
            &path,
            args.offset.unwrap_or(1).max(1),
            args.limit
                .unwrap_or(DEFAULT_READ_LIMIT)
                .min(DEFAULT_READ_LIMIT),
        )
        .await?;
        // Fingerprint the whole file, not the window: the point is to notice
        // that the file moved under the model before it edits, whichever part
        // of it was displayed.
        self.state.record_parts(
            &path,
            stats.hash,
            stats.lines,
            stats.shown.into_iter().collect(),
        );
        // Consumed only now: a read that failed shouldn't burn a rule that
        // fires once per session.
        Ok(rendered + &self.state.take_rules_for(&self.relative(&path)))
    }
}

/// Parse `path` into an outline, when it's a language we ship a grammar for
/// and small enough to hold in memory. Returns the outline and the file's
/// fingerprint (computed from the same read, so no second pass).
async fn try_outline(path: &Path) -> Option<(outline::Outline, u64)> {
    let size = tokio::fs::metadata(path).await.ok()?.len();
    if size > outline::MAX_OUTLINE_BYTES {
        return None;
    }
    let source = tokio::fs::read_to_string(path).await.ok()?;
    let hash = state::fingerprint(source.as_bytes());
    outline::summarize(path, &source).map(|o| (o, hash))
}

/// What one read learned about the file: its whole-file identity, its length,
/// and which lines actually reached the model.
struct ReadStats {
    hash: u64,
    lines: usize,
    /// 1-based inclusive range displayed; `None` when the read rendered no
    /// lines at all (an offset past the end of the file).
    shown: Option<(usize, usize)>,
}

async fn read_numbered(
    path: &Path,
    offset: usize,
    limit: usize,
) -> Result<(String, ReadStats), ToolError> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| ToolError::Execution(format!("read {}: {e}", path.display())))?;
    let mut chunk = [0u8; 8192];
    let mut line = Vec::with_capacity(MAX_LINE_LEN.min(8192));
    let mut out = String::new();
    let mut line_no = 1usize;
    let mut total = 0usize;
    let mut saw_bytes = false;
    let mut truncated_line = false;
    let mut output_truncated = false;
    let mut last_was_newline = false;
    let mut hasher = state::StreamingFingerprint::default();
    // The last line actually written into `out`, which is what the model sees.
    let mut last_shown: Option<usize> = None;

    loop {
        let n = file
            .read(&mut chunk)
            .await
            .map_err(|e| ToolError::Execution(format!("read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        saw_bytes = true;
        hasher.update(&chunk[..n]);
        for &byte in &chunk[..n] {
            last_was_newline = byte == b'\n';
            if byte == b'\n' {
                append_numbered_line(
                    &mut out,
                    line_no,
                    offset,
                    limit,
                    &line,
                    truncated_line,
                    &mut output_truncated,
                    &mut last_shown,
                );
                line.clear();
                truncated_line = false;
                total += 1;
                line_no += 1;
            } else if line.len() < MAX_LINE_LEN * 4 {
                line.push(byte);
            } else {
                truncated_line = true;
            }
        }
    }
    if saw_bytes && !last_was_newline {
        append_numbered_line(
            &mut out,
            line_no,
            offset,
            limit,
            &line,
            truncated_line,
            &mut output_truncated,
            &mut last_shown,
        );
        total += 1;
    }
    let shown_end = (offset.saturating_sub(1) + limit).min(total);
    let stats = ReadStats {
        hash: hasher.finish(),
        lines: total,
        // What was *rendered*, not what was requested: a read past the end of
        // the file, or one cut short by the character cap, displays fewer
        // lines than the window asked for, and claiming otherwise would let an
        // edit land on content the model never saw.
        shown: last_shown.map(|last| (offset, last)),
    };
    if !saw_bytes {
        return Ok(("(file is empty)".to_string(), stats));
    }
    if offset > total {
        return Ok((
            format!(
                "(offset {offset} is past the end of the file, which has {total} line{})",
                if total == 1 { "" } else { "s" }
            ),
            stats,
        ));
    }
    if shown_end < total || output_truncated {
        out.push_str(&format!(
            "… [showing lines {offset}-{shown_end} of {total}{}]\n",
            if output_truncated {
                "; output capped"
            } else {
                "; pass offset to read more"
            }
        ));
    }
    Ok((out, stats))
}

#[allow(clippy::too_many_arguments)]
fn append_numbered_line(
    out: &mut String,
    line_no: usize,
    offset: usize,
    limit: usize,
    bytes: &[u8],
    truncated_line: bool,
    output_truncated: &mut bool,
    last_shown: &mut Option<usize>,
) {
    if line_no < offset || line_no >= offset.saturating_add(limit) || *output_truncated {
        return;
    }
    let text = String::from_utf8_lossy(bytes);
    let mut shown: String = text.chars().take(MAX_LINE_LEN).collect();
    if truncated_line || text.chars().count() > MAX_LINE_LEN {
        shown.push_str("… [line truncated]");
    }
    let rendered = format!("{line_no:>6}\t{shown}\n");
    if out.chars().count() + rendered.chars().count() > MAX_READ_CHARS {
        *output_truncated = true;
    } else {
        out.push_str(&rendered);
        *last_shown = Some(line_no);
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::fs::testkit::*;
    use crate::fs::PathRule;
    use crate::{ToolError, TypedTool};

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
    async fn a_big_source_file_reads_as_an_outline() {
        let (_dir, ws) = workspace();
        write(&ws, "config.rs", &long_source()).await;

        let out = ReadFileTool::new(ws)
            .invoke(serde_json::json!({"path": "config.rs"}))
            .await
            .unwrap();

        assert!(out.contains("pub struct Config"));
        assert!(out.contains("pub fn load() -> Self {"));
        assert!(!out.contains("let step_100 = 100;"));
        assert!(out.contains("lines elided"));
    }

    #[tokio::test]
    async fn an_explicit_window_is_never_outlined() {
        let (_dir, ws) = workspace();
        write(&ws, "config.rs", &long_source()).await;

        let out = ReadFileTool::new(ws)
            .invoke(serde_json::json!({"path": "config.rs", "offset": 100, "limit": 20}))
            .await
            .unwrap();

        // Asking for lines means getting lines.
        assert!(out.contains("let step_94 = 94;"), "{out}");
        assert!(!out.contains("lines elided"));
    }

    #[tokio::test]
    async fn a_read_that_displayed_nothing_grants_no_licence_to_edit() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "let a = 1;\nlet b = 2;\n").await;
        let (read, _write, edit, _state) = gated_tools(&ws);

        // A read past the end renders no lines — it must not count as having
        // seen the file, or the window arithmetic would hand the model a
        // licence to edit content it never received.
        let out = read
            .invoke(serde_json::json!({"path": "f.rs", "offset": 50}))
            .await
            .unwrap();
        assert!(out.contains("past the end"), "{out}");

        let err = edit
            .invoke(serde_json::json!({
                "path": "f.rs", "old_string": "let a = 1;", "new_string": "let a = 9;"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
        assert_eq!(read_raw(&ws, "f.rs").await, "let a = 1;\nlet b = 2;\n");
    }

    #[tokio::test]
    async fn a_convention_fires_for_an_absolute_path_too() {
        let (_dir, ws) = workspace();
        write(&ws, "app/main.tsx", "export const App = () => null;\n").await;
        let (read, _write, _edit, state) = gated_tools(&ws);
        state.set_rules(vec![PathRule::new(
            "app/AGENTS.md",
            "Frontend: no default exports.",
            &["app/**".to_string()],
        )]);

        // A model that pasted an absolute path out of a grep must still see
        // the convention governing that file.
        let absolute = ws.resolve("app/main.tsx").unwrap();
        let out = read
            .invoke(serde_json::json!({"path": absolute.to_string_lossy()}))
            .await
            .unwrap();

        assert!(out.contains("no default exports"), "got: {out}");
    }

    #[tokio::test]
    async fn a_failed_read_does_not_burn_a_once_per_session_convention() {
        let (_dir, ws) = workspace();
        write(&ws, "app/real.tsx", "export const App = () => null;\n").await;
        let (read, _write, _edit, state) = gated_tools(&ws);
        state.set_rules(vec![PathRule::new(
            "app/AGENTS.md",
            "Frontend: no default exports.",
            &["app/**".to_string()],
        )]);

        // The file doesn't exist, so the read fails…
        read.invoke(serde_json::json!({"path": "app/missing.tsx"}))
            .await
            .unwrap_err();
        // …and the convention is still waiting for the next real read.
        let out = read
            .invoke(serde_json::json!({"path": "app/real.tsx"}))
            .await
            .unwrap();
        assert!(out.contains("no default exports"), "got: {out}");
    }
}
