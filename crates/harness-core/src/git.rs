//! One place to shell out to `git` and capture stdout.
//!
//! Several crates need to run a git command in a working directory and read
//! its output — the review pipeline computing a diff, the loop detecting what a
//! pass changed, the CLI checking whether a ref exists. Each had grown its own
//! copy with a slightly different error/stderr policy; this is the shared
//! runner they call instead, so the behavior (and the `git … failed: <stderr>`
//! message shape) is defined once. It depends only on `std`, so it belongs in
//! the leaf crate alongside the other cross-cutting helpers.

use std::path::Path;
use std::process::Command;

/// Run `git <args>` in `root`, returning captured stdout on success.
///
/// On failure the error carries the command and git's trimmed stderr, so a
/// caller can surface *why* it failed; callers that only care whether it
/// succeeded can `.ok()` the result. Never panics — a missing `git` binary is
/// an `Err`, not a crash.
pub fn capture(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
