//! Custom slash commands: prompt templates in Markdown files.
//!
//! A file `<name>.md` becomes `/<name>`. Typing `/<name> some args` sends the
//! file's body to the model with `$ARGUMENTS` (or `$1`, `$2`, …) filled in —
//! the cheapest possible way to make a repeated prompt one keystroke away,
//! and the same shape Claude Code uses, so a team's existing commands work
//! unchanged.
//!
//! Locations, in precedence order (first wins on a name clash):
//!
//! 1. `<workspace>/.oxen-harness/commands/*.md` — the project's own.
//! 2. `~/.oxen-harness/commands/*.md` — the user's.
//! 3. `<workspace>/.claude/commands/**/*.md` and `~/.claude/commands/**/*.md`
//!    — read as-is for compatibility; a nested `foo/bar.md` is `/foo:bar`.
//!
//! A command's description is its `description:` front-matter field, else its
//! first non-empty line. Discovery runs when a session starts; there is no
//! watcher.

use std::path::{Path, PathBuf};

use harness_config::paths;
use serde::Serialize;

/// The most of a body's first line used as a description.
const DESCRIPTION_CHARS: usize = 60;
/// How deep the compatibility directories are walked.
const MAX_DEPTH: usize = 3;

/// Where a command came from, for display and precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    Project,
    Global,
    /// Read from a `.claude/commands` tree.
    Claude,
}

/// One discovered command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CustomCommand {
    /// The word after the slash (`review-pr`, or `frontend:component` for a
    /// nested compatibility file).
    pub name: String,
    pub description: String,
    /// The template, front matter removed.
    pub body: String,
    pub source: CommandSource,
    pub path: PathBuf,
}

impl CustomCommand {
    /// The prompt to send for `/<name> <args>`: `$ARGUMENTS`/`$@` become the
    /// whole argument string, `$1`…`$9` the whitespace-split words. When the
    /// template uses no placeholder and arguments were given, they are
    /// appended on their own line so nothing the user typed is lost.
    pub fn expand(&self, args: &str) -> String {
        let args = args.trim();
        let words: Vec<&str> = args.split_whitespace().collect();
        let mut out = String::with_capacity(self.body.len() + args.len());
        let mut used_placeholder = false;
        let mut rest = self.body.as_str();
        while let Some(i) = rest.find('$') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            if let Some(tail) = after.strip_prefix("ARGUMENTS") {
                out.push_str(args);
                used_placeholder = true;
                rest = tail;
            } else if let Some(tail) = after.strip_prefix('@') {
                out.push_str(args);
                used_placeholder = true;
                rest = tail;
            } else if let Some(digit) = after.chars().next().filter(|c| c.is_ascii_digit()) {
                let n = digit.to_digit(10).unwrap_or(0) as usize;
                if n >= 1 {
                    out.push_str(words.get(n - 1).copied().unwrap_or(""));
                    used_placeholder = true;
                }
                rest = &after[1..];
            } else {
                out.push('$');
                rest = after;
            }
        }
        out.push_str(rest);
        if !used_placeholder && !args.is_empty() {
            let out = out.trim_end();
            return format!("{out}\n\n{args}");
        }
        out
    }
}

/// Discover every command visible from `workspace_root`, sorted by name.
pub fn discover(workspace_root: &Path) -> Vec<CustomCommand> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut found: Vec<CustomCommand> = Vec::new();
    let mut add = |mut batch: Vec<CustomCommand>| {
        batch.retain(|c| !found.iter().any(|f| f.name == c.name));
        found.extend(batch);
    };
    add(load_dir(
        &project_commands_dir(workspace_root),
        CommandSource::Project,
        false,
    ));
    if let Ok(dir) = paths::commands_dir() {
        add(load_dir(&dir, CommandSource::Global, false));
    }
    add(load_dir(
        &workspace_root.join(".claude").join("commands"),
        CommandSource::Claude,
        true,
    ));
    if let Some(home) = home {
        add(load_dir(
            &home.join(".claude").join("commands"),
            CommandSource::Claude,
            true,
        ));
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// The project-scoped commands directory for a workspace.
pub fn project_commands_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".oxen-harness").join("commands")
}

/// Load every `*.md` under `dir` (nested when `recursive`, with directory
/// names joined by `:`). A missing directory is simply no commands.
pub fn load_dir(dir: &Path, source: CommandSource, recursive: bool) -> Vec<CustomCommand> {
    let mut out = Vec::new();
    walk(dir, "", source, recursive, 0, &mut out);
    out
}

