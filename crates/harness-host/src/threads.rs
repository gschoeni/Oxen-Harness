//! The host's thread overview: one snapshot read answering "what threads
//! exist, which are running, which did the user mark finished" across every
//! project, plus the small writes an overview makes — finishing a thread,
//! reopening one, marking one seen, naming one.
//!
//! Everything the snapshot reports is *derived* truth. Freshness, titles, and
//! mid-turn detection come straight from the transcript; the running set is
//! read from the host's authoritative in-flight registry, so it survives a UI
//! restart. The only human-authored state is the finished mark — by design
//! the one thing a thread can't do to itself.

use std::time::{SystemTime, UNIX_EPOCH};

use harness_protocol::{ThreadEntry, ThreadSnapshot};
use harness_store::{ThreadRow, FINISHED_STATE, SEEN_STATE};

use crate::SessionService;

impl SessionService {
    /// Every native thread with its derived status, and which sessions have
    /// work in flight.
    pub async fn thread_snapshot(&self) -> Result<ThreadSnapshot, String> {
        let entries = self
            .store()?
            .thread_rows()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(entry_from_row)
            .collect();
        Ok(ThreadSnapshot {
            entries,
            running: self.running_sessions().await,
        })
    }

    /// Session ids with work in flight right now — a turn or a review.
    /// Read from the cancel-token registry, which is the
    /// host's single source of truth for "busy" (every long-running operation
    /// registers there for mutual exclusion before it starts).
    pub async fn running_sessions(&self) -> Vec<String> {
        self.cancels.lock().await.keys().cloned().collect()
    }

    /// Mark a thread finished, returning the recorded time. Idempotent —
    /// finishing again just refreshes the mark. Errors when the session
    /// doesn't exist or has work in flight: a list rendered a moment ago can
    /// offer "mark finished" on a thread that just started a turn, and
    /// finishing it would hide a live agent (and its approval prompts). The
    /// UI's render gate is a convenience; this is the contract.
    ///
    /// The running-set lock is held across the whole check-and-write, and a
    /// turn registering its token clears any finished mark right after — so
    /// however a finish races a turn start, the outcome is coherent: a
    /// running thread is never finished.
    pub async fn finish_session(&self, session: &str) -> Result<i64, String> {
        let cancels = self.cancels.lock().await;
        if cancels.contains_key(session) {
            return Err(
                "this thread is mid-turn — wait for it to finish (or stop it) before marking it finished"
                    .to_string(),
            );
        }
        let store = self.store()?;
        // A clean "no such session" beats a foreign-key violation string.
        store.session_meta(session).map_err(|e| e.to_string())?;
        let finished_at = now();
        // Written while the lock is still held: a turn can't slip into the
        // gap between the check above and this write.
        store
            .save_session_state(
                session,
                FINISHED_STATE,
                &serde_json::json!({ "settled_at": finished_at }),
            )
            .map_err(|e| e.to_string())?;
        drop(cancels);
        Ok(finished_at)
    }

    /// Reopen a finished thread. Idempotent.
    pub fn reopen_session(&self, session: &str) -> Result<(), String> {
        self.store()?
            .clear_session_state(session, FINISHED_STATE)
            .map_err(|e| e.to_string())
    }

    /// Running and finished are mutually exclusive: called right after a run
    /// (turn, loop, review) registers its cancellation token, so a finish
    /// that landed a beat before the registration — the other side of the
    /// [`Self::finish_session`] race — is undone. A thread that runs again is
    /// open again, whatever a list said a moment ago.
    pub(crate) fn reopen_for_run(&self, session: &str) {
        if let Ok(store) = self.store() {
            let _ = store.clear_session_state(session, FINISHED_STATE);
        }
    }

    /// Record that the user just looked at one thread — opened its chat, or
    /// watched its turn finish — returning the new mark. Anything that lands
    /// on the thread after this is "finished while you were away" until the
    /// user opens it again.
    pub fn mark_session_seen(&self, session: &str) -> Result<i64, String> {
        let store = self.store()?;
        store.session_meta(session).map_err(|e| e.to_string())?;
        let seen = now();
        store
            .save_session_state(session, SEEN_STATE, &seen)
            .map_err(|e| e.to_string())?;
        Ok(seen)
    }

    /// Give a chat a name of the user's choosing (blank clears it). Every
    /// title read — the history list, an overview, a tab — shows it from then
    /// on; the transcript itself is untouched.
    pub fn rename_session(&self, session: &str, title: &str) -> Result<(), String> {
        let store = self.store()?;
        store.session_meta(session).map_err(|e| e.to_string())?;
        store
            .rename_session(session, title)
            .map_err(|e| e.to_string())
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Shape one store row into its wire entry.
fn entry_from_row(row: ThreadRow) -> ThreadEntry {
    ThreadEntry {
        mid_turn: matches!(row.last_role.as_str(), "user" | "tool"),
        id: row.id,
        workspace: row.workspace,
        model: row.model,
        created_at: row.created_at,
        last_activity_at: row.last_activity_at,
        title: row.title,
        last_reply: row.last_reply,
        message_count: row.message_count,
        finished_at: row.finished_at,
        review_status: row.review_status,
        seen_at: row.seen_at,
    }
}
