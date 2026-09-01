//! `/resume` — pick up an earlier trail in this workspace.
//!
//! One keystroke back into a conversation: `/resume` opens a picker over the
//! most recent sessions for the working directory, `/resume <id-prefix>` jumps
//! straight to one. The same rows feed the startup banner's "Recent trails"
//! block (see [`crate::theme::trail_summary`]), so what you see at launch is
//! what the picker offers.
//!
//! Resuming swaps the live [`Agent`] for one rebuilt from the store, which is
//! why the REPL context carries an agent factory: the client, tools, and config
//! are all cheap to clone, so a resume is a rebuild rather than a restart.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use harness_agent::Agent;
use harness_store::HistoryStore;

use crate::picker::{self, Choice};
use crate::repl_loop::ReplContext;
use crate::theme::{relative_age, trail_summary, RecentTrail, Ui};
use crate::turn::ends_mid_turn;

/// How many trails the banner shows, and how many the picker lists.
pub(crate) const BANNER_TRAILS: usize = 4;
pub(crate) const PICKER_TRAILS: usize = 12;

/// The most recent resumable sessions for `workspace_root`, newest first.
///
/// Imported transcripts (Claude Code / Cursor) are review-only — they share the
/// store but must never resume as a live agent — and `exclude` drops the
/// session already open, which is a trail you're standing on, not one to
/// pick up.
pub(crate) fn recent_trails(
    store: &HistoryStore,
    workspace_root: &Path,
    exclude: &str,
    limit: usize,
) -> Vec<RecentTrail> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    store
        .list_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.source.is_empty() && s.id != exclude)
        .filter(|s| Path::new(&s.workspace) == workspace_root)
        .take(limit)
        .map(|s| RecentTrail {
            title: s.title.unwrap_or_default(),
            age: relative_age(now - s.created_at),
            entries: s.message_count,
            id: s.id,
        })
        .collect()
}

/// Resolve a session id from a user-typed prefix, requiring it to be
/// unambiguous — two matches is a question, not a guess.
fn resolve<'a>(trails: &'a [RecentTrail], prefix: &str) -> Result<&'a RecentTrail, String> {
    let matches: Vec<&RecentTrail> = trails.iter().filter(|t| t.id.starts_with(prefix)).collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("no trail in this camp starts with `{prefix}`")),
        many => Err(format!(
            "`{prefix}` matches {} trails — type more of the id",
            many.len()
        )),
    }
}

/// The picker rows for a set of trails: the id is the label (it's what
/// `/resume <id>` takes), the shared summary is the description.
fn options(trails: &[RecentTrail]) -> Vec<Choice> {
    trails
        .iter()
        .map(|t| Choice::new(t.id.clone(), trail_summary(t)))
        .collect()
}

/// Ask which trail to pick up. `None` when the user backs out, or when there's
/// no interactive terminal to ask on.
pub(crate) fn choose(ui: &Ui, trails: &[RecentTrail]) -> Option<String> {
    if trails.is_empty() {
        return None;
    }
    let chosen = picker::select(
        ui,
        "Resume",
        "Which trail do you want to pick back up?",
        &options(trails),
        false,
    )
    .ok()??;
    chosen.into_iter().next()
}

/// The startup picker behind a bare `--resume`: choose a session for
/// `workspace_root` before the REPL builds its agent. `None` (nothing on
/// record, or the user escaped) means "set out fresh".
pub(crate) fn pick_at_startup(
    store: &HistoryStore,
    workspace_root: &Path,
    ui: &Ui,
) -> Option<String> {
    let trails = recent_trails(store, workspace_root, "", PICKER_TRAILS);
    if trails.is_empty() {
        println!(
            "  {}",
            ui.dim("no trails on record for this camp — setting out fresh")
        );
        return None;
    }
    choose(ui, &trails)
}

/// The lines printed when a transcript is restored, shared by the startup path
/// in `main` and the in-REPL `/resume` so both greet a resumed trail the same
/// way. A transcript that stops mid-turn (the reply never landed — provider
/// error, no internet, a crash) can be continued in place with `/retry`.
pub(crate) fn restored_lines(ui: &Ui, entries: usize, mid_turn: bool) -> Vec<String> {
    let mut lines = vec![format!(
        "  {} {}",
        ui.green("↺ Picking up the trail:"),
        ui.cream(&format!("{entries} journal entries restored")),
    )];
    if mid_turn {
        lines.push(format!(
            "  {} {}",
            ui.red("⚠"),
            ui.dim(
                "this expedition stopped mid-turn — /retry is pre-filled, \
                 press ⏎ to pick up where it left off"
            ),
        ));
    }
    lines
}

