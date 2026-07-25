//! Filesystem tools: read, write, edit, find (glob), and search (grep).
//!
//! These mirror the essential file primitives a strong coding agent expects
//! (read with line numbers + offset/limit, exact-string edit, glob file
//! discovery, and ripgrep-style regex search), all confined to the
//! [`Workspace`] sandbox.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use globset::GlobBuilder;
use regex::RegexBuilder;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

mod edit_diagnostics;
pub mod outline;
pub mod state;
pub mod syntax;

pub use state::{FileState, Freshness, PathRule};

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
/// Total text returned by one read, independent of the requested line count.
const MAX_READ_CHARS: usize = 100_000;
/// Files larger than this are skipped by the in-process regex search.
const MAX_SEARCH_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Default cap on `find_files` / `search_files` results.
const DEFAULT_MAX_RESULTS: usize = 200;

/// A resolved path expressed relative to the workspace root.
///
/// Path-scoped conventions are written as globs against the project (`app/**`),
/// so they must be matched against that shape — not the model's raw argument,
/// which `Workspace::resolve` also accepts as absolute or `./`-prefixed. A
/// model that pasted an absolute path out of a grep would otherwise never see
/// the convention governing it.
fn relative_to(workspace: &Workspace, resolved: &Path) -> std::path::PathBuf {
    resolved
        .strip_prefix(workspace.root())
        .unwrap_or(resolved)
        .to_path_buf()
}

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

/// Create or overwrite a text file, creating parent directories as needed.
pub struct WriteFileTool {
    workspace: Workspace,
    state: Arc<FileState>,
}

impl WriteFileTool {
    /// A standalone write tool with its own (ungated) session state.
    pub fn new(workspace: Workspace) -> Self {
        Self::with_state(workspace, FileState::ungated())
    }

    /// A write tool sharing session state, so overwriting an existing file is
    /// held to the same freshness contract as editing one.
    pub fn with_state(workspace: Workspace, state: Arc<FileState>) -> Self {
        Self { workspace, state }
    }

    fn relative(&self, resolved: &Path) -> std::path::PathBuf {
        relative_to(&self.workspace, resolved)
    }
}

/// Arguments to `write_file`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct WriteFileArgs {
    /// Path relative to the workspace root; parent directories are created.
    pub path: String,
    /// The full file contents to write.
    pub contents: String,
}

#[async_trait]
impl TypedTool for WriteFileTool {
    const NAME: &'static str = WRITE_FILE_TOOL;
    type Args = WriteFileArgs;

    fn description(&self) -> &str {
        "Create or overwrite a text file at a path relative to the workspace root. \
         Overwriting an existing file requires having read it first (use `edit_file` \
         for changes to part of a file)."
    }

    async fn run(&self, args: WriteFileArgs) -> Result<String, ToolError> {
        let path = self.workspace.resolve(&args.path)?;
        // Held across the check and the write so a concurrent lane can't slip
        // a change in between them.
        let _guard = self.state.lock(&path).await;
        // Creating a new file is unrestricted; replacing one wholesale is an
        // edit by another name and answers to the same contract.
        let before = tokio::fs::read_to_string(&path).await.ok();
        if let Some(current) = &before {
            self.state.guard(&args.path, &path, current)?;
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &args.contents)
            .await
            .map_err(|e| ToolError::Execution(format!("write {}: {e}", path.display())))?;
        // The model has now seen this content — it wrote it — so a follow-up
        // edit in the same turn doesn't have to re-read first.
        self.state.record(&path, &args.contents);
        let mut result = format!("wrote {} bytes to {}", args.contents.len(), path.display());
        if let Some(note) = syntax::regression_note(&path, before.as_deref(), &args.contents) {
            result.push_str(&format!("\n{note}"));
        }
        result.push_str(&self.state.take_rules_for(&self.relative(&path)));
        Ok(result)
    }
}

/// Replace an exact, unique string in a file (like a precise patch).
pub struct EditFileTool {
    workspace: Workspace,
    state: Arc<FileState>,
}

impl EditFileTool {
    /// A standalone edit tool with its own (ungated) session state.
    pub fn new(workspace: Workspace) -> Self {
        Self::with_state(workspace, FileState::ungated())
    }

