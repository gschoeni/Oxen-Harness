//! Version control for everything that isn't in the history database — config,
//! project data, and shareable conversation traces — backed by [Oxen].
//!
//! We deliberately **shell out to the `oxen` CLI** rather than depend on the
//! `liboxen` crate. `liboxen` pulls in Polars + Arrow + the AWS SDK + a bundled
//! DuckDB C++ build (hundreds of transitive crates, multi-minute cold builds),
//! and the operations we need — `init`, `add`, `commit`, `push`, `clone` — are
//! exactly the stable CLI verbs. The CLI also shares `~/.config/oxen` auth, so a
//! user who has run `oxen config --auth …` is already set up to push/share. See
//! `03-decisions.md`.
//!
//! Everything goes through a [`Runner`] so the argv we build and the output
//! parsing are unit-testable without the binary installed; production uses
//! [`SystemRunner`] (`std::process::Command`).
//!
//! [Oxen]: https://docs.oxen.ai/getting-started/intro

use std::path::Path;
use std::process::Output;

mod trace;

pub use trace::{export_trace, TraceAttachment, TraceBundle};

/// Default name of the Oxen executable on `PATH`.
pub const OXEN_BIN: &str = "oxen";

/// Errors from driving Oxen.
#[derive(Debug, thiserror::Error)]
pub enum OxenError {
    #[error(
        "the `oxen` CLI is not installed or not on PATH — install it from \
         https://docs.oxen.ai/getting-started/install to enable versioning"
    )]
    NotInstalled,
    #[error("`{program} {args}` failed (exit {code}): {stderr}")]
    Command {
        program: String,
        args: String,
        code: i32,
        stderr: String,
    },
    #[error("oxen IO failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Executes external commands. Abstracted so the [`Oxen`] argv and output
/// handling can be tested with a fake.
pub trait Runner {
    /// Run `program` with `args`, optionally in `cwd`, and capture its output.
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> std::io::Result<Output>;
}

/// Runs commands as real subprocesses via [`std::process::Command`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[&str], cwd: Option<&Path>) -> std::io::Result<Output> {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.output()
    }
}

/// A thin, typed wrapper over the `oxen` CLI.
#[derive(Debug, Clone)]
pub struct Oxen<R: Runner = SystemRunner> {
    runner: R,
    program: String,
}

impl Default for Oxen<SystemRunner> {
    fn default() -> Self {
        Self::new()
    }
}

impl Oxen<SystemRunner> {
    /// An Oxen client that shells out to the `oxen` binary on `PATH`.
    pub fn new() -> Self {
        Self {
            runner: SystemRunner,
            program: OXEN_BIN.to_string(),
        }
    }
}

impl<R: Runner> Oxen<R> {
    /// Build with a custom [`Runner`] (used in tests).
    pub fn with_runner(runner: R) -> Self {
        Self {
            runner,
            program: OXEN_BIN.to_string(),
        }
    }

