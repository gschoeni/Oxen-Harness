//! The model-dialects corpus: tool arguments transcribed VERBATIM from real
//! sessions, replayed against the argument parsers forever.
//!
//! A tool's JSON Schema says what we'd like; models under momentum speak a
//! dialect — every advertised field filled in with the unused ones left empty,
//! optional-looking fields omitted, shapes borrowed from sibling tools they
//! know better. Schema-level unit tests can't predict that dialect, but once
//! it's observed it must never break again.
//!
//! The discipline, whenever a tool call fails in the wild:
//!   1. Pull the exact arguments from the history DB:
//!      `sqlite3 ~/.oxen-harness/history.sqlite` — the assistant message's
//!      `tool_calls[].function.arguments` is stored verbatim.
//!   2. Paste them here, dated, as a failing test.
//!   3. Fix the tool to read the model's INTENT (an empty form is "unused",
//!      never an error), keeping only genuine ambiguity as a failure — and
//!      make that failure message teach, because the model reads it.
//!
//! And when SHIPPING a new tool, write its dialect cases up front: omit each
//! non-load-bearing field, send both forms at once with one empty, and mirror
//! the arguments of the sibling tool models will confuse it with.

use harness_tools::fs::EditFileArgs;
use harness_tools::{parse_trail_arguments, TypedTool};

/// Jul 26, 2026 — the model's first-ever `update_trail` calls. Perfect
/// waypoints, no `title`: the shape mirrors `update_plan`, which has none.
/// Failed with serde's terse "missing field `title`" twice, taught nothing,
/// and the model gave up charting. Titles are now optional (persistence keeps
/// the previously charted name; the board falls back to the first message).
#[test]
fn update_trail_without_a_title_first_charting() {
    let trail = parse_trail_arguments(
        r#"{"waypoints":[{"name":"inspect","status":"current"},{"name":"design","status":"ahead"},{"name":"implement","status":"ahead"},{"name":"verify","status":"ahead"}]}"#,
    )
    .expect("a titleless trail is a valid trail");
    assert_eq!(trail.title, "");
    assert_eq!(trail.waypoints.len(), 4);
}

/// Same session, later: advancing waypoints, still no title.
#[test]
fn update_trail_without_a_title_mid_ride() {
    let trail = parse_trail_arguments(
        r#"{"waypoints":[{"name":"inspect","status":"done"},{"name":"design","status":"done"},{"name":"implement","status":"current"},{"name":"verify","status":"ahead"}]}"#,
    )
    .expect("waypoint updates may omit the title");
    assert_eq!(trail.waypoints[2].name, "implement");
}

/// The tool result must nudge toward a title, not error — the result string
/// is the one channel the model reliably reads.
#[tokio::test]
async fn update_trail_untitled_result_teaches() {
    let out = harness_tools::TrailTool::new()
        .invoke(serde_json::json!({
            "waypoints": [
                {"name": "inspect", "status": "current"},
                {"name": "verify", "status": "ahead"},
            ]
        }))
        .await
        .expect("untitled call succeeds");
    assert!(out.contains("tip: include `title`"), "{out}");
}

/// Jul 26, 2026 — `edit_file` with every schema field present at once: a real
/// pair beside an EMPTY `edits` list. Failed with "pass either `edits` or a
/// single pair, not both". The empty list is the unused half of the schema,
/// not a second request.
#[test]
fn edit_file_real_pair_beside_empty_edits_list() {
    let args: EditFileArgs = serde_json::from_str(
        r#"{"path":"src/app/(app)/account/api-keys/page.tsx","edits":[],"old_string":"import { createClient } from \"@/lib/supabase/server\";","new_string":"import { KeyIcon } from \"@/components/ui/icons\";\nimport { createClient } from \"@/lib/supabase/server\";","replace_all":false}"#,
    )
    .unwrap();
    let edits = args.replacements().expect("empty `edits` beside a real pair is the pair");
    assert_eq!(edits.len(), 1);
    assert!(edits[0].new_string.contains("KeyIcon"));
}

/// Jul 26, 2026 — the mirror dialect: populated `edits` beside an EMPTY
/// `old_string`/`new_string` pair. Same session, same rejection.
#[test]
fn edit_file_real_edits_beside_empty_pair() {
    let args: EditFileArgs = serde_json::from_str(
        r#"{"path":"src/app/(app)/account/api-keys/page.tsx","edits":[{"old_string":"a","new_string":"b","replace_all":false},{"old_string":"c","new_string":"d","replace_all":false}],"old_string":"","new_string":"","replace_all":false}"#,
    )
    .unwrap();
    let edits = args.replacements().expect("an empty pair beside real `edits` is the edits");
    assert_eq!(edits.len(), 2);
}

/// Both forms carrying real content stays an error — the combined ordering
/// would be a guess — but the message now says what to do instead.
#[test]
fn edit_file_two_populated_forms_is_a_teaching_error() {
    let args: EditFileArgs = serde_json::from_str(
        r#"{"path":"a.txt","edits":[{"old_string":"a","new_string":"b"}],"old_string":"c","new_string":"d"}"#,
    )
    .unwrap();
    let err = match args.replacements() {
        Ok(_) => panic!("two populated forms must not silently combine"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("put every change in `edits`"), "{err}");
}
