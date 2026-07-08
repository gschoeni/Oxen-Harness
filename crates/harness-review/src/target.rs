//! What a review looks at, and the diff text that anchors every step.
//!
//! The diff is computed mechanically (never left to the model to improvise)
//! and substituted into each step's prompt. Step agents still carry the full
//! tool set, so they can — and are prompted to — read beyond the diff; the
//! embedded text just pins *which change* is under review.

use std::path::Path;
use std::process::Command;

use crate::config::DIFF_CHAR_BUDGET;
use crate::ReviewError;

/// What `/code-review` reviews.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewTarget {
    /// The working tree: staged, unstaged, and untracked files.
    Uncommitted,
    /// Everything on this branch (plus the working tree) relative to the merge
    /// base with `branch` — a PR-style review.
    BaseBranch(String),
}

impl ReviewTarget {
    /// A short human label ("uncommitted changes", "changes against `main`").
    pub fn label(&self) -> String {
        match self {
            ReviewTarget::Uncommitted => "uncommitted changes".to_string(),
            ReviewTarget::BaseBranch(branch) => format!("changes against `{branch}`"),
        }
    }
}

/// The change under review, resolved from a [`ReviewTarget`]: a description
/// for prompts and headers, and the unified diff text.
#[derive(Clone, Debug)]
pub struct ReviewInput {
    pub target: ReviewTarget,
    /// Prompt-facing description: the label, how the diff was produced, and
    /// the untracked-file list when relevant.
    pub description: String,
    /// The unified diff, truncated past [`DIFF_CHAR_BUDGET`] with a note.
    pub diff: String,
}

