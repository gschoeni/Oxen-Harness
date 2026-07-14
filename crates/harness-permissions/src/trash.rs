//! Move-to-trash instead of delete.
//!
//! macOS ships no trash CLI, so "trash" here is a harness-managed directory:
//! `~/.oxen-harness/trash/<epoch-ms>/`. When the user picks "move to trash
//! instead" on an `rm` approval, the gate rewrites the command into a `mv`
//! into a fresh timestamped folder — explicit and reversible, never a silent
//! semantic change. Old entries are pruned on gate construction after a TTL.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::classify::TrashPlan;

/// How long trashed files are kept before pruning.
const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Build the replacement command for an approved-as-trash deletion, plus the
/// note prepended to the tool result so the model knows what actually happened.
/// Returns `None` if the trash directory can't be resolved.
pub(crate) fn rewrite(plan: &TrashPlan) -> Option<(String, String)> {
    let dir = harness_config::paths::trash_dir().ok()?.join(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string(),
    );
    let dir_q = quote(&dir.to_string_lossy());
    let targets = plan
        .targets
        .iter()
        .map(|t| quote(t))
        .collect::<Vec<_>>()
        .join(" ");
    let command = format!("mkdir -p {dir_q} && mv {targets} {dir_q}/");
    let note = format!(
        "(The user chose to move these files to the harness trash instead of deleting them. \
         They are recoverable at {} for 7 days.)",
        dir.display()
    );
    Some((command, note))
}

/// Single-quote a string for the shell (`'` → `'\''`).
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Best-effort prune of trash entries older than the TTL. Called once per gate
/// construction; failures are traced and ignored.
pub(crate) fn prune_expired() {
    let Ok(dir) = harness_config::paths::trash_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    for entry in entries.flatten() {
        if expired(&entry.path(), now) {
            if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                tracing::debug!("trash prune failed for {}: {e}", entry.path().display());
            }
        }
    }
}

/// A trash entry's age comes from its epoch-millis directory name.
fn expired(path: &Path, now: Duration) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Ok(created_ms) = name.parse::<u64>() else {
        return false;
    };
    now.saturating_sub(Duration::from_millis(created_ms)) > TTL
}

#[allow(unused)]
pub(crate) fn trash_root() -> Option<PathBuf> {
    harness_config::paths::trash_dir().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_quotes_targets_and_notes_recovery() {
        let _env = crate::testutil::env_guard();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("OXEN_HARNESS_DIR", home.path());
        let plan = TrashPlan {
            targets: vec!["build".into(), "it's here".into()],
        };
        let (command, note) = rewrite(&plan).unwrap();
        std::env::remove_var("OXEN_HARNESS_DIR");
        assert!(command.starts_with("mkdir -p '"));
        assert!(command.contains("&& mv 'build' 'it'\\''s here' '"));
        assert!(note.contains("harness trash"));
    }

    #[test]
    fn expiry_is_driven_by_the_directory_name() {
        let now = Duration::from_millis(10_000_000_000_000);
        assert!(expired(Path::new("/t/1"), now)); // ancient
        let fresh = now.as_millis() as u64 - 1000;
        assert!(!expired(Path::new(&format!("/t/{fresh}")), now));
        assert!(!expired(Path::new("/t/not-a-timestamp"), now));
    }
}
