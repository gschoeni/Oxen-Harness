//! What a batch of questions is written from.
//!
//! The model never explores the workspace itself: the host gathers a bounded
//! slab of source text here — a territory's files, the recent diff, or the
//! files an agent just touched — and hands it over in one prompt. That keeps
//! a batch to a single completion (seconds, not a tool loop) and keeps every
//! question grounded in text the player can be pointed back to.

use std::path::Path;
use std::process::Command;

use crate::territories::{territory_for, Territory};
use crate::StudyError;

/// The most characters of source one prompt carries.
pub const MATERIAL_CHARS: usize = 14_000;
/// What the focus file may take; the rest is for the files that use it.
const FOCUS_CHARS: usize = 7_000;
/// Lines shown around a place another file uses the focus file's code.
const USE_CONTEXT_LINES: usize = 22;
/// How many "used in" excerpts follow the focus file.
const USE_SITES: usize = 2;
/// The most files searched for uses of the focus file's symbols.
const SEARCH_FILES: usize = 400;
/// How many recently read files a ride-along falls back to.
const RIDE_ALONG_FILES: usize = 8;
/// The most characters of one side of an agent edit shown.
const EDIT_SIDE_CHARS: usize = 1_400;

/// The gathered source text and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Material {
    pub territory_id: String,
    pub territory_name: String,
    /// One line telling the question writer what this slab is.
    pub framing: String,
    /// The files excerpted, relative to the workspace root.
    pub files: Vec<String>,
    /// The labelled excerpts.
    pub text: String,
    /// Whether every line carries its file line number, so a question can
    /// cite lines a hint is then read back from. A diff's lines are not file
    /// lines: its hints must quote the material instead.
    pub numbered: bool,
}

/// Source for a territory: a window of one focus file, then the places
/// other files use what it defines. Understanding a codebase is mostly
/// knowing how its pieces meet, and a single file's head can't show that.
/// `seed` rotates the focus file and where in it the window starts, so
/// consecutive batches don't keep rereading the top of `lib.rs`.
pub fn for_territory(
    root: &Path,
    territory: &Territory,
    all: &[Territory],
    seed: u64,
) -> Result<Material, StudyError> {
    let mut files: Vec<&String> = territory
        .files
        .iter()
        .filter(|f| !is_test_file(f))
        .collect();
    if files.is_empty() {
        files = territory.files.iter().collect();
    }
    // Entry points lead the rotation, so a region's first batch starts at
    // its front door.
    let (mut order, rest): (Vec<&String>, Vec<&String>) =
        files.iter().partition(|f| is_entry_point(f));
    order.extend(rest);
    let readable = |rel: &String| {
        std::fs::read_to_string(root.join(rel))
            .ok()
            .filter(|body| !body.trim().is_empty())
            .map(|body| (rel.clone(), body))
    };
    let start = (seed as usize).checked_rem(order.len()).unwrap_or(0);
    let Some((focus, body)) = order
        .iter()
        .cycle()
        .skip(start)
        .take(order.len())
        .find_map(|rel| readable(rel))
    else {
        return Err(StudyError::Nothing(format!(
            "none of the {} territory's files could be read",
            territory.name
        )));
    };

    let first_line = window_start(&body, FOCUS_CHARS, seed / 7);
    let mut text = numbered_excerpt(&focus, &body, first_line, FOCUS_CHARS);
    let mut used = vec![focus.clone()];
    let shown: String = body
        .lines()
        .skip(first_line - 1)
        .take(text.lines().count())
        .collect::<Vec<_>>()
        .join("\n");
    let symbols = defined_symbols(&shown);

    // Other regions first: a use across a crate boundary says more about
    // how the project fits together than one next door.
    let own: Vec<&String> = order.iter().copied().filter(|f| **f != focus).collect();
    let elsewhere = all
        .iter()
        .filter(|t| t.id != territory.id)
        .flat_map(|t| t.files.iter())
        .filter(|f| !is_test_file(f));
    for rel in elsewhere.chain(own.iter().copied()).take(SEARCH_FILES) {
        if used.len() > USE_SITES || text.len() >= MATERIAL_CHARS.saturating_sub(600) {
            break;
        }
        let Ok(other) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let Some((line, symbol)) = first_use(&other, &symbols) else {
            continue;
        };
        let from = line.saturating_sub(USE_CONTEXT_LINES / 2).max(1);
        let room = (MATERIAL_CHARS - text.len()).min(3_200);
        text.push_str(&format!("(uses `{symbol}` from {focus})\n"));
        text.push_str(&numbered_excerpt_lines(
            rel,
            &other,
            from,
            USE_CONTEXT_LINES,
            room,
        ));
        used.push(rel.clone());
    }
    // Nothing uses it (or nothing was found): a second file from the region
    // still gives the questions more than one place to stand.
    if used.len() == 1 {
        if let Some((rel, other)) = own.iter().find_map(|rel| readable(rel)) {
            let room = MATERIAL_CHARS - text.len();
            text.push_str(&numbered_excerpt(&rel, &other, 1, room.min(FOCUS_CHARS)));
            used.push(rel);
        }
    }

    Ok(Material {
        territory_id: territory.id.clone(),
        territory_name: territory.name.clone(),
        framing: format!(
            "Source from the `{}` region of the project. The first excerpt is the focus; any \
             excerpt marked \"uses … from\" is another file relying on something the focus \
             defines. The number before each `|` is that line's number in its file.",
            territory.id
        ),
        files: used,
        text,
        numbered: true,
    })
}

