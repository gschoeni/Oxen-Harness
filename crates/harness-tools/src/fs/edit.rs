//! `edit_file` — exact-text replacement, one hunk or many.
//!
//! Every hunk is matched against the *original* text and applied back to
//! front, so a batch is atomic and order-independent: a swap works, and one
//! bad hunk leaves the file untouched rather than half-edited. Line endings
//! and a leading BOM survive the round trip, and an edit that lands on a
//! region the model never saw — or on a file that moved since it was read —
//! is refused rather than written.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::sandbox::Workspace;
use crate::{ToolError, TypedTool};

use super::relative_to;
use super::state::{self, FileState};
use super::EDIT_FILE_TOOL;
use super::{edit_diagnostics, syntax};

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

#[cfg(test)]
mod tests {

    use super::*;
    use crate::fs::testkit::*;
    use crate::{ToolError, TypedTool};

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
}
