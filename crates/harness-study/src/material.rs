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
/// The most one file contributes when several share the budget.
const FILE_CHARS: usize = 6_000;
/// How many recently touched files a ride-along batch reads.
const RIDE_ALONG_FILES: usize = 8;

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
    /// Whether the excerpts are whole-file heads, so a line number in them is
    /// a line number in the file. A diff's are not: its hints must quote the
    /// material rather than be read back from disk by line.
    pub numbered: bool,
}

/// Excerpts from a territory's files. `seed` rotates which files are read
/// so consecutive batches on one territory don't all start at `lib.rs`.
pub fn for_territory(
    root: &Path,
    territory: &Territory,
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
    if files.is_empty() {
        return Err(StudyError::Nothing(format!(
            "the {} territory has no source files",
            territory.name
        )));
    }
    // Entry points first, then the rest rotated by the seed.
    let (mut entries, mut rest): (Vec<&String>, Vec<&String>) =
        files.iter().partition(|f| is_entry_point(f));
    if !rest.is_empty() {
        let start = (seed as usize) % rest.len();
        rest.rotate_left(start);
    }
    entries.extend(rest);
    let chosen: Vec<String> = entries.into_iter().cloned().collect();
    let (text, used) = excerpts(root, &chosen, MATERIAL_CHARS);
    if used.is_empty() {
        return Err(StudyError::Nothing(format!(
            "none of the {} territory's files could be read",
            territory.name
        )));
    }
    Ok(Material {
        territory_id: territory.id.clone(),
        territory_name: territory.name.clone(),
        framing: format!(
            "Source files from the `{}` region of the project ({} files in the region; \
             {} excerpted below).",
            territory.id,
            territory.files.len(),
            used.len()
        ),
        files: used,
        text,
        numbered: true,
    })
}

/// Excerpts from an explicit file list (ride-along), attributed to the
/// first file's territory.
pub fn for_files(
    root: &Path,
    files: &[String],
    framing: &str,
    territories: &[Territory],
) -> Result<Material, StudyError> {
    let (text, used) = excerpts(root, files, MATERIAL_CHARS);
    if used.is_empty() {
        return Err(StudyError::Nothing(
            "none of the recently touched files could be read".into(),
        ));
    }
    let home = used
        .iter()
        .find_map(|f| territory_for(territories, f))
        .or_else(|| territories.first());
    Ok(Material {
        territory_id: home.map(|t| t.id.clone()).unwrap_or_default(),
        territory_name: home.map(|t| t.name.clone()).unwrap_or_default(),
        framing: framing.to_string(),
        files: used,
        text,
        numbered: true,
    })
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

/// The files a session's agent read, wrote, or opened, most recent first,
/// from the session's stored messages (assistant `tool_calls`).
pub fn ride_along_paths(messages: &[serde_json::Value]) -> Vec<String> {
    const FILE_TOOLS: &[&str] = &["read_file", "edit_file", "write_file", "open_file"];
    let mut out: Vec<String> = Vec::new();
    for message in messages.iter().rev() {
        let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) else {
            continue;
        };
        for call in calls.iter().rev() {
            let name = call
                .pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if !FILE_TOOLS.contains(&name) {
                continue;
            }
            let args = call
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok());
            let Some(path) = args
                .as_ref()
                .and_then(|a| a.get("path"))
                .and_then(|p| p.as_str())
            else {
                continue;
            };
            let path = path.trim_start_matches("./").to_string();
            if !out.contains(&path) {
                out.push(path);
            }
            if out.len() >= RIDE_ALONG_FILES {
                return out;
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

/// Read `files` in order until `budget` characters are used. Returns the
/// labelled text and the files that made it in.
fn excerpts(root: &Path, files: &[String], budget: usize) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut used = Vec::new();
    let per_file = if files.len() > 1 {
        FILE_CHARS.min(budget / 2)
    } else {
        budget
    };
    for rel in files {
        if text.len() >= budget.saturating_sub(400) {
            break;
        }
        let Ok(body) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let room = per_file.min(budget - text.len());
        let cut = truncate(&body, room);
        let shown_lines = cut.lines().count();
        let total_lines = body.lines().count();
        text.push_str(&format!(
            "### {rel} (lines 1-{shown_lines} of {total_lines})\n{cut}\n\n"
        ));
        used.push(rel.clone());
    }
    (text, used)
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

    #[test]
    fn a_territory_reads_its_entry_point_first_and_rotates_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        for name in ["a.rs", "b.rs", "lib.rs", "lib_tests.rs"] {
            std::fs::write(root.join("src").join(name), format!("// {name}\n")).unwrap();
        }
        let t = Territory {
            id: "src".into(),
            name: "src".into(),
            files: vec![
                "src/a.rs".into(),
                "src/b.rs".into(),
                "src/lib.rs".into(),
                "src/lib_tests.rs".into(),
            ],
        };
        let m = for_territory(root, &t, 0).unwrap();
        assert_eq!(m.files, vec!["src/lib.rs", "src/a.rs", "src/b.rs"]);
        assert!(m.text.starts_with("### src/lib.rs (lines 1-1 of 1)"));
        let rotated = for_territory(root, &t, 1).unwrap();
        assert_eq!(rotated.files, vec!["src/lib.rs", "src/b.rs", "src/a.rs"]);
    }

    #[test]
    fn excerpts_respect_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("big.md"), "x\n".repeat(5_000)).unwrap();
        std::fs::write(root.join("small.md"), "hello\n").unwrap();
        // Several files share the budget: the big one is cut to half of it,
        // leaving room for the next.
        let (text, used) = excerpts(root, &["big.md".into(), "small.md".into()], 1_000);
        assert!(text.len() <= 1_100, "{}", text.len());
        assert_eq!(used, vec!["big.md", "small.md"]);
        assert!(text.contains("### big.md (lines 1-250 of 5000)"));
        // A budget one file fills leaves none for the rest.
        let (_, only) = excerpts(
            root,
            &["big.md".into(), "big.md".into(), "small.md".into()],
            1_000,
        );
        assert_eq!(only, vec!["big.md", "big.md"]);
    }

    #[test]
    fn ride_along_paths_come_from_file_tool_calls_newest_first() {
        let messages = vec![
            serde_json::json!({"role":"assistant","tool_calls":[
                {"function":{"name":"read_file","arguments":"{\"path\":\"./a.rs\"}"}},
                {"function":{"name":"run_shell","arguments":"{\"command\":\"ls\"}"}}]}),
            serde_json::json!({"role":"tool","content":"…"}),
            serde_json::json!({"role":"assistant","tool_calls":[
                {"function":{"name":"edit_file","arguments":"{\"path\":\"b.rs\"}"}},
                {"function":{"name":"read_file","arguments":"{\"path\":\"a.rs\"}"}}]}),
        ];
        assert_eq!(ride_along_paths(&messages), vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn fresh_tracks_is_none_outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert!(fresh_tracks(dir.path(), &[]).unwrap().is_none());
    }
}