/// `/resume [id-prefix]` — swap the live agent for an earlier session's.
///
/// With no argument this opens the picker over this workspace's recent trails;
/// with one it matches by id prefix. The agent is rebuilt from the store
/// through the context's factory, so the session, transcript, and mid-turn
/// state all move together.
pub(crate) async fn handle_repl(
    rest: Option<String>,
    agent: &mut Agent,
    ui: &Ui,
    ctx: &ReplContext<'_>,
) -> Result<()> {
    let current = ctx.session();
    let trails = recent_trails(ctx.store, ctx.workspace_root, &current, PICKER_TRAILS);
    if trails.is_empty() {
        println!(
            "  {}",
            ui.dim("no other trails on record for this camp yet")
        );
        return Ok(());
    }

    let id = match rest {
        Some(prefix) => match resolve(&trails, prefix.trim()) {
            Ok(trail) => trail.id.clone(),
            Err(why) => {
                println!("  {}", ui.red(&why));
                return Ok(());
            }
        },
        None => match choose(ui, &trails) {
            Some(id) => id,
            // Cancelled, or no interactive terminal: list what's on offer so a
            // piped session can still `/resume <id>`.
            None => {
                println!("  {}", ui.brown("Recent trails"));
                for trail in &trails {
                    println!(
                        "    {} {}",
                        ui.accent(&trail.id[..trail.id.len().min(8)]),
                        ui.dim(&trail_summary(trail)),
                    );
                }
                return Ok(());
            }
        },
    };

    *agent = ctx.resume_agent(&id)?;
    for line in restored_lines(ui, agent.messages().len(), ends_mid_turn(agent.messages())) {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::Message;
    use harness_store::SessionMeta;

    fn trail(id: &str) -> RecentTrail {
        RecentTrail {
            id: id.into(),
            title: "t".into(),
            age: "1m ago".into(),
            entries: 2,
        }
    }

    #[test]
    fn resolve_needs_an_unambiguous_prefix() {
        let trails = [trail("abc123"), trail("abd999"), trail("zzz000")];
        assert_eq!(resolve(&trails, "abc").unwrap().id, "abc123");
        assert_eq!(resolve(&trails, "zzz000").unwrap().id, "zzz000");
        // Two candidates is a question, not a coin flip.
        assert!(resolve(&trails, "ab").unwrap_err().contains("matches 2"));
        assert!(resolve(&trails, "q").unwrap_err().contains("no trail"));
    }

    #[test]
    fn recent_trails_are_this_workspace_only_newest_first() {
        let store = HistoryStore::open_in_memory().unwrap();
        let here = "/tmp/here";
        let mine = |workspace: &str, prompt: &str| {
            let id = store
                .create_session(&SessionMeta {
                    workspace: workspace.into(),
                    ..Default::default()
                })
                .unwrap();
            store.append_message(&id, &Message::user(prompt)).unwrap();
            id
        };
        let older = mine(here, "older here");
        let newer = mine(here, "newer here");
        mine("/tmp/elsewhere", "another camp");
        // An imported transcript in the same directory: review-only, never
        // something to resume as a live agent.
        store
            .import_conversations(
                "claude-code",
                &[harness_store::ImportedConversation {
                    source_ref: "ext-1".into(),
                    workspace: here.into(),
                    model: "m".into(),
                    created_at: 10,
                    messages: vec![serde_json::json!({
                        "role": "user",
                        "content": "from claude code",
                    })],
                }],
            )
            .unwrap();

        let trails = recent_trails(&store, Path::new(here), "", 10);
        let mut ids: Vec<&str> = trails.iter().map(|t| t.id.as_str()).collect();
        ids.sort_unstable();
        // This workspace only (ordering is `list_sessions`', newest first), and
        // never another tool's imported transcript.
        let mut want = vec![older.as_str(), newer.as_str()];
        want.sort_unstable();
        assert_eq!(ids, want);
        let newest = trails.iter().find(|t| t.id == newer).expect("listed");
        assert_eq!(newest.title, "newer here");
        assert_eq!(newest.entries, 1);
        assert_eq!(newest.age, "just now");

        // The session you're already in isn't offered as somewhere to go.
        let others = recent_trails(&store, Path::new(here), &newer, 10);
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].id, older);

        // The limit clips the list.
        assert_eq!(recent_trails(&store, Path::new(here), "", 1).len(), 1);
    }

    #[test]
    fn restored_lines_add_the_retry_hint_only_mid_turn() {
        let ui = Ui::plain();
        let settled = restored_lines(&ui, 41, false);
        assert_eq!(settled.len(), 1);
        assert!(settled[0].contains("41 journal entries restored"));
        let dangling = restored_lines(&ui, 3, true);
        assert_eq!(dangling.len(), 2);
        assert!(dangling[1].contains("/retry"));
    }

    #[test]
    fn picker_rows_are_labelled_by_id_and_described_by_the_shared_summary() {
        let rows = options(&[trail("abc123")]);
        assert_eq!(rows[0].label, "abc123");
        assert_eq!(rows[0].description, trail_summary(&trail("abc123")));
    }
}
