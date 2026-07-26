//! The Ledger's host surface: one snapshot read answering "what threads exist,
//! which are running, what's left untied" across every project, plus the small
//! writes the board makes — settling (tying off) a thread, reopening one, and
//! marking the board seen.
//!
//! Everything the snapshot reports is *derived* truth. Freshness, titles, and
//! mid-turn detection come straight from the transcript; plan progress is the
//! projection the agent persists on every `update_plan` call (backfilled here,
//! once, for sessions that predate it); the running set is read from the
//! host's authoritative in-flight registry, so it survives a UI restart. The
//! only human-authored state is the settle mark — by design the one thing a
//! wagon can't do to itself.

use std::time::{SystemTime, UNIX_EPOCH};

use harness_protocol::{LedgerEntry, LedgerSnapshot, PlanProgress, SettleState};
use harness_store::{HistoryStore, LedgerRow, PLAN_STATE, SETTLE_STATE};
use harness_tools::PlanSnapshot;

use crate::SessionService;

/// `app_meta` key: unix seconds the Ledger was last marked seen.
const LEDGER_SEEN_META: &str = "ledger_last_seen";

impl SessionService {
    /// Everything the Ledger board needs, in one read: every native thread
    /// with its derived status, which sessions have work in flight, and when
    /// the board was last looked at.
    pub async fn ledger_snapshot(&self) -> Result<LedgerSnapshot, String> {
        let store = self.store()?;
        let entries = store
            .ledger_rows()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|row| entry_from_row(&store, row))
            .collect();
        Ok(LedgerSnapshot {
            entries,
            running: self.running_sessions().await,
            last_seen: store
                .meta_get_i64(LEDGER_SEEN_META)
                .map_err(|e| e.to_string())?
                .unwrap_or(0),
        })
    }

    /// Session ids with work in flight right now — a turn, a review, or a
    /// verification loop. Read from the cancel-token registry, which is the
    /// host's single source of truth for "busy" (every long-running operation
    /// registers there for mutual exclusion before it starts).
    pub async fn running_sessions(&self) -> Vec<String> {
        self.cancels.lock().await.keys().cloned().collect()
    }

    /// Tie off a thread: record when and (optionally) the user's one-line
    /// closing note. Settling is idempotent — settling again just refreshes
    /// the mark. Errors when the session doesn't exist.
    pub fn settle_session(&self, session: &str, note: &str) -> Result<SettleState, String> {
        let store = self.store()?;
        // A clean "no such session" beats a foreign-key violation string.
        store.session_meta(session).map_err(|e| e.to_string())?;
        let state = SettleState {
            settled_at: now(),
            note: note.trim().to_string(),
        };
        store
            .save_session_state(session, SETTLE_STATE, &state)
            .map_err(|e| e.to_string())?;
        Ok(state)
    }

    /// Bring a settled thread back to the trail. Idempotent.
    pub fn reopen_session(&self, session: &str) -> Result<(), String> {
        self.store()?
            .clear_session_state(session, SETTLE_STATE)
            .map_err(|e| e.to_string())
    }

    /// Record that the user just looked at the board, returning the new mark.
    /// The *next* visit renders its "since you left" story against this.
    pub fn mark_ledger_seen(&self) -> Result<i64, String> {
        let store = self.store()?;
        let seen = now();
        store
            .meta_set_i64(LEDGER_SEEN_META, seen)
            .map_err(|e| e.to_string())?;
        Ok(seen)
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Shape one store row into its wire entry, resolving the plan projection.
fn entry_from_row(store: &HistoryStore, row: LedgerRow) -> LedgerEntry {
    let plan = match &row.plan_json {
        // `null` is a real verdict ("checked, no plan"), distinct from a
        // missing key ("never checked") — see the backfill below.
        Some(raw) => serde_json::from_str::<Option<PlanSnapshot>>(raw)
            .ok()
            .flatten(),
        None => backfill_plan(store, &row.id),
    };
    LedgerEntry {
        mid_turn: matches!(row.last_role.as_str(), "user" | "tool"),
        plan: plan.map(|p| PlanProgress {
            done: p.done,
            total: p.total,
            active: p.active,
        }),
        // TrailSnapshot and TrailProgress are serde-compatible (wire-tested),
        // so the stored JSON parses straight into the wire shape.
        trail: row
            .trail_json
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok()),
        settle: row
            .settle_json
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok()),
        review_status: row.review_status,
        id: row.id,
        workspace: row.workspace,
        model: row.model,
        created_at: row.created_at,
        last_activity_at: row.last_activity_at,
        title: row.title,
        last_reply: row.last_reply,
        message_count: row.message_count,
    }
}

/// One-time plan recovery for a session that predates persisted snapshots:
/// walk its candidate messages newest-first for the last valid `update_plan`
/// call, then persist the verdict either way (`None` stores as JSON `null`) so
/// the scan never runs for this session again.
fn backfill_plan(store: &HistoryStore, session_id: &str) -> Option<PlanSnapshot> {
    let candidates = store.plan_message_candidates(session_id).ok()?;
    let plan = candidates.iter().find_map(|raw| last_plan_call_in(raw));
    let _ = store.save_session_state(session_id, PLAN_STATE, &plan);
    plan
}

/// The last valid `update_plan` call in one assistant message, condensed.
fn last_plan_call_in(raw: &str) -> Option<PlanSnapshot> {
    let msg: serde_json::Value = serde_json::from_str(raw).ok()?;
    msg.get("tool_calls")?
        .as_array()?
        .iter()
        .rev()
        .find_map(|call| {
            let f = call.get("function")?;
            if f.get("name")?.as_str()? != harness_tools::PLAN_TOOL {
                return None;
            }
            let items = harness_tools::parse_plan_arguments(f.get("arguments")?.as_str()?)?;
            Some(harness_tools::plan_snapshot(&items))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_plan_call_ignores_prose_and_other_tools() {
        // Prose mention only — the LIKE prefilter let it through, we must not.
        assert!(last_plan_call_in(r#"{"role":"assistant","content":"update_plan is neat"}"#)
            .is_none());

        // A different tool plus a real plan call: the plan call wins.
        let msg = serde_json::json!({
            "role": "assistant",
            "tool_calls": [
                {"function": {"name": "read_file", "arguments": "{}"}},
                {"function": {"name": "update_plan", "arguments":
                    r#"{"plan":[{"content":"A","active_form":"Doing A","status":"completed"},
                                {"content":"B","active_form":"Doing B","status":"in_progress"}]}"#}}
            ]
        })
        .to_string();
        let snap = last_plan_call_in(&msg).unwrap();
        assert_eq!((snap.done, snap.total), (1, 2));
        assert_eq!(snap.active.as_deref(), Some("Doing B"));

        // A malformed plan call parses to no plan, not a panic.
        let bad = serde_json::json!({
            "role": "assistant",
            "tool_calls": [{"function": {"name": "update_plan", "arguments": "{\"plan\":[]}"}}]
        })
        .to_string();
        assert!(last_plan_call_in(&bad).is_none());
    }
}