    /// An edit tool sharing session state with `read_file`, so it can tell a
    /// file the model has seen from one that moved underneath it.
    pub fn with_state(workspace: Workspace, state: Arc<FileState>) -> Self {
        Self { workspace, state }
    }

    fn relative(&self, resolved: &Path) -> std::path::PathBuf {
        relative_to(&self.workspace, resolved)
    }
}

/// One replacement within an `edit_file` call.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct Replacement {
    /// Exact text to find (the real file content — no line-number prefix).
    pub old_string: String,
    /// The replacement text.
    pub new_string: String,
    /// Replace every occurrence instead of requiring a unique match.
    #[serde(default)]
    pub replace_all: bool,
}

/// Arguments to `edit_file`: either one `old_string`/`new_string` pair, or a
/// batch of them in `edits`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct EditFileArgs {
    /// Path relative to the workspace root.
    pub path: String,
    /// Exact text to find, for a single replacement.
    pub old_string: Option<String>,
    /// The replacement text, for a single replacement.
    pub new_string: Option<String>,
    /// Several replacements applied together, each matched against the
    /// original file. They must not overlap.
    pub edits: Option<Vec<Replacement>>,
    /// Replace every occurrence instead of requiring a unique match.
    #[serde(default)]
    pub replace_all: bool,
}

impl EditFileArgs {
    /// Collapse the two accepted shapes into one list. Accepting both keeps
    /// the common one-line fix a one-liner while letting a rename land its six
    /// call sites in a single call instead of six round-trips.
    fn replacements(self) -> Result<Vec<Replacement>, ToolError> {
        match (self.edits, self.old_string, self.new_string) {
            (Some(edits), None, None) if !edits.is_empty() => Ok(edits),
            (None, Some(old_string), Some(new_string)) => Ok(vec![Replacement {
                old_string,
                new_string,
                replace_all: self.replace_all,
            }]),
            (Some(_), _, _) => Err(ToolError::InvalidArguments(
                "pass either `edits` or a single `old_string`/`new_string` pair, not both".into(),
            )),
            _ => Err(ToolError::InvalidArguments(
                "`edit_file` needs either `edits` or both `old_string` and `new_string`".into(),
            )),
        }
    }
}

#[async_trait]
impl TypedTool for EditFileTool {
    const NAME: &'static str = EDIT_FILE_TOOL;
    type Args = EditFileArgs;

    fn description(&self) -> &str {
        "Replace exact text in a file you have already read. One change: pass \
         `old_string` + `new_string`. Several changes to the same file: pass `edits` \
         (each matched against the original, non-overlapping, applied together or not \
         at all) — one call beats one call per change. Each `old_string` must match \
         exactly once unless `replace_all` is set, and must be the real file content: \
         do NOT include the line-number/tab prefix `read_file` adds."
    }

    async fn run(&self, args: EditFileArgs) -> Result<String, ToolError> {
        let path = self.workspace.resolve(&args.path)?;
        let display = args.path.clone();
        let edits = args.replacements()?;

        // Held for the whole read-modify-write: two fleet lanes editing one
        // file queue up instead of both writing over the same original.
        let _guard = self.state.lock(&path).await;
        let raw = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::Execution(format!("read {}: {e}", path.display())))?;
        self.state.guard(&display, &path, &raw)?;

        // Match against LF text so an `old_string` copied out of a read still
        // matches in a CRLF file; the file's own endings are restored on write.
        let (body, endings) = LineEndings::split(&raw);
        let outcome = apply_edits(&body, &edits)?;
        // An outlined read hides function bodies. Editing into one is editing
        // code nobody looked at — the exact mistake the outline saves tokens
        // by risking, so it's caught here rather than written.
        for &(first, last) in &outcome.touched_lines {
            if let Some((from, to)) = self.state.unseen_around(&path, first, last) {
                return Err(ToolError::InvalidArguments(format!(
                    "lines {from}-{to} of {display} were elided in your read, so you \
                     haven't seen what you're replacing. Read that range first \
                     (`read_file` with offset={from}), then edit."
                )));
            }
        }
        let updated = endings.restore(&outcome.text);