/// What a ride-along studies: the edits the session's agent made, or — when
/// it has only read so far — the files it read.
pub fn ride_along(
    root: &Path,
    messages: &[serde_json::Value],
    territories: &[Territory],
) -> Result<Material, StudyError> {
    let edits = agent_edits(messages);
    if !edits.is_empty() {
        return Ok(for_edits(&edits, territories));
    }
    let paths = ride_along_paths(messages);
    if paths.is_empty() {
        return Err(StudyError::Nothing(
            "the agent hasn't read or edited any files in this chat yet".into(),
        ));
    }
    let mut text = String::new();
    let mut used = Vec::new();
    for rel in &paths {
        if text.len() >= MATERIAL_CHARS.saturating_sub(600) {
            break;
        }
        let Ok(body) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let room = (MATERIAL_CHARS - text.len()).min(FOCUS_CHARS);
        text.push_str(&numbered_excerpt(rel, &body, 1, room));
        used.push(rel.clone());
    }
    if used.is_empty() {
        return Err(StudyError::Nothing(
            "none of the files the agent read could be opened".into(),
        ));
    }
    let home = used
        .iter()
        .find_map(|f| territory_for(territories, f))
        .or_else(|| territories.first());
    Ok(Material {
        territory_id: home.map(|t| t.id.clone()).unwrap_or_default(),
        territory_name: home.map(|t| t.name.clone()).unwrap_or_default(),
        framing: "Files the coding agent read most recently in this chat. The number before \
                  each `|` is that line's number in its file."
            .into(),
        files: used,
        text,
        numbered: true,
    })
}

/// One change the agent made to a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEdit {
    pub path: String,
    /// What was there; empty for a new file or an insertion.
    pub before: String,
    pub after: String,
}

