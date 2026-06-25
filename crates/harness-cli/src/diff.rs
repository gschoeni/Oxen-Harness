//! Rendering file writes/edits as a colored diff block.
//!
//! When the agent calls `write_file` or `edit_file`, a one-line argument preview
//! hides what's actually changing. Instead we parse the tool arguments and render
//! a Claude-Code-style diff: new files as line-numbered green additions, edits as
//! a removed/added (`-`/`+`) hunk. Returns plain `Vec<String>` lines so both the
//! classic and live renderers can print them their own way.

use crate::theme::Ui;

/// At most this many diff lines per side, so a huge write doesn't flood the
/// scrollback; the remainder is summarized with a "… +N more" line.
const MAX_LINES: usize = 24;
/// Long lines are clipped to keep the block from wrapping messily.
const MAX_COLS: usize = 120;

/// If `name`/`args` is a `write_file` or `edit_file` call, render its diff block;
/// otherwise `None` (the caller falls back to the generic tool line).
pub fn render_file_change(ui: &Ui, name: &str, args: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(args).ok()?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    match name {
        "write_file" => {
            let path = v.get("path")?.as_str()?;
            Some(render_write(ui, path, get("contents")))
        }
        "edit_file" => {
            let path = v.get("path")?.as_str()?;
            Some(render_edit(ui, path, get("old_string"), get("new_string")))
        }
        _ => None,
    }
}

/// A new/overwritten file: every line is an addition.
fn render_write(ui: &Ui, path: &str, contents: &str) -> Vec<String> {
    let total = contents.lines().count();
    let mut lines = vec![header(
        ui,
        "Write",
        path,
        &format!("+{total} {}", noun(total)),
    )];
    for (i, line) in contents.lines().take(MAX_LINES).enumerate() {
        lines.push(added(ui, Some(i + 1), line));
    }
    if total > MAX_LINES {
        lines.push(more(ui, total - MAX_LINES));
    }
    lines
}

/// An exact-string edit: the removed lines then the added lines.
fn render_edit(ui: &Ui, path: &str, old: &str, new: &str) -> Vec<String> {
    let (removed, added_n) = (old.lines().count(), new.lines().count());
    let mut lines = vec![header(ui, "Edit", path, &format!("-{removed} +{added_n}"))];
    for line in old.lines().take(MAX_LINES) {
        lines.push(removed_line(ui, line));
    }
    if removed > MAX_LINES {
        lines.push(more(ui, removed - MAX_LINES));
    }
    for line in new.lines().take(MAX_LINES) {
        lines.push(added(ui, None, line));
    }
    if added_n > MAX_LINES {
        lines.push(more(ui, added_n - MAX_LINES));
    }
    lines
}

/// The `✎ <verb> <path>  <summary>` title line.
fn header(ui: &Ui, verb: &str, path: &str, summary: &str) -> String {
    format!(
        "  {} {} {}  {}",
        ui.green("✎"),
        ui.accent(verb),
        ui.strong(path),
        ui.dim(summary),
    )
}

/// An added line: optional line number, a green `+`, and green content.
fn added(ui: &Ui, lineno: Option<usize>, text: &str) -> String {
    let gutter = match lineno {
        Some(n) => ui.dim(&format!("{n:>4} ")),
        None => "     ".to_string(),
    };
    format!("  {gutter}{} {}", ui.green("+"), ui.green(&clip(text)))
}

/// A removed line: a red `-` and red content.
fn removed_line(ui: &Ui, text: &str) -> String {
    format!("       {} {}", ui.red("-"), ui.red(&clip(text)))
}

fn more(ui: &Ui, n: usize) -> String {
    format!("  {}", ui.dim(&format!("… +{n} more {}", noun(n))))
}

fn noun(n: usize) -> &'static str {
    if n == 1 {
        "line"
    } else {
        "lines"
    }
}

/// Clip a single line to [`MAX_COLS`] characters (diff lines shouldn't wrap).
fn clip(s: &str) -> String {
    if s.chars().count() <= MAX_COLS {
        s.to_string()
    } else {
        let kept: String = s.chars().take(MAX_COLS - 1).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_file_renders_numbered_additions() {
        let ui = Ui::plain();
        let args = r#"{"path":"src/a.rs","contents":"fn a() {}\nfn b() {}"}"#;
        let lines = render_file_change(&ui, "write_file", args).unwrap();
        let joined = lines.join("\n");
        assert!(joined.contains("Write"));
        assert!(joined.contains("src/a.rs"));
        assert!(joined.contains("+2 lines"));
        assert!(joined.contains("1 ") && joined.contains("fn a() {}"));
        assert!(joined.contains("+ fn b() {}"));
    }

    #[test]
    fn edit_file_renders_removed_then_added() {
        let ui = Ui::plain();
        let args = r#"{"path":"f.rs","old_string":"let x = 1;","new_string":"let x = 2;"}"#;
        let lines = render_file_change(&ui, "edit_file", args).unwrap();
        let joined = lines.join("\n");
        assert!(joined.contains("Edit"));
        assert!(joined.contains("-1 +1"));
        assert!(joined.contains("- let x = 1;"));
        assert!(joined.contains("+ let x = 2;"));
    }

    #[test]
    fn long_writes_are_truncated_with_a_summary() {
        let ui = Ui::plain();
        let body: String = (0..40).map(|i| format!("line {i}\\n")).collect();
        let args = format!(r#"{{"path":"big.txt","contents":"{body}"}}"#);
        let lines = render_file_change(&ui, "write_file", &args).unwrap();
        assert!(lines.join("\n").contains("more lines"));
        // header + MAX_LINES + summary
        assert_eq!(lines.len(), 1 + MAX_LINES + 1);
    }

    #[test]
    fn other_tools_are_not_diffed() {
        let ui = Ui::plain();
        assert!(render_file_change(&ui, "run_command", r#"{"command":"ls"}"#).is_none());
        assert!(render_file_change(&ui, "read_file", r#"{"path":"a"}"#).is_none());
    }

    #[test]
    fn malformed_arguments_fall_back() {
        let ui = Ui::plain();
        assert!(render_file_change(&ui, "write_file", "not json").is_none());
        assert!(render_file_change(&ui, "write_file", r#"{"no":"path"}"#).is_none());
    }
}
