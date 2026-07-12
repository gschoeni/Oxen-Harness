//! One place to shell out to `git` and capture stdout.
//!
//! Several crates need to run a git command in a working directory and read
//! its output — the review pipeline computing a diff, the loop detecting what a
//! pass changed, the CLI checking whether a ref exists. Each had grown its own
//! copy with a slightly different error/stderr policy; this is the shared
//! runner they call instead, so the behavior (and the `git … failed: <stderr>`
//! message shape) is defined once. It depends only on `std`, so it belongs in
//! the leaf crate alongside the other cross-cutting helpers.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::bounded::BoundedText;

const MAX_CAPTURE_CHARS: usize = 100_000;

/// Run `git <args>` in `root`, returning captured stdout on success.
///
/// On failure the error carries the command and git's trimmed stderr, so a
/// caller can surface *why* it failed; callers that only care whether it
/// succeeded can `.ok()` the result. Never panics — a missing `git` binary is
/// an `Err`, not a crash.
pub fn capture(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run git: {e}"))?;
    let stdout = child.stdout.take().map(|s| drain(s, MAX_CAPTURE_CHARS));
    let stderr = child.stderr.take().map(|s| drain(s, MAX_CAPTURE_CHARS));
    let status = child
        .wait()
        .map_err(|e| format!("could not wait for git: {e}"))?;
    let stdout = stdout.map(join_reader).unwrap_or_default();
    let stderr = stderr.map(join_reader).unwrap_or_default();
    if !status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(stdout)
}

fn drain(
    mut reader: impl Read + Send + 'static,
    max_chars: usize,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut kept = BoundedText::new(max_chars);
        let mut bytes = [0u8; 8192];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(n) => kept.push(&String::from_utf8_lossy(&bytes[..n])),
            }
        }
        kept.into_string()
    })
}

fn join_reader(handle: std::thread::JoinHandle<String>) -> String {
    handle.join().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_failure_with_stderr() {
        // `git` in a non-repo directory fails; the error names the command.
        let tmp = std::env::temp_dir();
        let err = capture(&tmp, &["rev-parse", "--absurd-flag-xyz"]).unwrap_err();
        assert!(err.starts_with("git rev-parse"), "{err}");
    }

    #[test]
    fn captures_stdout_of_a_trivial_command() {
        // `git --version` succeeds anywhere git is installed; skip if absent.
        let tmp = std::env::temp_dir();
        if let Ok(out) = capture(&tmp, &["--version"]) {
            assert!(out.contains("git"));
        }
    }
}