fn walk(
    dir: &Path,
    prefix: &str,
    source: CommandSource,
    recursive: bool,
    depth: usize,
    out: &mut Vec<CustomCommand>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.is_dir() {
            if recursive && depth < MAX_DEPTH && !stem.starts_with('.') {
                walk(
                    &path,
                    &format!("{prefix}{stem}:"),
                    source,
                    recursive,
                    depth + 1,
                    out,
                );
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") || !is_valid_name(stem) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (description, body) = parse(&text);
        if body.trim().is_empty() {
            continue;
        }
        out.push(CustomCommand {
            name: format!("{prefix}{stem}"),
            description,
            body,
            source,
            path,
        });
    }
}

/// A command name is a slug: letters, digits, `-`, `_`, and (for nested
/// compatibility files) `:`.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
}

/// Split optional `---` front matter off a template, returning the
/// description (front matter `description:`, else the first non-empty body
/// line, clipped) and the body.
fn parse(text: &str) -> (String, String) {
    let text = text.trim_start_matches('\u{feff}');
    let mut description = None;
    let body = match text.strip_prefix("---\n") {
        Some(rest) => match rest.find("\n---") {
            Some(end) => {
                let front = &rest[..end];
                for line in front.lines() {
                    if let Some(value) = line.strip_prefix("description:") {
                        let value = value.trim().trim_matches(['"', '\'']);
                        if !value.is_empty() {
                            description = Some(value.to_string());
                        }
                    }
                }
                let after = &rest[end + 4..];
                after.strip_prefix('\n').unwrap_or(after)
            }
            None => text,
        },
        None => text,
    };
    let body = body.trim().to_string();
    let description = description.unwrap_or_else(|| {
        let first = body
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .trim_start_matches('#')
            .trim();
        harness_core::text::ellipsize(first, DESCRIPTION_CHARS)
    });
    (description, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(body: &str) -> CustomCommand {
        CustomCommand {
            name: "t".into(),
            description: String::new(),
            body: body.into(),
            source: CommandSource::Project,
            path: PathBuf::new(),
        }
    }

    #[test]
    fn arguments_placeholder_takes_the_whole_string() {
        assert_eq!(
            cmd("Review $ARGUMENTS carefully.").expand("PR 42 please"),
            "Review PR 42 please carefully."
        );
        assert_eq!(cmd("Fix $@").expand("the build"), "Fix the build");
    }

    #[test]
    fn positional_placeholders_take_words_and_missing_ones_are_empty() {
        assert_eq!(
            cmd("Rename $1 to $2 ($3)").expand("foo bar"),
            "Rename foo to bar ()"
        );
    }

    #[test]
    fn unused_arguments_are_appended_and_a_bare_dollar_survives() {
        assert_eq!(
            cmd("Costs $USD.\n").expand("context here"),
            "Costs $USD.\n\ncontext here"
        );
        assert_eq!(cmd("No args here").expand(""), "No args here");
    }

    #[test]
    fn front_matter_description_wins_over_the_first_line() {
        let (d, body) =
            parse("---\ndescription: \"Review a PR\"\nother: x\n---\n# Review\n\nDo it.");
        assert_eq!(d, "Review a PR");
        assert_eq!(body, "# Review\n\nDo it.");
        let (d, _) = parse("# Ship it\n\nbody");
        assert_eq!(d, "Ship it");
    }

    #[test]
    fn discovery_layers_project_over_global_over_claude() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let p = project_commands_dir(&project);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("ship.md"), "project ship").unwrap();
        std::fs::write(p.join("bad name.md"), "ignored").unwrap();
        let claude = project.join(".claude").join("commands").join("fe");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("component.md"), "make a component").unwrap();
        std::fs::write(
            project.join(".claude").join("commands").join("ship.md"),
            "claude ship",
        )
        .unwrap();

        let found = discover(&project);
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"ship"), "{names:?}");
        assert!(names.contains(&"fe:component"), "{names:?}");
        assert!(!names.iter().any(|n| n.contains(' ')));
        let ship = found.iter().find(|c| c.name == "ship").unwrap();
        assert_eq!(ship.source, CommandSource::Project);
        assert_eq!(ship.body, "project ship");
    }
}
