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
    let edits = args
        .replacements()
        .expect("empty `edits` beside a real pair is the pair");
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
    let edits = args
        .replacements()
        .expect("an empty pair beside real `edits` is the edits");
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

/// Oct 12, 2026 — gpt-oss-20b (local, MXFP4) sent a lone `old_string` six
/// times in a row. The generic "needs either `edits` or both …" error never
/// told it which half was missing, so it repeated the same call until the
/// loop guard stopped it.
#[test]
fn edit_file_pair_missing_new_string_names_the_missing_field() {
    let args: EditFileArgs = serde_json::from_str(
        r#"{"path":"calc.py","old_string":"return sum(price for price, qty in items)","replace_all":false}"#,
    )
    .unwrap();
    let err = match args.replacements() {
        Ok(_) => panic!("a pair without `new_string` has no replacement text"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("`new_string` is missing"), "{err}");
}

/// Oct 13, 2026 — gpt-6-1-sol fills every line field of every hunk with `0`
/// when it anchors by text. Failed with "anchor a hunk by `old_string` or by
/// line number, not both": the zeros are filler, not a line anchor.
#[test]
fn edit_file_text_hunks_with_zeroed_line_fields() {
    let args: EditFileArgs = serde_json::from_str(
        r#"{"edits":[{"insert_after_line":0,"line_end":0,"line_start":0,"new_string":"References received: [Image #1] orange spiky Ferris crab, glossy black oval eyes, tiny open smile; [Image #2] cream rectangular shaggy ox, tan curved horns, tan rounded-rectangle muzzle, two black nostrils, black dot eyes, tan hooves, heavy black outline. The ox has NO mouth line; never add one. Match both as clean flat 2D outlined characters in a warm illustrated loft. Setting and sequential keyframe generation now underway.","old_string":"Script and research saved. GENERATION BLOCKED: the two user-mentioned ox/crab reference images have not been attached or found in the project. Do not substitute invented characters or spend on character generation until they arrive.","replace_all":false},{"insert_after_line":0,"line_end":0,"line_start":0,"new_string":"**Non-negotiable ox mouth lock:** the reference has NO visible mouth line. Preserve the tan rounded-rectangle muzzle and two nostrils exactly; do NOT add a mouth, smile, frown, teeth, lips, or opening. Keep muzzle and jaw completely static. The ox emotes ONLY through eye direction, eyelids, pupil focus and blinks. Body motion may provide locomotion, not substitute facial expression. No brow/muzzle/ear emoting. Prefer modest motion and stable camera angles to reduce drift. Inspect outputs; prompting alone is not a guarantee.","old_string":"**Non-negotiable ox mouth lock:** copy the exact mouth expression from the reference. Keep that mouth shape, corners, opening, teeth visibility and jaw position unchanged in every still and every frame. No smiling, frowning, chewing, talking, lip motion or jaw motion. The ox emotes ONLY through eye direction, eyelids, pupil focus and blinks. Body motion may provide locomotion, not substitute facial expression. No brow/muzzle/ear emoting. Prefer modest motion and stable camera angles to reduce drift. Inspect outputs; prompting alone is not a guarantee.","replace_all":false}],"new_string":"","old_string":"","path":"production/rust-la-trailer.md","replace_all":false}"#,
    )
    .unwrap();
    let edits = args
        .replacements()
        .expect("zeroed line fields beside a real `old_string` are unused");
    assert_eq!(edits.len(), 2);
    for edit in &edits {
        assert!(!edit.old_string.is_empty());
        assert_eq!(
            (edit.line_start, edit.line_end, edit.insert_after_line),
            (None, None, None)
        );
    }
}

/// Oct 13, 2026 — same session, retrying by line: a real `line_start` beside
/// `insert_after_line: 0`. The zero made both hunks insertions at the top of
/// the file, failing with "edits 1 and 2 overlap in the file".
#[test]
fn edit_file_line_hunks_with_zeroed_insert_after_line() {
    let args: EditFileArgs = serde_json::from_str(
        r#"{"edits":[{"insert_after_line":0,"line_end":0,"line_start":4,"new_string":"References received: [Image #1] orange spiky Ferris crab, glossy black oval eyes, tiny open smile; [Image #2] cream rectangular shaggy ox, tan curved horns, tan rounded-rectangle muzzle, two black nostrils, black dot eyes, tan hooves, heavy black outline. The ox has NO mouth line; never add one. Match both as clean flat 2D outlined characters in a warm illustrated loft. Setting and sequential keyframe generation now underway.","old_string":"","replace_all":false},{"insert_after_line":0,"line_end":0,"line_start":29,"new_string":"**Non-negotiable ox mouth lock:** the reference has NO visible mouth line. Preserve the tan rounded-rectangle muzzle and two nostrils exactly; do NOT add a mouth, smile, frown, teeth, lips, or opening. Keep muzzle and jaw completely static. The ox emotes ONLY through eye direction, eyelids, pupil focus and blinks. Body motion may provide locomotion, not substitute facial expression. No brow/muzzle/ear emoting. Prefer modest motion and stable camera angles to reduce drift. Inspect outputs; prompting alone is not a guarantee.","old_string":"","replace_all":false}],"new_string":"","old_string":"","path":"production/rust-la-trailer.md","replace_all":false}"#,
    )
    .unwrap();
    let edits = args
        .replacements()
        .expect("`insert_after_line: 0` beside a real `line_start` is unused");
    let anchors: Vec<_> = edits
        .iter()
        .map(|e| (e.line_start, e.line_end, e.insert_after_line))
        .collect();
    assert_eq!(anchors, [(Some(4), None, None), (Some(29), None, None)]);
}

/// Oct 13, 2026 — same session: an insertion that echoes its line into
/// `line_start`/`line_end` (all three equal), beside replacements carrying
/// `insert_after_line: 0`. The echo is the insertion's own line, not a
/// conflicting range.
#[test]
fn edit_file_insertion_echoing_its_line_beside_zeroed_replacements() {
    let args: EditFileArgs = serde_json::from_str(
        r#"{"edits":[{"insert_after_line":5,"line_end":5,"line_start":5,"new_string":"import io\nfrom PIL import Image\nassets = {}","old_string":"","replace_all":false},{"insert_after_line":0,"line_end":13,"line_start":13,"new_string":"    uri = f'data:{mime};base64,' + base64.b64encode(path.read_bytes()).decode('ascii')\n    if path.suffix == '.mp4':\n        assets[uri] = f'http://localhost:55378/{path.relative_to(OUT).as_posix()}'\n    else:\n        image = Image.open(path).convert('RGB')\n        image.thumbnail((1280, 1280))\n        buffer = io.BytesIO()\n        image.save(buffer, format='JPEG', quality=82)\n        assets[uri] = 'data:image/jpeg;base64,' + base64.b64encode(buffer.getvalue()).decode('ascii')\n    return uri","old_string":"","replace_all":false},{"insert_after_line":0,"line_end":55,"line_start":54,"new_string":"(OUT / 'breakdown-portable.html').write_text(page)\nfor embedded, compact in assets.items():\n    page = page.replace(embedded, compact)\npage = page.replace(\"This document embeds every displayed image and video. It can be opened or shared without the project's folder structure.\", 'Images are embedded; videos load from the local trailer server. For offline sharing, use breakdown-portable.html.')\n(OUT / 'index.html').write_text(page)\nprint(f'Built compact viewer: {len(page.encode()) / 1024:.0f} KiB; portable offline copy also saved.')","old_string":"","replace_all":false}],"new_string":"","old_string":"","path":"production/build_breakdown.py","replace_all":false}"#,
    )
    .unwrap();
    let edits = args.replacements().expect("each hunk has one clear anchor");
    let anchors: Vec<_> = edits
        .iter()
        .map(|e| (e.line_start, e.line_end, e.insert_after_line))
        .collect();
    assert_eq!(
        anchors,
        [
            (None, None, Some(5)),
            (Some(13), Some(13), None),
            (Some(54), Some(55), None)
        ]
    );
}

/// Two line anchors that genuinely disagree stay an error, and say how to
/// fix it rather than silently picking one.
#[test]
fn edit_file_conflicting_line_anchors_is_a_teaching_error() {
    let args: EditFileArgs = serde_json::from_str(
        r#"{"path":"a.txt","edits":[{"line_start":3,"line_end":4,"insert_after_line":7,"new_string":"x"}]}"#,
    )
    .unwrap();
    let err = match args.replacements() {
        Ok(_) => panic!("replace 3-4 vs insert after 7 is a real conflict"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("send only the one you mean"), "{err}");
}
