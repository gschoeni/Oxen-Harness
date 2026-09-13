//! Redirect shell commands that have a first-class tool.
//!
//! `grep`, `find`, and `cat` are the three commands a model reaches for out of
//! habit, and all three are worse through the shell than through the tool that
//! replaces them: no gitignore awareness, output that gets truncated by the
//! stream cap, paths the model then has to re-derive, and — for `cat` — a read
//! that `edit_file` doesn't know happened, so the next edit trips the
//! freshness check.
//!
//! So the shell tool classifies the command first and, for those shapes,
//! returns a short redirect *as a successful result* instead of running it.
//! An error would read as a malfunction and invite a retry; a result reads as
//! an instruction, and the model reissues the call against the right tool.
//!
//! The rule is deliberately narrow — only a bare, single command with no shell
//! plumbing. A pipe, a redirect, a `&&`, or a leading space (the documented
//! escape hatch, and the same convention shells use for "don't record this")
//! all mean the model wants the shell specifically, and it gets it.

use crate::fs::{FIND_FILES_TOOL, READ_FILE_TOOL, SEARCH_FILES_TOOL};

/// How every redirect ends: the model needs to know the door isn't locked.
const ESCAPE: &str = "If you really need the shell form, prefix the command with a space.";

/// Characters that turn a command line into shell plumbing. Any of them and
/// the model is composing, not just reading — hands off.
const PLUMBING: &[char] = &['|', '&', ';', '>', '<', '`', '\n', '\r', '(', ')'];

/// If `command` is one of the bare shapes a dedicated tool does better,
/// return the message to hand back in place of running it.
///
/// `available` says whether the tool a redirect would point at is actually
/// registered right now: a user can switch `search_files` off after the
/// registry was built, and a redirect to a tool the model doesn't have is a
/// dead end (blocked `grep` on one side, unknown tool on the other).
///
/// Pure and total: no I/O, and anything it doesn't recognize returns `None`
/// so the shell runs it unchanged.
pub fn intercept(command: &str, available: impl Fn(&str) -> bool) -> Option<String> {
    // Checked before trimming — the leading space *is* the bypass.
    if command.starts_with([' ', '\t']) {
        return None;
    }
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.contains(PLUMBING) || trimmed.contains("$(") {
        return None;
    }

    let mut tokens = trimmed.split_whitespace();
    let program = tokens.next()?;
    let args: Vec<&str> = tokens.collect();

    let (target, redirect) = match program {
        "grep" | "egrep" | "rg" => (
            SEARCH_FILES_TOOL,
            "use search_files for content search (in-process, gitignore-aware, \
             returns matches you can act on)",
        ),
        // `-exec`/`-delete` make it an action, not a search; `find_files` only
        // lists paths, so those keep going to the shell.
        "find" if !args.iter().any(|a| ACTION_PREDICATES.contains(a)) => (
            FIND_FILES_TOOL,
            "use find_files for file discovery (glob-based, gitignore-aware, \
             returns paths you can act on)",
        ),
        // Only the plain "show me this file" form: any flag means the model
        // wants something `read_file` may not do (`tail -f`, `head -c`).
        "cat" | "head" | "tail" if is_single_file(&args) => (
            READ_FILE_TOOL,
            "use read_file to read a file (numbered lines, and the read is \
             recorded so a later edit_file isn't rejected as stale)",
        ),
        _ => return None,
    };
    if !available(target) {
        return None;
    }
    Some(format!("Blocked: {redirect}. {ESCAPE}"))
}

/// `find` predicates that run something rather than report it.
const ACTION_PREDICATES: &[&str] = &["-exec", "-execdir", "-ok", "-okdir", "-delete"];

/// Exactly one argument, and it isn't a flag.
fn is_single_file(args: &[&str]) -> bool {
    matches!(args, [only] if !only.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_search_goes_to_search_files() {
        for command in ["grep -rn needle .", "rg needle", "egrep needle src"] {
            let out = intercept(command, |_| true)
                .unwrap_or_else(|| panic!("{command} should be blocked"));
            assert!(out.starts_with("Blocked: use search_files"), "{out}");
            assert!(out.contains("prefix the command with a space"), "{out}");
        }
    }

    #[test]
    fn file_discovery_goes_to_find_files() {
        let out = intercept("find . -name '*.rs'", |_| true).expect("blocked");
        assert!(out.contains("find_files"), "{out}");
    }

    #[test]
    fn a_find_that_acts_is_left_alone() {
        assert!(intercept("find . -name '*.tmp' -delete", |_| true).is_none());
        assert!(intercept("find . -name '*.rs' -exec wc -l {} +", |_| true).is_none());
    }

    #[test]
    fn reading_one_file_goes_to_read_file() {
        for command in ["cat src/lib.rs", "head README.md", "tail notes.txt"] {
            let out = intercept(command, |_| true)
                .unwrap_or_else(|| panic!("{command} should be blocked"));
            assert!(out.contains("read_file"), "{out}");
        }
    }

    #[test]
    fn a_flagged_or_multi_file_read_is_left_alone() {
        // `read_file` has its own windowing; these ask for something else.
        assert!(intercept("head -n 5 file.txt", |_| true).is_none());
        assert!(intercept("tail -f server.log", |_| true).is_none());
        assert!(intercept("cat a.txt b.txt", |_| true).is_none());
        assert!(intercept("cat", |_| true).is_none());
    }

    #[test]
    fn a_leading_space_bypasses_the_interceptor() {
        assert!(intercept(" grep -rn needle .", |_| true).is_none());
        assert!(intercept("\tcat file.txt", |_| true).is_none());
    }

    #[test]
    fn plumbing_bypasses_the_interceptor() {
        assert!(intercept("grep needle . | head -20", |_| true).is_none());
        assert!(intercept("cat file.txt > copy.txt", |_| true).is_none());
        assert!(intercept("cd src && grep -rn needle .", |_| true).is_none());
        assert!(intercept("grep -rn needle . ; echo done", |_| true).is_none());
        assert!(intercept("echo $(grep -c needle file)", |_| true).is_none());
    }

    #[test]
    fn a_redirect_to_a_missing_tool_is_not_made() {
        // The user switched `search_files` off: `grep` must reach the shell
        // rather than bounce between a blocked command and an unknown tool.
        let only_read = |tool: &str| tool == READ_FILE_TOOL;
        assert!(intercept("grep -rn needle .", only_read).is_none());
        assert!(intercept("find . -name '*.rs'", only_read).is_none());
        assert!(intercept("cat src/lib.rs", only_read).is_some());
    }

    #[test]
    fn ordinary_commands_are_untouched() {
        for command in ["cargo test", "ls -la", "git status", ""] {
            assert!(intercept(command, |_| true).is_none(), "{command}");
        }
    }
}
