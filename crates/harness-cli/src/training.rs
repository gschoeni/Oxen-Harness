//! The end-of-session training-data prompt.
//!
//! Every session is persisted with a per-session `review_status` (`""` =
//! unreviewed, `"kept"`, `"rejected"`) — the same field the desktop app's
//! Training-data builder curates and exports fine-tuning JSONL from. When an
//! interactive CLI session ends, offer to label the run right there, while
//! the user still remembers whether it went well — so good traces accumulate
//! without a separate review pass.

use harness_agent::Agent;
use harness_store::HistoryStore;

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// The labels offered at exit and the `review_status` each maps to.
const KEEP: &str = "It was great!";
const REJECT: &str = "Leave it behind";
const LATER: &str = "Decide later";

/// Map a picked label to the review status to persist (`None` = leave as-is).
/// Typed free-text answers also count when they clearly mean keep/reject.
fn status_for(picked: &str) -> Option<&'static str> {
    let p = picked.trim().to_ascii_lowercase();
    if p == KEEP.to_ascii_lowercase()
        || p == "keep"
        || p == "kept"
        || p == "good"
        || p == "great"
        || p == "yes"
    {
        Some("kept")
    } else if p == REJECT.to_ascii_lowercase() || p == "reject" || p == "rejected" || p == "bad" {
        Some("rejected")
    } else {
        None
    }
}

/// Whether the session is worth asking about: it had at least one real user
/// prompt (not a bare open-then-quit) and hasn't already been labeled.
fn worth_asking(agent: &Agent, current_status: &str) -> bool {
    current_status.is_empty() && agent.messages().iter().any(|m| m.role == "user")
}

/// Offer to label the finished session as training data. Runs after the death
/// screen, in cooked mode; skips silently when the session is empty, already
/// labeled, the terminal isn't interactive, or the user cancels/esc-es out.
pub(crate) fn prompt_session_review(store: &HistoryStore, session: &str, agent: &Agent, ui: &Ui) {
    let current = store.review_status(session).unwrap_or_default();
    if !worth_asking(agent, &current) {
        return;
    }

    let options = [
        Choice::new(KEEP, "a good run — include it in the fine-tuning export"),
        Choice::new(REJECT, "not a good example — exclude it from the export"),
        Choice::new(LATER, "leave it unreviewed for now"),
    ];
    let picked = match picker::select(
        ui,
        "Training data",
        "One last thing — was this a good run? Kept sessions become fine-tuning \
         examples when you export training data (desktop app → Settings → Training data).",
        &options,
        false,
    ) {
        Ok(Some(sel)) => sel.into_iter().next().unwrap_or_default(),
        // Cancelled or no interactive terminal — don't hold up the exit.
        _ => return,
    };

    let Some(status) = status_for(&picked) else {
        println!(
            "  {}",
            ui.dim("left unreviewed — label it any time from the desktop app's Training data page")
        );
        return;
    };
    match store.set_review_status(session, status) {
        Ok(()) => {
            let line = if status == "kept" {
                "kept — this run will feed the herd on the next training export"
            } else {
                "rejected — this run stays out of the training export"
            };
            println!("  {} {}", ui.green("✓"), ui.dim(line));
        }
        Err(e) => println!("  {} {e}", ui.dim("couldn't save the label:")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_map_to_review_statuses() {
        assert_eq!(status_for(KEEP), Some("kept"));
        assert_eq!(status_for("It Was Great!"), Some("kept"));
        assert_eq!(status_for("keep"), Some("kept"));
        assert_eq!(status_for("great"), Some("kept"));
        assert_eq!(status_for("YES"), Some("kept"));
        assert_eq!(status_for(REJECT), Some("rejected"));
        assert_eq!(status_for("bad"), Some("rejected"));
        assert_eq!(status_for(LATER), None);
        assert_eq!(status_for("whatever else"), None);
        assert_eq!(status_for(""), None);
    }
}
