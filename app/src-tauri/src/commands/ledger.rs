//! The Ledger — the home board's commands: the one snapshot read behind the
//! trail map, the settle/reopen writes for tying threads off, the seen mark
//! for "since you left", and the per-workspace git overviews rendered on each
//! project's banner. Thread truth comes from the shared
//! `harness_host::SessionService`; git truth is read fresh from each
//! workspace, because only the filesystem knows.

use std::collections::HashMap;

use harness_core::git::GitOverview;
use harness_protocol::{LedgerSnapshot, SettleState};
use tauri::State;

use crate::state::AppState;

/// Everything the Ledger board needs, in one read: every native thread with
/// its derived status (freshness, plan progress, mid-turn, settled), which
/// sessions have work in flight right now, and when the board was last seen.
///
/// Threads of a *removed* project are dropped here: their history stays on
/// disk (that's the removal contract), but the board deriving trains from
/// sessions would otherwise resurrect the project as a nameless train every
/// time. Removal is a desktop projects concept, so the filter lives in this
/// adapter, not the shared host.
#[tauri::command]
pub(crate) async fn ledger_snapshot(state: State<'_, AppState>) -> Result<LedgerSnapshot, String> {
    let mut snapshot = state.service.ledger_snapshot().await?;
    drop_removed(
        &mut snapshot,
        &crate::commands::project::read_projects_config().removed,
    );
    Ok(snapshot)
}

/// Drop entries living in explicitly-removed workspaces.
fn drop_removed(snapshot: &mut LedgerSnapshot, removed: &[String]) {
    if removed.is_empty() {
        return;
    }
    snapshot
        .entries
        .retain(|entry| !removed.iter().any(|r| r == &entry.workspace));
}

/// Tie off a thread, optionally with a one-line closing note. Returns the
/// recorded settle mark.
#[tauri::command]
pub(crate) async fn settle_session(
    state: State<'_, AppState>,
    id: String,
    note: Option<String>,
) -> Result<SettleState, String> {
    state.service.settle_session(&id, note.as_deref().unwrap_or(""))
}

/// Bring a settled thread back to the trail.
#[tauri::command]
pub(crate) async fn reopen_session(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.service.reopen_session(&id)
}

/// Record that the user just looked at the board; the next visit renders its
/// "since you left" story against the returned mark.
#[tauri::command]
pub(crate) async fn ledger_mark_seen(state: State<'_, AppState>) -> Result<i64, String> {
    state.service.mark_ledger_seen()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(workspace: &str) -> harness_protocol::LedgerEntry {
        harness_protocol::LedgerEntry {
            id: workspace.to_string(),
            workspace: workspace.to_string(),
            model: String::new(),
            created_at: 0,
            last_activity_at: 0,
            title: String::new(),
            last_reply: String::new(),
            message_count: 0,
            mid_turn: false,
            plan: None,
            trail: None,
            settle: None,
            review_status: String::new(),
        }
    }

    #[test]
    fn removed_workspaces_drop_out_of_the_snapshot() {
        let mut snapshot = LedgerSnapshot {
            entries: vec![entry("/kept"), entry("/gone"), entry("/kept-too")],
            running: vec![],
            last_seen: 0,
        };
        drop_removed(&mut snapshot, &["/gone".to_string()]);
        let workspaces: Vec<_> = snapshot.entries.iter().map(|e| e.workspace.as_str()).collect();
        assert_eq!(workspaces, ["/kept", "/kept-too"]);

        // No removals: untouched.
        drop_removed(&mut snapshot, &[]);
        assert_eq!(snapshot.entries.len(), 2);
    }
}

/// Git overviews for a set of workspaces, keyed by the requested path.
/// Workspaces that aren't git repositories are simply absent from the result.
/// Each overview shells out to git a few times, so the batch runs off the
/// async runtime with one thread per workspace — the slowest repo, not the
/// sum, bounds the wall clock.
#[tauri::command]
pub(crate) async fn workspace_git(
    paths: Vec<String>,
) -> Result<HashMap<String, GitOverview>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::scope(|scope| {
            let handles: Vec<_> = paths
                .iter()
                .map(|path| {
                    scope.spawn(|| {
                        harness_core::git::overview(std::path::Path::new(path))
                            .map(|overview| (path.clone(), overview))
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|h| h.join().ok().flatten())
                .collect()
        })
    })
    .await
    .map_err(|e| e.to_string())
}
