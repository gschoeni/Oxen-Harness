//! What a shell keeps between commands.
//!
//! Every `run_shell` call used to spawn a fresh `sh -c`, so `cd subdir` did
//! nothing for the next call, `source .venv/bin/activate` was a no-op, and
//! `export` evaporated. Models work around that by chaining `cd x && …` onto
//! every command — wasted tokens, and a steady source of quoting bugs.
//!
//! Rather than embedding a shell, each command still runs in its own `sh -c`
//! but inherits the session's working directory and environment, and reports
//! its own back afterwards through a temp file. `cd` and `export` therefore
//! persist without a long-lived shell process to babysit.
//!
//! Limits worth knowing: shell *functions*, aliases, and `set -e` style
//! options do not survive (they never leave the process), and a command that
//! ends by replacing the shell — `exit 3`, `exec` — skips the trailer, so the
//! previous state simply stays. Both degrade to today's behavior.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Variables that describe the shell rather than the user's intent. Carrying
/// them forward would pin a stale value onto the next command.
const VOLATILE: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_", "RANDOM", "LINENO", "SECONDS"];

/// Cap on the environment carried between commands. A process that exports a
/// megabyte of JSON shouldn't make every later command pay for it.
const MAX_ENV_BYTES: usize = 32 * 1024;

/// The separator between the two records the trailer writes (cwd, then env).
/// ASCII record separator: it cannot appear in a path and is vanishingly
/// unlikely in an environment value.
const RECORD_SEP: char = '\u{1e}';

/// The working directory and exported variables carried between commands.
#[derive(Debug, Clone)]
pub struct ShellSession {
    cwd: PathBuf,
    env: BTreeMap<String, String>,
}

impl ShellSession {
    /// A session rooted at the workspace, inheriting nothing yet.
    pub fn new(root: &Path) -> Self {
        Self {
            cwd: root.to_path_buf(),
            env: BTreeMap::new(),
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    /// Wrap `command` so it reports its final directory and environment into
    /// `state_path` without disturbing its own exit code or output.
    pub fn wrap(command: &str, state_path: &Path) -> String {
        let state = state_path.display();
        if cfg!(windows) {
            format!(
                "{command}\r\n\
                 @set __oxen_rc=%errorlevel%\r\n\
                 @(cd & echo {RECORD_SEP} & set) > \"{state}\" 2>nul\r\n\
                 @exit /b %__oxen_rc%"
            )
        } else {
            format!(
                "{command}\n\
                 __oxen_rc=$?\n\
                 {{ pwd; printf '{RECORD_SEP}\\n'; env; }} > '{state}' 2>/dev/null\n\
                 exit $__oxen_rc"
            )
        }
    }

    /// Fold a trailer's report back into the session.
    ///
    /// `root` bounds the directory: a command may `cd` anywhere for its own
    /// duration, but a cwd outside the workspace is not made sticky — the
    /// sandbox story stays true, and the caller says so in the tool result.
    /// Returns a note when the directory changed, for the model to see.
    pub fn absorb(&mut self, report: &str, root: &Path) -> Option<String> {
        let (cwd_text, env_text) = report.split_once(RECORD_SEP)?;
        let reported = PathBuf::from(cwd_text.trim());

        let mut note = None;
        if reported != self.cwd && reported.is_dir() {
            let inside = reported
                .canonicalize()
                .ok()
                .zip(root.canonicalize().ok())
                .is_some_and(|(dir, root)| dir.starts_with(root));
            if inside {
                self.cwd = reported.clone();
                note = Some(format!("working directory is now {}", reported.display()));
            } else {
                note = Some(format!(
                    "working directory {} is outside the project, so later commands \
                     still run from {}",
                    reported.display(),
                    self.cwd.display()
                ));
            }
        }

        self.env = parse_env(env_text);
        note
    }
}

/// Parse `env` output into variables. A line without a `NAME=` prefix is a
/// continuation of the previous value, which is how multi-line exports (a PEM
/// key, a formatted JSON blob) survive the round trip.
fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut bytes = 0usize;

    for line in text.lines() {
        match line.split_once('=').filter(|(name, _)| is_var_name(name)) {
            Some((name, value)) => {
                current = Some(name.to_string());
                if VOLATILE.contains(&name) {
                    current = None;
                    continue;
                }
                bytes += name.len() + value.len();
                if bytes > MAX_ENV_BYTES {
                    break;
                }
                vars.insert(name.to_string(), value.to_string());
            }
            None => {
                if let Some(name) = &current {
                    if let Some(value) = vars.get_mut(name) {
                        value.push('\n');
                        value.push_str(line);
                    }
                }
            }
        }
    }
    vars
}

fn is_var_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(cwd: &str, env: &str) -> String {
        format!("{cwd}\n{RECORD_SEP}\n{env}")
    }