/// The edits a session's agent made, newest first, from the `edit_file` and
/// `write_file` calls in its stored messages.
pub fn agent_edits(messages: &[serde_json::Value]) -> Vec<AgentEdit> {
    let mut out = Vec::new();
    for (name, args) in tool_calls_newest_first(messages) {
        let Some(path) = args.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        let path = path.trim_start_matches("./").to_string();
        let text = |v: &serde_json::Value, key: &str| {
            v.get(key)
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string()
        };
        match name.as_str() {
            "edit_file" => {
                let pairs: Vec<&serde_json::Value> =
                    match args.get("edits").and_then(|e| e.as_array()) {
                        Some(list) if !list.is_empty() => list.iter().collect(),
                        _ => vec![&args],
                    };
                for pair in pairs {
                    let after = text(pair, "new_string");
                    let before = text(pair, "old_string");
                    if !after.trim().is_empty() || !before.trim().is_empty() {
                        out.push(AgentEdit {
                            path: path.clone(),
                            before,
                            after,
                        });
                    }
                }
            }
            "write_file" => {
                let after = text(&args, "content");
                if !after.trim().is_empty() {
                    out.push(AgentEdit {
                        path,
                        before: String::new(),
                        after,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

fn for_edits(edits: &[AgentEdit], territories: &[Territory]) -> Material {
    let mut text = String::new();
    let mut files: Vec<String> = Vec::new();
    for edit in edits {
        if text.len() >= MATERIAL_CHARS.saturating_sub(600) {
            break;
        }
        text.push_str(&format!("### {}\n", edit.path));
        if edit.before.trim().is_empty() {
            text.push_str("--- (new code) ---\n");
        } else {
            text.push_str("--- before ---\n");
            text.push_str(&truncate(&edit.before, EDIT_SIDE_CHARS));
            text.push_str("\n--- after ---\n");
        }
        text.push_str(&truncate(&edit.after, EDIT_SIDE_CHARS));
        text.push_str("\n\n");
        if !files.contains(&edit.path) {
            files.push(edit.path.clone());
        }
    }
    let home = files
        .iter()
        .find_map(|f| territory_for(territories, f))
        .or_else(|| territories.first());
    Material {
        territory_id: home.map(|t| t.id.clone()).unwrap_or_default(),
        territory_name: home.map(|t| t.name.clone()).unwrap_or_default(),
        framing: "Edits the coding agent made in this chat, newest first: what each file \
                  held before and what the agent wrote. Ask what the change does, why it was \
                  needed, and what would break without it — the developer should be able to \
                  explain the agent's work before accepting it."
            .into(),
        files,
        text: truncate(&text, MATERIAL_CHARS),
        numbered: false,
    }
}

/// The recent git changes: the uncommitted diff against HEAD when there is
/// one, else the last three commits with their patches. `None` when the
/// workspace isn't a git repository or has no history and no changes.
pub fn fresh_tracks(
    root: &Path,
    territories: &[Territory],
) -> Result<Option<Material>, StudyError> {
    // The probe failing means "not a repository" (or one with no commit
    // yet): nothing to study, not an error. Once it has succeeded, a later
    // command failing is a real failure and is reported as one.
    let Some(diff) = git(root, &["diff", "HEAD"])? else {
        return Ok(None);
    };
    let (text, files, framing) = if !diff.trim().is_empty() {
        let names = git_ok(root, &["diff", "HEAD", "--name-only"])?;
        (
            diff,
            lines(&names),
            "The uncommitted changes in the working tree (a unified diff against HEAD).",
        )
    } else {
        let log = git_ok(
            root,
            &["log", "-n", "3", "-p", "--format=commit %h — %s (%an)"],
        )?;
        if log.trim().is_empty() {
            return Ok(None);
        }
        let names = git_ok(root, &["log", "-n", "3", "--name-only", "--format="])?;
        (
            log,
            lines(&names),
            "The last three commits, with their patches (newest first).",
        )
    };
    let home = files
        .iter()
        .find_map(|f| territory_for(territories, f))
        .or_else(|| territories.first());
    Ok(Some(Material {
        territory_id: home.map(|t| t.id.clone()).unwrap_or_default(),
        territory_name: home.map(|t| t.name.clone()).unwrap_or_default(),
        framing: framing.to_string(),
        files,
        text: truncate(&text, MATERIAL_CHARS),
        numbered: false,
    }))
}

/// The files a session's agent read or opened, most recent first.
pub fn ride_along_paths(messages: &[serde_json::Value]) -> Vec<String> {
    const FILE_TOOLS: &[&str] = &["read_file", "edit_file", "write_file", "open_file"];
    let mut out: Vec<String> = Vec::new();
    for (name, args) in tool_calls_newest_first(messages) {
        if !FILE_TOOLS.contains(&name.as_str()) {
            continue;
        }
        let Some(path) = args.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        let path = path.trim_start_matches("./").to_string();
        if !out.contains(&path) {
            out.push(path);
        }
        if out.len() >= RIDE_ALONG_FILES {
            break;
        }
    }
    out
}

/// Every tool call in a transcript as (name, parsed arguments), newest
/// first. Calls whose arguments aren't a JSON object are skipped.
fn tool_calls_newest_first(messages: &[serde_json::Value]) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    for message in messages.iter().rev() {
        let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) else {
            continue;
        };
        for call in calls.iter().rev() {
            let name = call
                .pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            let args = call
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok());
            if let Some(args) = args.filter(|a| a.is_object()) {
                out.push((name.to_string(), args));
            }
        }
    }
    out
}

fn is_test_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.contains("/__tests__/")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.ends_with("_test.go")
        || lower.ends_with("_tests.rs")
}

fn is_entry_point(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    matches!(
        name.as_str(),
        "lib.rs"
            | "mod.rs"
            | "main.rs"
            | "index.ts"
            | "index.tsx"
            | "index.js"
            | "readme.md"
            | "__init__.py"
    )
}

/// `### path (lines a-b of n)` then each line prefixed with its file line
/// number, starting at `first_line` and stopping at `budget` characters.
fn numbered_excerpt(rel: &str, body: &str, first_line: usize, budget: usize) -> String {
    numbered_excerpt_lines(rel, body, first_line, usize::MAX, budget)
}

fn numbered_excerpt_lines(
    rel: &str,
    body: &str,
    first_line: usize,
    max_lines: usize,
    budget: usize,
) -> String {
    let total = body.lines().count();
    let mut lines = String::new();
    let mut last = first_line.saturating_sub(1);
    for (i, line) in body
        .lines()
        .enumerate()
        .skip(first_line.saturating_sub(1))
        .take(max_lines)
    {
        let row = format!("{:>4}| {}\n", i + 1, line);
        if lines.len() + row.len() > budget && !lines.is_empty() {
            break;
        }
        lines.push_str(&row);
        last = i + 1;
    }
    format!("### {rel} (lines {first_line}-{last} of {total})\n{lines}\n")
}

/// The 1-based line a focus window starts on. A file that fits is read from
/// the top; a longer one starts at one of its top-level items, chosen by
/// `seed`, so the middle and end of big files get asked about too.
fn window_start(body: &str, budget: usize, seed: u64) -> usize {
    if body.len() <= budget {
        return 1;
    }
    let starts: Vec<usize> = body
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            line.chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '#' || c == '/')
        })
        .map(|(i, _)| i + 1)
        .collect();
    (seed as usize)
        .checked_rem(starts.len())
        .and_then(|i| starts.get(i).copied())
        .unwrap_or(1)
}