    /// Whether the `oxen` binary is present and runnable (`oxen --version`).
    pub fn is_available(&self) -> bool {
        self.runner
            .run(&self.program, &["--version"], None)
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Whether `dir` is already an Oxen repository (has a `.oxen/` directory).
    pub fn is_repo(&self, dir: &Path) -> bool {
        dir.join(".oxen").is_dir()
    }

    /// `oxen init` — create a repository in `dir`.
    pub fn init(&self, dir: &Path) -> Result<(), OxenError> {
        self.run(&["init"], Some(dir)).map(drop)
    }

    /// `oxen add <path>` — stage a path (relative to the repo `dir`).
    pub fn add(&self, dir: &Path, path: &str) -> Result<(), OxenError> {
        self.run(&["add", path], Some(dir)).map(drop)
    }

    /// `oxen commit -m <message>`.
    pub fn commit(&self, dir: &Path, message: &str) -> Result<(), OxenError> {
        self.run(&["commit", "-m", message], Some(dir)).map(drop)
    }

    /// `oxen config --set-remote <name> <url>` — point the repo at a remote.
    pub fn set_remote(&self, dir: &Path, name: &str, url: &str) -> Result<(), OxenError> {
        self.run(&["config", "--set-remote", name, url], Some(dir))
            .map(drop)
    }

    /// `oxen push <remote> <branch>`.
    pub fn push(&self, dir: &Path, remote: &str, branch: &str) -> Result<(), OxenError> {
        self.run(&["push", remote, branch], Some(dir)).map(drop)
    }

    /// `oxen clone <url>` into `dst`'s parent (Oxen creates the repo directory).
    pub fn clone(&self, url: &str, dst: &Path) -> Result<(), OxenError> {
        let dst = dst.to_string_lossy();
        self.run(&["clone", url, &dst], None).map(drop)
    }

    /// Initialize the repo if needed, stage everything, and commit. Returns
    /// `true` if a commit was made, `false` if there was nothing to commit (not
    /// an error — config that didn't change shouldn't fail a snapshot).
    pub fn snapshot(&self, dir: &Path, message: &str) -> Result<bool, OxenError> {
        if !self.is_repo(dir) {
            self.init(dir)?;
        }
        self.add(dir, ".")?;
        match self.run(&["commit", "-m", message], Some(dir)) {
            Ok(_) => Ok(true),
            Err(OxenError::Command { stderr, .. }) if is_nothing_to_commit(&stderr) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Run an `oxen` subcommand, turning a non-zero exit into [`OxenError`] and a
    /// missing binary into [`OxenError::NotInstalled`].
    fn run(&self, args: &[&str], cwd: Option<&Path>) -> Result<Output, OxenError> {
        let output = match self.runner.run(&self.program, args, cwd) {
            Ok(output) => output,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(OxenError::NotInstalled)
            }
            Err(e) => return Err(OxenError::Io(e)),
        };
        if output.status.success() {
            return Ok(output);
        }
        Err(OxenError::Command {
            program: self.program.clone(),
            args: args.join(" "),
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

/// Whether a commit's stderr indicates an empty/no-op commit, which `snapshot`
/// treats as success.
fn is_nothing_to_commit(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("nothing to commit") || s.contains("no changes") || s.contains("nothing staged")
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    /// A recorded invocation: the argv (joined) and the cwd it ran in.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Call {
        pub args: String,
        pub cwd: Option<String>,
    }

    /// A [`Runner`] that records calls and replays scripted outputs instead of
    /// spawning processes.
    pub struct FakeRunner {
        pub calls: RefCell<Vec<Call>>,
        /// `(exit_code, stderr)` returned for each call, in order; the last entry
        /// repeats once exhausted.
        responses: RefCell<Vec<(i32, String)>>,
    }

    impl FakeRunner {
        pub fn ok() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                responses: RefCell::new(vec![(0, String::new())]),
            }
        }

        pub fn with_responses(responses: Vec<(i32, String)>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                responses: RefCell::new(responses),
            }
        }

        pub fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl Runner for FakeRunner {
        fn run(
            &self,
            _program: &str,
            args: &[&str],
            cwd: Option<&Path>,
        ) -> std::io::Result<Output> {
            self.calls.borrow_mut().push(Call {
                args: args.join(" "),
                cwd: cwd.map(|c| c.to_string_lossy().to_string()),
            });
            let mut responses = self.responses.borrow_mut();
            let (code, stderr) = if responses.len() > 1 {
                responses.remove(0)
            } else {
                responses[0].clone()
            };
            Ok(Output {
                // A Unix wait-status encodes the exit code in bits 8-15.
                status: ExitStatus::from_raw(code << 8),
                stdout: Vec::new(),
                stderr: stderr.into_bytes(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn builds_expected_argv_for_each_verb() {
        let dir = std::path::PathBuf::from("/repo");
        let oxen = Oxen::with_runner(FakeRunner::ok());

        oxen.init(&dir).unwrap();
        oxen.add(&dir, "transcript.jsonl").unwrap();
        oxen.commit(&dir, "msg").unwrap();
        oxen.set_remote(&dir, "origin", "https://hub.oxen.ai/me/repo")
            .unwrap();
        oxen.push(&dir, "origin", "main").unwrap();

        let calls = oxen.runner.calls();
        assert_eq!(calls[0].args, "init");
        assert_eq!(calls[0].cwd.as_deref(), Some("/repo"));
        assert_eq!(calls[1].args, "add transcript.jsonl");
        assert_eq!(calls[2].args, "commit -m msg");
        assert_eq!(
            calls[3].args,
            "config --set-remote origin https://hub.oxen.ai/me/repo"
        );
        assert_eq!(calls[4].args, "push origin main");
    }

    #[test]
    fn nonzero_exit_becomes_command_error() {
        let oxen = Oxen::with_runner(FakeRunner::with_responses(vec![(1, "boom".into())]));
        let err = oxen.commit(Path::new("/repo"), "x").unwrap_err();
        match err {
            OxenError::Command { code, stderr, .. } => {
                assert_eq!(code, 1);
                assert_eq!(stderr, "boom");
            }
            other => panic!("expected Command error, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_treats_nothing_to_commit_as_no_op() {
        let dir = tempfile::tempdir().unwrap();
        // is_repo() is false (no .oxen dir), so snapshot runs init, add, commit.
        // The commit reports "nothing to commit", which must not be an error.
        let oxen = Oxen::with_runner(FakeRunner::with_responses(vec![
            (0, String::new()),              // init
            (0, String::new()),              // add .
            (1, "Nothing to commit".into()), // commit
        ]));
        let committed = oxen.snapshot(dir.path(), "snapshot").unwrap();
        assert!(!committed);
    }
}
