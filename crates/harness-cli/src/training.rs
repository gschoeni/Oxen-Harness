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
const LATER: &str = "Choose later";
const KEEP: &str = "Yes, mark as good";
const REJECT: &str = "Reject";

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

/// A snapshot of the training-data curation queue. Each kept session becomes
/// one fine-tuning example when the dataset is exported.
#[derive(Debug, Default, PartialEq, Eq)]
struct TrainingStats {
    kept: usize,
    rejected: usize,
    unreviewed: usize,
}

impl TrainingStats {
    fn collect(store: &HistoryStore) -> Self {
        let mut stats = Self::default();
        if let Ok(sessions) = store.list_sessions() {
            for session in sessions {
                match session.review_status.as_str() {
                    "kept" => stats.kept += 1,
                    "rejected" => stats.rejected += 1,
                    _ => stats.unreviewed += 1,
                }
            }
        }
        stats
    }
}

/// Print an upbeat end-of-session data-labeling report. The totals are
/// best-effort: a reporting failure must never make a clean CLI exit fail.
fn print_report(ui: &Ui, outcome: &str, stats: TrainingStats) {
    let feedback = match outcome {
        "kept" => ui.green("🎉 New gold-star example saved!"),
        "rejected" => ui.brown("🧹 Good call — the training herd stays picky."),
        _ => ui.dim("🗺️ Marked for later — your trail journal is still waiting."),
    };
    println!("\n  {feedback}");
    println!(
        "  {} {}",
        ui.accent("Training-data roundup:"),
        ui.cream(&format!(
            "{} ready to train • {} set aside • {} awaiting review",
            stats.kept, stats.rejected, stats.unreviewed
        )),
    );
    if outcome == "kept" {
        println!(
            "  {}",
            ui.dim("Every good label makes the next export a little wiser. Nice work, trail boss!")
        );
    } else if outcome == "rejected" {
        println!(
            "  {}",
            ui.dim("Honest no's are valuable labels too — thanks for keeping the dataset sharp.")
        );
    } else {
        println!(
            "  {}",
            ui.dim("A quick label next time helps turn strong runs into better training data.")
        );
    }
}

pub(crate) fn prompt_session_review(store: &HistoryStore, session: &str, agent: &Agent, ui: &Ui) {
    let current = store.review_status(session).unwrap_or_default();
    if !worth_asking(agent, &current) {
        return;
    }

    let options = [
        Choice::new(LATER, "leave it unreviewed for now"),
        Choice::new(KEEP, "a good run — include it in the fine-tuning export"),
        Choice::new(REJECT, "not a good example — exclude it from the export"),
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
        print_report(ui, "later", TrainingStats::collect(store));
        return;
    };
    match store.set_review_status(session, status) {
        Ok(()) => print_report(ui, status, TrainingStats::collect(store)),
        Err(e) => println!("  {} {e}", ui.dim("couldn't save the label:")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_count_each_review_outcome() {
        let store = HistoryStore::open_in_memory().unwrap();
        let kept = store.create_session(&Default::default()).unwrap();
        let rejected = store.create_session(&Default::default()).unwrap();
        let later = store.create_session(&Default::default()).unwrap();
        for session in [&kept, &rejected, &later] {
            store
                .append_raw_message(
                    session,
                    "user",
                    Some("label this run"),
                    r#"{"role":"user","content":"label this run"}"#,
                )
                .unwrap();
        }
        store.set_review_status(&kept, "kept").unwrap();
        store.set_review_status(&rejected, "rejected").unwrap();

        assert_eq!(
            TrainingStats::collect(&store),
            TrainingStats {
                kept: 1,
                rejected: 1,
                unreviewed: 1,
            }
        );
    }

    #[test]
    fn labels_map_to_review_statuses() {
        assert_eq!(status_for(KEEP), Some("kept"));
        assert_eq!(status_for("YES, MARK AS GOOD"), Some("kept"));
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
