//! Thread overview commands: the one snapshot read behind "what needs you",
//! the finish/reopen writes for the user's one boolean, the per-thread seen
//! mark, and renaming. Thread truth comes from the shared
//! `harness_host::SessionService`.

use harness_protocol::ThreadSnapshot;
use tauri::State;

use crate::state::AppState;

/// Every native thread with its derived status (freshness, mid-turn,
/// finished) and which sessions have work in flight right now.
///
/// Threads of a *removed* project are dropped here: their history stays on
/// disk (that's the removal contract), but an overview grouping sessions by
/// workspace would otherwise resurrect the project every time. Removal is a
/// desktop projects concept, so the filter lives in this adapter, not the
/// shared host.
#[tauri::command]
pub(crate) async fn threads_snapshot(state: State<'_, AppState>) -> Result<ThreadSnapshot, String> {
    let mut snapshot = state.service.thread_snapshot().await?;
    drop_removed(
        &mut snapshot,
        &crate::commands::project::read_projects_config().removed,
    );
    Ok(snapshot)
}

/// Drop entries living in explicitly-removed workspaces.
fn drop_removed(snapshot: &mut ThreadSnapshot, removed: &[String]) {
    if removed.is_empty() {
        return;
    }
    snapshot
        .entries
        .retain(|entry| !removed.iter().any(|r| r == &entry.workspace));
}

/// Mark a thread finished. Returns the recorded unix time.
#[tauri::command]
pub(crate) async fn session_finish(state: State<'_, AppState>, id: String) -> Result<i64, String> {
    state.service.finish_session(&id).await
}

/// Reopen a finished thread.
#[tauri::command]
pub(crate) async fn session_reopen(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.service.reopen_session(&id)
}

/// Record that the user just looked at one thread (opened its chat, or
/// watched its turn end). Activity after this mark is "finished while you
/// were away" for that thread until it is opened again.
#[tauri::command]
pub(crate) async fn session_mark_seen(
    state: State<'_, AppState>,
    id: String,
) -> Result<i64, String> {
    state.service.mark_session_seen(&id)
}

/// Give a chat a name of the user's choosing; blank clears it so the chat
/// titles itself by its first message again.
#[tauri::command]
pub(crate) async fn rename_session(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<(), String> {
    state.service.rename_session(&id, &title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(workspace: &str) -> harness_protocol::ThreadEntry {
        harness_protocol::ThreadEntry {
            id: workspace.to_string(),
            workspace: workspace.to_string(),
            model: String::new(),
            created_at: 0,
            last_activity_at: 0,
            title: String::new(),
            last_reply: String::new(),
            message_count: 0,
            mid_turn: false,
            finished_at: 0,
            review_status: String::new(),
            seen_at: 0,
        }
    }

    #[test]
    fn removed_workspaces_drop_out_of_the_snapshot() {
        let mut snapshot = ThreadSnapshot {
            entries: vec![entry("/kept"), entry("/gone"), entry("/kept-too")],
            running: vec![],
        };
        drop_removed(&mut snapshot, &["/gone".to_string()]);
        let workspaces: Vec<_> = snapshot
            .entries
            .iter()
            .map(|e| e.workspace.as_str())
            .collect();
        assert_eq!(workspaces, ["/kept", "/kept-too"]);

        // No removals: untouched.
        drop_removed(&mut snapshot, &[]);
        assert_eq!(snapshot.entries.len(), 2);
    }
}