        tokio::fs::write(&path, &updated)
            .await
            .map_err(|e| ToolError::Execution(format!("write {}: {e}", path.display())))?;
        // Re-anchor on what's now on disk, so a follow-up edit this turn is
        // measured against the model's own change rather than the stale read —
        // carrying the seen-ranges across into the new line numbering, rather
        // than granting sight of the whole file.
        let seen = state::remap_seen(&self.state.seen_ranges(&path), &outcome.applied);
        self.state.record_parts(
            &path,
            state::fingerprint(updated.as_bytes()),
            updated.lines().count(),
            seen,
        );
        let mut result = format!("edited {} ({})", path.display(), outcome.summary);
        // A patch that drops a brace is the most common way an edit goes
        // wrong, and nothing else notices until a build runs.
        if let Some(note) = syntax::regression_note(&path, Some(&body), &outcome.text) {
            result.push_str(&format!("\n{note}"));
        }
        result.push_str(&self.state.take_rules_for(&self.relative(&path)));
        Ok(result)
    }
}

/// The line endings and BOM a file arrived with, so an edit doesn't silently
/// rewrite every line of a CRLF file (and blow up the diff its author reads).
struct LineEndings {
    crlf: bool,
    bom: bool,
}

impl LineEndings {
    fn split(raw: &str) -> (String, Self) {
        let bom = raw.starts_with('\u{feff}');
        let body = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let crlf = body.contains("\r\n");
        let normalized = if crlf {
            body.replace("\r\n", "\n")
        } else {
            body.to_string()
        };
        (normalized, Self { crlf, bom })
    }

    fn restore(&self, text: &str) -> String {
        let body = if self.crlf {
            text.replace('\n', "\r\n")
        } else {
            text.to_string()
        };
        if self.bom {
            format!("\u{feff}{body}")
        } else {
            body
        }
    }
}

/// What an applied batch produced.
struct EditOutcome {
    text: String,
    summary: String,
    /// 1-based inclusive line ranges of the original that each hunk covered,
    /// for checking them against what the model was actually shown.
    touched_lines: Vec<(usize, usize)>,
    /// The same hunks with their replacement sizes, for moving the recorded
    /// seen-ranges into the edited file's coordinates.
    applied: Vec<state::AppliedEdit>,
}

/// The 1-based line range a byte span covers in `text`.
///
/// Counts newline *bytes* rather than slicing: `end - 1` lands mid-character
/// whenever the match ends on a multi-byte one (`old_string: "café"`), and
/// slicing there panics — which aborts the turn, since tool calls aren't
/// unwind-guarded. A newline can't occur inside a UTF-8 sequence, so counting
/// bytes is equivalent and total.
fn line_span(text: &str, start: usize, end: usize) -> (usize, usize) {
    let line_of = |offset: usize| {
        text.as_bytes()[..offset.min(text.len())]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1
    };
    (line_of(start), line_of(end.saturating_sub(1).max(start)))
}