/// Names the excerpt defines: functions, types, constants. Short names are
/// dropped — `new` or `run` would "match" half the codebase.
fn defined_symbols(excerpt: &str) -> Vec<String> {
    const KEYWORDS: &[&str] = &[
        "fn",
        "struct",
        "enum",
        "trait",
        "type",
        "const",
        "static",
        "function",
        "class",
        "interface",
        "def",
        "func",
    ];
    let mut out: Vec<String> = Vec::new();
    for line in excerpt.lines() {
        let mut words = line
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty());
        while let Some(word) = words.next() {
            if !KEYWORDS.contains(&word) {
                continue;
            }
            if let Some(name) = words.next() {
                if name.len() >= 6 && !out.iter().any(|n| n == name) {
                    out.push(name.to_string());
                }
            }
            break;
        }
    }
    out
}

/// The first line of `body` that mentions one of `symbols` as a whole word.
fn first_use(body: &str, symbols: &[String]) -> Option<(usize, String)> {
    for (i, line) in body.lines().enumerate() {
        for symbol in symbols {
            let mut from = 0;
            while let Some(at) = line[from..].find(symbol.as_str()) {
                let start = from + at;
                let end = start + symbol.len();
                let word = |c: char| c.is_alphanumeric() || c == '_';
                let before = line[..start].chars().next_back().is_some_and(word);
                let after = line[end..].chars().next().is_some_and(word);
                if !before && !after {
                    return Some((i + 1, symbol.clone()));
                }
                from = end;
            }
        }
    }
    None
}