/// Resolve a target into the diff the pipeline reviews. Fails with
/// [`ReviewError::NothingToReview`] when the target has no changes, and
/// [`ReviewError::Git`] when git can't answer (not a repo, unknown branch).
pub fn resolve_target(root: &Path, target: ReviewTarget) -> Result<ReviewInput, ReviewError> {
    match &target {
        ReviewTarget::Uncommitted => {
            // `HEAD` covers staged + unstaged in one diff; an unborn HEAD
            // (fresh repo, no commits) falls back to the index diff.
            let diff = git(root, &["diff", "HEAD"])
                .or_else(|_| git(root, &["diff"]))
                .map_err(|e| ReviewError::Git(format!("could not diff the working tree: {e}")))?;
            let untracked =
                git(root, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
            let untracked: Vec<&str> = untracked.lines().filter(|l| !l.is_empty()).collect();
            if diff.trim().is_empty() && untracked.is_empty() {
                return Err(ReviewError::NothingToReview);
            }
            let mut description =
                "uncommitted changes (staged, unstaged, and untracked), from `git diff HEAD`"
                    .to_string();
            if !untracked.is_empty() {
                description.push_str(&format!(
                    "\nUntracked files, not in the diff below — read them with your tools:\n{}",
                    untracked
                        .iter()
                        .map(|p| format!("  - {p}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            Ok(ReviewInput {
                target,
                description,
                diff: budget_diff(&diff),
            })
        }
        ReviewTarget::BaseBranch(branch) => {
            let merge_base = git(root, &["merge-base", "HEAD", branch]).map_err(|_| {
                ReviewError::Git(format!(
                    "could not find a merge base with `{branch}` — is it a branch or ref in this repo?"
                ))
            })?;
            let merge_base = merge_base.trim().to_string();
            // Diff the *working tree* against the merge base, so the review
            // covers committed work and anything still uncommitted on top.
            let diff = git(root, &["diff", &merge_base])
                .map_err(|e| ReviewError::Git(format!("could not diff against `{branch}`: {e}")))?;
            if diff.trim().is_empty() {
                return Err(ReviewError::NothingToReview);
            }
            let short = &merge_base[..merge_base.len().min(12)];
            Ok(ReviewInput {
                target: ReviewTarget::BaseBranch(branch.clone()),
                description: format!(
                    "changes against base branch `{branch}` (merge base {short}), from `git diff {short}` — committed and working-tree changes together"
                ),
                diff: budget_diff(&diff),
            })
        }
    }
}

/// Run a git command in `root`, returning stdout on success and the combined
/// failure detail otherwise.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
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

/// Truncate an oversized diff on a line boundary, telling the step agents how
/// to read the rest themselves.
fn budget_diff(diff: &str) -> String {
    if diff.len() <= DIFF_CHAR_BUDGET {
        return diff.to_string();
    }
    let mut cut = DIFF_CHAR_BUDGET;
    while cut > 0 && !diff.is_char_boundary(cut) {
        cut -= 1;
    }
    let kept = match diff[..cut].rfind('\n') {
        Some(nl) => &diff[..nl],
        None => &diff[..cut],
    };
    format!(
        "{kept}\n… [diff truncated at ~{DIFF_CHAR_BUDGET} chars — run the git diff \
         command named in the TARGET line yourself to read the rest]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A committed git repo in a temp dir; skips (None) when git is missing.
    fn repo() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        run(&root, &["init", "-q", "-b", "main"])?;
        run(&root, &["config", "user.email", "t@t"])?;
        run(&root, &["config", "user.name", "t"])?;
        std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
        run(&root, &["add", "."])?;
        run(&root, &["commit", "-q", "-m", "init"])?;
        Some((dir, root))
    }

    fn run(root: &Path, args: &[&str]) -> Option<()> {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|_| ())
    }

    #[test]
    fn clean_tree_has_nothing_to_review() {
        let Some((_d, root)) = repo() else { return };
        assert!(matches!(
            resolve_target(&root, ReviewTarget::Uncommitted),
            Err(ReviewError::NothingToReview)
        ));
    }

    #[test]
    fn uncommitted_covers_edits_and_lists_untracked() {
        let Some((_d, root)) = repo() else { return };
        std::fs::write(root.join("a.rs"), "fn main() { changed(); }\n").unwrap();
        std::fs::write(root.join("new.rs"), "fn fresh() {}\n").unwrap();
        let input = resolve_target(&root, ReviewTarget::Uncommitted).unwrap();
        assert!(input.diff.contains("changed()"));
        assert!(input.description.contains("new.rs"));
    }

    #[test]
    fn base_branch_reviews_commits_and_working_tree_since_merge_base() {
        let Some((_d, root)) = repo() else { return };
        run(&root, &["checkout", "-q", "-b", "feature"]).unwrap();
        std::fs::write(root.join("a.rs"), "fn main() { committed(); }\n").unwrap();
        run(&root, &["add", "."]).unwrap();
        run(&root, &["commit", "-q", "-m", "work"]).unwrap();
        std::fs::write(root.join("a.rs"), "fn main() { committed(); pending(); }\n").unwrap();

        let input = resolve_target(&root, ReviewTarget::BaseBranch("main".into())).unwrap();
        assert!(input.diff.contains("committed()"));
        assert!(input.diff.contains("pending()"));
        assert!(input.description.contains("`main`"));

        assert!(matches!(
            resolve_target(&root, ReviewTarget::BaseBranch("no-such-branch".into())),
            Err(ReviewError::Git(_))
        ));
    }

    #[test]
    fn oversized_diffs_truncate_on_a_line_boundary_with_a_note() {
        let line = format!("+{}\n", "x".repeat(99));
        let big = line.repeat(DIFF_CHAR_BUDGET / 100 + 10);
        let out = budget_diff(&big);
        assert!(out.len() < big.len());
        assert!(out.contains("diff truncated"));
        // The cut falls on a line boundary, not mid-line.
        let before_note = out.split("\n… [diff truncated").next().unwrap();
        assert!(before_note.ends_with('x'));
        assert_eq!(before_note.lines().last().unwrap().len(), 100);
    }
}