/// Apply every replacement against the *original* text, atomically: each match
/// is resolved before anything is written, so a batch with one bad hunk leaves
/// the file untouched rather than half-edited.
fn apply_edits(original: &str, edits: &[Replacement]) -> Result<EditOutcome, ToolError> {
    let many = edits.len() > 1;
    // (start, end, replacement, edit index) over the original text.
    let mut spans: Vec<(usize, usize, &str, usize)> = Vec::new();
    let mut replaced_lines = 0usize;
    let mut inserted_lines = 0usize;

    for (i, edit) in edits.iter().enumerate() {
        let label = |msg: String| {
            if many {
                ToolError::InvalidArguments(format!("edit {}: {msg}", i + 1))
            } else {
                ToolError::InvalidArguments(msg)
            }
        };
        if edit.old_string == edit.new_string {
            return Err(label(
                "`old_string` and `new_string` are identical; the edit would do nothing".into(),
            ));
        }
        if edit.old_string.is_empty() {
            return Err(label("`old_string` is empty; nothing to match".into()));
        }
        let hits: Vec<usize> = original
            .match_indices(&edit.old_string)
            .map(|(at, _)| at)
            .collect();
        match hits.len() {
            0 => {
                return Err(label(edit_diagnostics::diagnose_no_match(
                    original,
                    &edit.old_string,
                )))
            }
            1 => {}
            n if edit.replace_all => {
                let _ = n;
            }
            n => {
                return Err(label(format!(
                    "`old_string` matches {n} times; pass replace_all=true or add more context"
                )))
            }
        }
        for at in hits {
            spans.push((at, at + edit.old_string.len(), &edit.new_string, i));
            replaced_lines += edit.old_string.lines().count();
            inserted_lines += edit.new_string.lines().count();
        }
    }

    // Overlapping hunks are the classic way a batch corrupts a file: two edits
    // both "succeed" and the second one's context is already gone.
    spans.sort_by_key(|(start, ..)| *start);
    for pair in spans.windows(2) {
        let (_, first_end, _, first_idx) = pair[0];
        let (second_start, _, _, second_idx) = pair[1];
        if second_start < first_end {
            return Err(ToolError::InvalidArguments(format!(
                "edits {} and {} overlap in the file; merge them into one edit",
                first_idx + 1,
                second_idx + 1
            )));
        }
    }

    let touched_lines: Vec<(usize, usize)> = spans
        .iter()
        .map(|&(start, end, ..)| line_span(original, start, end))
        .collect();
    let applied: Vec<state::AppliedEdit> = spans
        .iter()
        .zip(&touched_lines)
        .map(
            |(&(.., replacement, _), &(first_line, last_line))| state::AppliedEdit {
                first_line,
                last_line,
                new_lines: replacement.lines().count().max(1),
            },
        )
        .collect();

    // Back to front, so earlier offsets stay valid as later ones are spliced.
    let mut text = original.to_string();
    for &(start, end, replacement, _) in spans.iter().rev() {
        text.replace_range(start..end, replacement);
    }

    let summary = if many {
        format!(
            "{} edits, {replaced_lines} lines replaced by {inserted_lines}",
            edits.len()
        )
    } else if spans.len() > 1 {
        format!("{} replacements", spans.len())
    } else {
        "1 replacement".to_string()
    };
    Ok(EditOutcome {
        text,
        summary,
        touched_lines,
        applied,
    })
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

    /// A Rust file long enough to outline: a visible signature, a long body.
    fn long_source() -> String {
        let mut src = String::from("pub struct Config {\n    pub name: String,\n}\n\nimpl Config {\n    pub fn load() -> Self {\n");
        for i in 0..250 {
            src.push_str(&format!("        let step_{i} = {i};\n"));
        }
        src.push_str("        Self { name: String::new() }\n    }\n}\n");
        src
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
    async fn editing_into_an_elided_body_is_refused_with_the_range_to_read() {
        let (_dir, ws) = workspace();
        write(&ws, "config.rs", &long_source()).await;
        let (read, _write, edit, _state) = gated_tools(&ws);
        read.invoke(serde_json::json!({"path": "config.rs"}))
            .await
            .unwrap();

        // A line the outline hid: the model is guessing, not editing.
        let err = edit
            .invoke(serde_json::json!({
                "path": "config.rs",
                "old_string": "let step_100 = 100;",
                "new_string": "let step_100 = 999;"
            }))
            .await
            .unwrap_err();

        match err {
            ToolError::InvalidArguments(m) => {
                assert!(m.contains("elided in your read"), "got: {m}");
                assert!(m.contains("offset="), "got: {m}");
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }

        // Reading the range makes the same edit legal.
        read.invoke(serde_json::json!({"path": "config.rs", "offset": 1, "limit": 300}))
            .await
            .unwrap();
        edit.invoke(serde_json::json!({
            "path": "config.rs",
            "old_string": "let step_100 = 100;",
            "new_string": "let step_100 = 999;"
        }))
        .await
        .unwrap();
        assert!(read_raw(&ws, "config.rs").await.contains("step_100 = 999"));
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
    async fn a_windowed_read_licenses_only_the_lines_it_showed() {
        let (_dir, ws) = workspace();
        let body: String = (1..=40).map(|i| format!("let v{i} = {i};\n")).collect();
        write(&ws, "f.rs", &body).await;
        let (read, _write, edit, _state) = gated_tools(&ws);

        read.invoke(serde_json::json!({"path": "f.rs", "offset": 1, "limit": 10}))
            .await
            .unwrap();

        // Inside the window: fine.
        edit.invoke(serde_json::json!({
            "path": "f.rs", "old_string": "let v3 = 3;", "new_string": "let v3 = 33;"
        }))
        .await
        .unwrap();
        // Outside it: the model is guessing.
        let err = edit
            .invoke(serde_json::json!({
                "path": "f.rs", "old_string": "let v30 = 30;", "new_string": "let v30 = 99;"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn editing_a_line_the_outline_showed_is_allowed() {
        let (_dir, ws) = workspace();
        write(&ws, "config.rs", &long_source()).await;
        let (read, _write, edit, _state) = gated_tools(&ws);
        read.invoke(serde_json::json!({"path": "config.rs"}))
            .await
            .unwrap();

        edit.invoke(serde_json::json!({
            "path": "config.rs",
            "old_string": "pub name: String,",
            "new_string": "pub name: Box<str>,"
        }))
        .await
        .unwrap();

        assert!(read_raw(&ws, "config.rs").await.contains("Box<str>"));
    }

    #[tokio::test]
    async fn an_edit_that_breaks_the_file_says_so_on_the_result() {
        let (_dir, ws) = workspace();
        write(&ws, "m.rs", "fn main() {\n    let x = 1;\n}\n").await;

        // Dropping the closing brace is the classic bad patch.
        let out = EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({
                "path": "m.rs", "old_string": "    let x = 1;\n}", "new_string": "    let x = 1;"
            }))
            .await
            .unwrap();

        assert!(out.contains("syntax error"), "{out}");
        assert!(out.contains("edited"), "the edit still applied: {out}");
    }

    #[tokio::test]
    async fn a_clean_edit_is_not_second_guessed() {
        let (_dir, ws) = workspace();
        write(&ws, "m.rs", "fn main() {\n    let x = 1;\n}\n").await;

        let out = EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({
                "path": "m.rs", "old_string": "let x = 1;", "new_string": "let x = 2;"
            }))
            .await
            .unwrap();

        assert!(!out.contains("syntax error"), "{out}");
    }

    #[tokio::test]
    async fn an_edit_ending_on_a_multibyte_character_is_applied_not_a_crash() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "let who = café;\nlet n = 1;\n").await;

        // The match ends mid-way through é's byte sequence; computing its line
        // span used to slice there and panic, which aborts the whole turn.
        EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({
                "path": "f.rs", "old_string": "café", "new_string": "tea"
            }))
            .await
            .unwrap();

        assert_eq!(read_raw(&ws, "f.rs").await, "let who = tea;\nlet n = 1;\n");
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

    #[tokio::test]
    async fn a_batch_of_edits_lands_in_one_call() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "let a = 1;\nlet b = 2;\nlet c = 3;\n").await;

        let out = EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({
                "path": "f.rs",
                "edits": [
                    {"old_string": "let a = 1;", "new_string": "let a = 10;"},
                    {"old_string": "let c = 3;", "new_string": "let c = 30;"},
                ]
            }))
            .await
            .unwrap();

        assert_eq!(
            read_raw(&ws, "f.rs").await,
            "let a = 10;\nlet b = 2;\nlet c = 30;\n"
        );
        assert!(out.contains("2 edits"), "got: {out}");
    }

    #[tokio::test]
    async fn a_batch_is_all_or_nothing() {
        let (_dir, ws) = workspace();
        let before = "let a = 1;\nlet b = 2;\n";
        write(&ws, "f.rs", before).await;

        let err = EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({
                "path": "f.rs",
                "edits": [
                    {"old_string": "let a = 1;", "new_string": "let a = 10;"},
                    {"old_string": "let nope = 0;", "new_string": "let nope = 1;"},
                ]
            }))
            .await
            .unwrap_err();

        match err {
            // The failing edit is named, so the model knows which one to fix.
            ToolError::InvalidArguments(m) => assert!(m.starts_with("edit 2:"), "got: {m}"),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
        // The good edit did not land — a half-applied batch is worse than none.
        assert_eq!(read_raw(&ws, "f.rs").await, before);
    }

    #[tokio::test]
    async fn overlapping_edits_are_refused() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "let total = compute(a, b);\n").await;

        let err = EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({
                "path": "f.rs",
                "edits": [
                    {"old_string": "let total = compute(a, b);", "new_string": "let total = 0;"},
                    {"old_string": "compute(a, b)", "new_string": "compute(b, a)"},
                ]
            }))
            .await
            .unwrap_err();

        match err {
            ToolError::InvalidArguments(m) => assert!(m.contains("overlap"), "got: {m}"),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edits_are_matched_against_the_original_not_each_other() {
        let (_dir, ws) = workspace();
        // A swap: applied incrementally these would chase each other's output.
        write(&ws, "f.rs", "alpha\nbeta\n").await;

        EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({
                "path": "f.rs",
                "edits": [
                    {"old_string": "alpha", "new_string": "beta"},
                    {"old_string": "beta", "new_string": "alpha"},
                ]
            }))
            .await
            .unwrap();

        assert_eq!(read_raw(&ws, "f.rs").await, "beta\nalpha\n");
    }

    #[tokio::test]
    async fn a_crlf_file_keeps_its_line_endings() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "let a = 1;\r\nlet b = 2;\r\n").await;

        // The model's `old_string` comes from a read, which shows LF.
        EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({
                "path": "f.rs", "old_string": "let a = 1;\nlet b = 2;",
                "new_string": "let a = 9;\nlet b = 8;"
            }))
            .await
            .unwrap();

        assert_eq!(read_raw(&ws, "f.rs").await, "let a = 9;\r\nlet b = 8;\r\n");
    }

    #[tokio::test]
    async fn a_byte_order_mark_survives_an_edit() {
        let (_dir, ws) = workspace();
        write(&ws, "f.txt", "\u{feff}hello world").await;

        EditFileTool::new(ws.clone())
            .invoke(serde_json::json!({
                "path": "f.txt", "old_string": "world", "new_string": "there"
            }))
            .await
            .unwrap();

        assert_eq!(read_raw(&ws, "f.txt").await, "\u{feff}hello there");
    }

    #[tokio::test]
    async fn mixing_the_single_and_batch_forms_is_rejected() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "x").await;

        let err = EditFileTool::new(ws)
            .invoke(serde_json::json!({
                "path": "f.rs",
                "old_string": "x",
                "new_string": "y",
                "edits": [{"old_string": "x", "new_string": "z"}]
            }))
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    /// The shipped registry's shape: read/write/edit behind one gated state.
    fn gated_tools(ws: &Workspace) -> (ReadFileTool, WriteFileTool, EditFileTool, Arc<FileState>) {
        let state = FileState::gated();
        (
            ReadFileTool::with_state(ws.clone(), state.clone()),
            WriteFileTool::with_state(ws.clone(), state.clone()),
            EditFileTool::with_state(ws.clone(), state.clone()),
            state,
        )
    }

    #[tokio::test]
    async fn editing_a_file_the_model_never_read_is_refused() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "let x = 1;\n").await;
        let (_read, _write, edit, _state) = gated_tools(&ws);

        let err = edit
            .invoke(serde_json::json!({
                "path": "f.rs", "old_string": "let x = 1;", "new_string": "let x = 2;"
            }))
            .await
            .unwrap_err();

        match err {
            ToolError::InvalidArguments(m) => assert!(m.contains("read_file"), "got: {m}"),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
        // And nothing was written.
        assert_eq!(read_raw(&ws, "f.rs").await, "let x = 1;\n");
    }

    #[tokio::test]
    async fn editing_after_reading_is_allowed_and_re_anchors_for_the_next_edit() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "let x = 1;\nlet y = 2;\n").await;
        let (read, _write, edit, _state) = gated_tools(&ws);

        read.invoke(serde_json::json!({"path": "f.rs"}))
            .await
            .unwrap();
        edit.invoke(serde_json::json!({
            "path": "f.rs", "old_string": "let x = 1;", "new_string": "let x = 9;"
        }))
        .await
        .unwrap();
        // The model's own edit doesn't make the file "stale" for the next one.
        edit.invoke(serde_json::json!({
            "path": "f.rs", "old_string": "let y = 2;", "new_string": "let y = 8;"
        }))
        .await
        .unwrap();

        assert_eq!(read_raw(&ws, "f.rs").await, "let x = 9;\nlet y = 8;\n");
    }

    #[tokio::test]
    async fn an_edit_onto_content_that_moved_underneath_is_refused() {
        let (_dir, ws) = workspace();
        write(&ws, "f.rs", "let x = 1;\n").await;
        let (read, _write, edit, _state) = gated_tools(&ws);
        read.invoke(serde_json::json!({"path": "f.rs"}))
            .await
            .unwrap();
        // The user (or a formatter, or another lane) rewrites the file.
        tokio::fs::write(ws.resolve("f.rs").unwrap(), "let x = 1;\nlet extra = 0;\n")
            .await
            .unwrap();

        let err = edit
            .invoke(serde_json::json!({
                "path": "f.rs", "old_string": "let x = 1;", "new_string": "let x = 2;"
            }))
            .await
            .unwrap_err();

        match err {
            ToolError::InvalidArguments(m) => {
                assert!(m.contains("changed on disk"), "got: {m}");
                assert!(m.contains("1 lines then, 2 now"), "got: {m}");
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
        // The other party's line survives.
        assert!(read_raw(&ws, "f.rs").await.contains("let extra = 0;"));
    }

    #[tokio::test]
    async fn creating_a_file_is_free_but_overwriting_an_unread_one_is_not() {
        let (_dir, ws) = workspace();
        let (_read, write_tool, _edit, _state) = gated_tools(&ws);

        // A brand new file: nothing to revert, nothing to check.
        write_tool
            .invoke(serde_json::json!({"path": "new.rs", "contents": "fn main() {}"}))
            .await
            .unwrap();

        // An existing file the model hasn't seen: same contract as an edit.
        write(&ws, "old.rs", "important\n").await;
        let err = write_tool
            .invoke(serde_json::json!({"path": "old.rs", "contents": "clobbered"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
        assert_eq!(read_raw(&ws, "old.rs").await, "important\n");
    }

    #[tokio::test]
    async fn concurrent_edits_to_one_file_both_land() {
        let (_dir, ws) = workspace();
        write(&ws, "shared.rs", "alpha\nbeta\n").await;
        let (read, _write, edit, state) = gated_tools(&ws);
        read.invoke(serde_json::json!({"path": "shared.rs"}))
            .await
            .unwrap();

        // Two lanes, one file. Without the per-path lock these interleave and
        // the second write reverts the first.
        let one = EditFileTool::with_state(ws.clone(), state.clone());
        let two = EditFileTool::with_state(ws.clone(), state.clone());
        let (a, b) = tokio::join!(
            one.invoke(
                serde_json::json!({"path": "shared.rs", "old_string": "alpha", "new_string": "ALPHA"})
            ),
            two.invoke(
                serde_json::json!({"path": "shared.rs", "old_string": "beta", "new_string": "BETA"})
            ),
        );
        a.unwrap();
        b.unwrap();
        drop(edit);

        assert_eq!(read_raw(&ws, "shared.rs").await, "ALPHA\nBETA\n");
    }

    #[tokio::test]
    async fn a_path_scoped_convention_rides_along_on_the_first_touch() {
        let (_dir, ws) = workspace();
        write(&ws, "app/main.tsx", "export const App = () => null;\n").await;
        write(&ws, "lib.rs", "fn main() {}\n").await;
        let (read, _write, _edit, state) = gated_tools(&ws);
        state.set_rules(vec![PathRule::new(
            "app/AGENTS.md",
            "Frontend: no default exports.",
            &["app/**".to_string()],
        )]);

        let first = read
            .invoke(serde_json::json!({"path": "app/main.tsx"}))
            .await
            .unwrap();
        assert!(first.contains("no default exports"), "got: {first}");

        // Once only, and never on a path the rule doesn't govern.
        let again = read
            .invoke(serde_json::json!({"path": "app/main.tsx"}))
            .await
            .unwrap();
        assert!(!again.contains("no default exports"));
        let elsewhere = read
            .invoke(serde_json::json!({"path": "lib.rs"}))
            .await
            .unwrap();
        assert!(!elsewhere.contains("no default exports"));
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