/// Cut at a line boundary under `max` characters.
fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    match head.rfind('\n') {
        Some(at) if at > max / 2 => head[..at].to_string(),
        _ => head.to_string(),
    }
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Run git in `root`. `Ok(None)` is git itself saying no (a non-zero exit);
/// `Err` is git not running at all.
fn git(root: &Path, args: &[&str]) -> Result<Option<String>, StudyError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| StudyError::Git {
            args: args.join(" "),
            detail: format!("could not run git: {e}"),
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// [`git`] for a command that must succeed in a working repository.
fn git_ok(root: &Path, args: &[&str]) -> Result<String, StudyError> {
    git(root, args)?.ok_or_else(|| StudyError::Git {
        args: args.join(" "),
        detail: "the command failed".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn territory(id: &str, files: &[&str]) -> Territory {
        Territory {
            id: id.into(),
            name: id.into(),
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn a_territory_shows_its_focus_file_and_who_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "core/lib.rs",
            "pub fn make_room() {}\npub fn stream_reply() {}\n",
        );
        write(root, "core/lib_tests.rs", "fn t() { make_room(); }\n");
        write(
            root,
            "host/turn.rs",
            "fn unrelated() {}\nfn drive() {\n    core::make_room();\n}\n",
        );
        let core = territory("core", &["core/lib.rs", "core/lib_tests.rs"]);
        let all = vec![core.clone(), territory("host", &["host/turn.rs"])];
        let m = for_territory(root, &core, &all, 0).unwrap();
        assert_eq!(m.files, vec!["core/lib.rs", "host/turn.rs"]);
        assert!(m
            .text
            .starts_with("### core/lib.rs (lines 1-2 of 2)\n   1| pub fn make_room() {}"));
        assert!(m.text.contains("(uses `make_room` from core/lib.rs)"));
        assert!(m.text.contains("   3|     core::make_room();"));
        assert!(m.numbered);
    }

    #[test]
    fn a_long_focus_file_is_read_from_different_items() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let body: String = (0..400)
            .map(|i| format!("fn item_number_{i}() {{\n    body();\n}}\n"))
            .collect();
        write(root, "src/big.rs", &body);
        let t = territory("src", &["src/big.rs"]);
        let top = for_territory(root, &t, std::slice::from_ref(&t), 0).unwrap();
        assert!(top.text.contains("   1| fn item_number_0()"));
        // seed / 7 picks the item: 70 → the tenth, which starts on line 31.
        let later = for_territory(root, &t, std::slice::from_ref(&t), 70).unwrap();
        assert!(later.text.starts_with("### src/big.rs (lines 31-"));
        assert!(later.text.contains("  31| fn item_number_10()"));
        assert!(later.text.len() <= MATERIAL_CHARS);
    }

    #[test]
    fn symbols_are_whole_words_and_skip_short_names() {
        let symbols = defined_symbols(
            "pub fn new() {}\npub struct Workspace;\nexport function loadTable() {}",
        );
        assert_eq!(symbols, vec!["Workspace", "loadTable"]);
        assert_eq!(
            first_use("let w = MyWorkspace::x();\nWorkspace::new()", &symbols),
            Some((2, "Workspace".into()))
        );
        assert_eq!(first_use("nothing here", &symbols), None);
    }

    #[test]
    fn a_ride_along_studies_what_the_agent_wrote() {
        let messages = vec![
            serde_json::json!({"role":"assistant","tool_calls":[
                {"function":{"name":"read_file","arguments":"{\"path\":\"./a.rs\"}"}},
                {"function":{"name":"edit_file","arguments":"{\"path\":\"a.rs\",\"old_string\":\"let a = 1;\",\"new_string\":\"let a = 9;\"}"}}]}),
            serde_json::json!({"role":"assistant","tool_calls":[
                {"function":{"name":"edit_file","arguments":"{\"path\":\"b.rs\",\"edits\":[{\"old_string\":\"x\",\"new_string\":\"y\"}]}"}},
                {"function":{"name":"write_file","arguments":"{\"path\":\"c.rs\",\"content\":\"fn c() {}\"}"}}]}),
        ];
        let edits = agent_edits(&messages);
        assert_eq!(
            edits.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["c.rs", "b.rs", "a.rs"]
        );
        let dir = tempfile::tempdir().unwrap();
        let m = ride_along(dir.path(), &messages, &[territory("docs", &[])]).unwrap();
        assert!(!m.numbered);
        assert!(m.framing.contains("Edits the coding agent made"));
        assert!(m
            .text
            .contains("### a.rs\n--- before ---\nlet a = 1;\n--- after ---\nlet a = 9;"));
        assert!(m.text.contains("### c.rs\n--- (new code) ---\nfn c() {}"));
    }

    #[test]
    fn a_ride_along_falls_back_to_files_the_agent_read() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.rs", "fn a() {}\n");
        let messages = vec![serde_json::json!({"role":"assistant","tool_calls":[
            {"function":{"name":"read_file","arguments":"{\"path\":\"./a.rs\"}"}},
            {"function":{"name":"run_shell","arguments":"{\"command\":\"ls\"}"}}]})];
        assert_eq!(ride_along_paths(&messages), vec!["a.rs"]);
        let m = ride_along(dir.path(), &messages, &[]).unwrap();
        assert!(m.numbered);
        assert!(m.text.contains("   1| fn a() {}"));
        assert!(matches!(
            ride_along(dir.path(), &[], &[]),
            Err(StudyError::Nothing(_))
        ));
    }

    #[test]
    fn fresh_tracks_is_none_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert!(fresh_tracks(dir.path(), &[]).unwrap().is_none());
    }
}