    #[test]
    fn a_cd_inside_the_project_sticks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let sub = root.join("src");
        std::fs::create_dir(&sub).unwrap();
        let mut session = ShellSession::new(&root);

        let note = session.absorb(&report(&sub.display().to_string(), ""), &root);

        assert_eq!(session.cwd(), sub.as_path());
        assert!(note.unwrap().contains("working directory is now"));
    }

    #[test]
    fn a_cd_outside_the_project_is_reported_but_not_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let outside = std::env::temp_dir().canonicalize().unwrap();
        let mut session = ShellSession::new(&root);

        let note = session.absorb(&report(&outside.display().to_string(), ""), &root);

        assert_eq!(session.cwd(), root.as_path());
        assert!(note.unwrap().contains("outside the project"));
    }

    #[test]
    fn exports_carry_forward_and_volatile_variables_do_not() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut session = ShellSession::new(&root);

        session.absorb(
            &report(
                &root.display().to_string(),
                "API_TOKEN=abc123\nPWD=/somewhere\nSHLVL=2\nVIRTUAL_ENV=/proj/.venv\n",
            ),
            &root,
        );

        assert_eq!(session.env().get("API_TOKEN").unwrap(), "abc123");
        assert_eq!(session.env().get("VIRTUAL_ENV").unwrap(), "/proj/.venv");
        // The shell's own bookkeeping would pin a stale value onto the next
        // command; the process supplies its own.
        assert!(!session.env().contains_key("PWD"));
        assert!(!session.env().contains_key("SHLVL"));
    }

    #[test]
    fn a_multi_line_value_survives_the_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut session = ShellSession::new(&root);

        session.absorb(
            &report(
                &root.display().to_string(),
                "KEY=-----BEGIN-----\nline two\nline three\nOTHER=plain\n",
            ),
            &root,
        );

        assert_eq!(
            session.env().get("KEY").unwrap(),
            "-----BEGIN-----\nline two\nline three"
        );
        assert_eq!(session.env().get("OTHER").unwrap(), "plain");
    }

    #[test]
    fn a_runaway_environment_is_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut session = ShellSession::new(&root);
        let huge: String = (0..200)
            .map(|i| format!("VAR_{i}={}\n", "x".repeat(500)))
            .collect();

        session.absorb(&report(&root.display().to_string(), &huge), &root);

        let total: usize = session.env().iter().map(|(k, v)| k.len() + v.len()).sum();
        assert!(total <= MAX_ENV_BYTES + 1_000, "kept {total} bytes");
    }

    #[test]
    fn a_report_the_shell_never_wrote_leaves_the_session_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut session = ShellSession::new(&root);
        session.env.insert("KEEP".into(), "yes".into());

        // `exit 3` replaces the shell before the trailer runs: no separator.
        assert!(session.absorb("", &root).is_none());
        assert_eq!(session.cwd(), root.as_path());
        assert_eq!(session.env().get("KEEP").unwrap(), "yes");
    }
}
