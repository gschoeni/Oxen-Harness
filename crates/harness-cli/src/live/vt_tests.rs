//! Golden screen tests: replay the bytes a capturing [`Live`] painted through
//! a vt100 terminal emulator and assert on the resulting grid — the only
//! coverage that catches row-arithmetic and escape-sequence regressions the
//! state-level unit tests can't see.
//!
//! [`Live`]: super::Live

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossterm::event::KeyCode;
use harness_agent::AgentEvent;

use super::test_support::{capture_live, capture_live_at, key, rows_text, screen};

fn token(t: &str) -> AgentEvent {
    AgentEvent::Token(t.to_string())
}

/// Spike: prove the emulator honors the sequences the live TUI is built on —
/// DECSTBM scroll regions, absolute cursor addressing, save/restore cursor,
/// and clear-line — before any golden test depends on them.
#[test]
fn vt100_emulates_the_scroll_region_machinery() {
    let mut parser = vt100::Parser::new(6, 20, 0);
    // Carve a region over rows 1..=4, park at its bottom, and write three lines.
    parser.process(b"\x1b[1;4r\x1b[4;1H");
    parser.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
    // Five lines through a 4-row region: the first scrolled away, the rest
    // stack up to the region bottom, and rows below the region stay blank.
    let rows = rows_text(&parser);
    assert_eq!(rows[0], "two");
    assert_eq!(rows[1], "three");
    assert_eq!(rows[2], "four");
    assert_eq!(rows[3], "five");
    assert_eq!(rows[4], "");
    assert_eq!(rows[5], "");

    // Pinned-area writes: save cursor, address a row below the region, clear +
    // write, restore. The region content must be untouched and the output
    // cursor back where it was.
    parser.process(b"\x1b7\x1b[6;1H\x1b[2Kpinned\x1b8");
    let rows = rows_text(&parser);
    assert_eq!(rows[3], "five");
    assert_eq!(rows[5], "pinned");
    parser.process(b"\r\nsix");
    let rows = rows_text(&parser);
    assert_eq!(rows[3], "six", "region cursor must survive save/restore");
    assert_eq!(rows[5], "pinned", "pinned row must survive region scroll");
}

/// Streaming tokens with spinner ticks interleaved: the finished markdown
/// lines stack in the region, the spinner rides below the last content line
/// with one blank row above it and one below, and `finish()` leaves no
/// spinner remnant behind.
#[test]
fn streaming_keeps_the_spinner_below_the_output_and_finish_clears_it() {
    let (mut live, handle) = capture_live(40, 12);
    let paused = Arc::new(AtomicBool::new(false));
    live.begin_turn(&[]);
    live.tick_spinner();
    live.on_event(&token("alpha line\n"), &paused);
    live.tick_spinner();
    live.on_event(&token("beta "), &paused);
    live.tick_spinner();
    live.on_event(&token("line\n"), &paused);

    // Mid-stream: content rows are clean, the spinner is the single non-blank
    // row under the output, breathing one blank row on each side.
    let rows = rows_text(&screen(&handle, 40, 12, ""));
    let alpha = rows.iter().position(|r| r == "alpha line").expect("alpha");
    assert_eq!(rows[alpha + 1], "beta line", "content stacks in order");
    assert_eq!(
        rows[alpha + 2],
        "",
        "one blank row must separate the output from the spinner:\n{}",
        rows.join("\n")
    );
    assert!(
        !rows[alpha + 3].is_empty(),
        "spinner must ride two rows below the output:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        rows[alpha + 4],
        "",
        "one blank row must follow the spinner:\n{}",
        rows.join("\n")
    );

    live.finish();
    let rows = rows_text(&screen(&handle, 40, 12, ""));
    let alpha = rows.iter().position(|r| r == "alpha line").expect("alpha");
    assert_eq!(rows[alpha + 1], "beta line");
    for r in alpha + 2..alpha + 5 {
        assert_eq!(
            rows[r],
            "",
            "finish must leave no spinner remnant on row {r}:\n{}",
            rows.join("\n")
        );
    }
}

/// With the region full, the padded spinner still shows exactly one blank
/// row between the last line of output and itself, and exactly one between
/// itself and the divider — the breathing room survives the scrolling.
#[test]
fn the_spinner_breathes_one_row_each_side_at_the_bottom_of_a_full_region() {
    let (mut live, handle) = capture_live(40, 12);
    live.begin_turn(&[]);
    for i in 1..=20 {
        live.print_line(&format!("line {i}"));
    }
    live.tick_spinner();

    let rows = rows_text(&screen(&handle, 40, 12, ""));
    let divider = rows
        .iter()
        .position(|r| r.starts_with("──"))
        .expect("divider");
    assert_eq!(
        rows[divider - 1],
        "",
        "blank row under the spinner:\n{}",
        rows.join("\n")
    );
    assert!(
        !rows[divider - 2].is_empty() && !rows[divider - 2].starts_with("line"),
        "spinner two rows above the divider:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        rows[divider - 3],
        "",
        "blank row above the spinner:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        rows[divider - 4],
        "line 20",
        "last output line above that:\n{}",
        rows.join("\n")
    );
}

/// A region too short for the padded spinner (a tiny terminal) falls back to
/// the bare one-row tail rather than scrolling the spinner into scrollback.
#[test]
fn a_tiny_region_uses_a_bare_spinner() {
    let (mut live, handle) = capture_live(40, 4);
    live.begin_turn(&[]);
    live.print_line("only");
    live.tick_spinner();
    let rows = rows_text(&screen(&handle, 40, 4, ""));
    let only = rows.iter().position(|r| r == "only").expect("only");
    assert!(
        !rows[only + 1].is_empty() && !rows[only + 1].starts_with("──"),
        "bare spinner directly under the output:\n{}",
        rows.join("\n")
    );
}

/// A mid-turn announcement (steering, retry) printed while the spinner is
/// animating must land on its own clean line — the spinner is lifted for the
/// write and redrawn below, never left embedded in the scrollback.
#[test]
fn print_line_during_a_spinner_lands_clean_and_moves_the_spinner_down() {
    let (mut live, handle) = capture_live(40, 12);
    live.begin_turn(&[]); // thinking spinner is now the region tail
    live.tick_spinner();
    live.print_line("steering: fix the tests");
    live.tick_spinner();

    let rows = rows_text(&screen(&handle, 40, 12, ""));
    let at = rows
        .iter()
        .position(|r| r.contains("steering: fix the tests"))
        .expect("announcement must be on screen");
    assert_eq!(
        rows[at],
        "steering: fix the tests",
        "the announcement row must hold nothing but the announcement:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        rows[at + 1],
        "",
        "blank row under the announcement:\n{}",
        rows.join("\n")
    );
    assert!(
        !rows[at + 2].is_empty(),
        "spinner must be redrawn below the announcement:\n{}",
        rows.join("\n")
    );
}

/// The void a tall picker leaves behind: its card scrolled the conversation
/// up, and once erased those rows are blank for good. Reclaiming must carve
/// the region right under the decision echo (the probed cursor row) and let
/// the pinned area walk down with the output — no gap between the spinner
/// and the divider, no nudging of rows already on screen — until it reaches
/// the terminal's bottom rows and anchors there as usual.
#[test]
fn a_void_left_by_a_picker_is_closed_by_following_the_conversation() {
    let (mut live, handle) = capture_live(40, 12);
    let paused = Arc::new(AtomicBool::new(false));
    live.begin_turn(&[]);
    for i in 1..=8 {
        live.print_line(&format!("line {i}"));
    }
    let approval = AgentEvent::ApprovalPending {
        name: "shell".into(),
        command: "rm -rf ./build".into(),
    };
    live.on_event(&approval, &paused);

    // What the cooked-mode picker does: draw a ten-row card at the cursor
    // (scrolling most of the conversation away), then erase it and leave a
    // two-row echo in its place — eight blank rows under the cursor.
    let card: String = (1..=10).map(|i| format!("card {i}\r\n")).collect();
    handle.inject(card.as_bytes());
    handle
        .inject(b"\x1b[10A\r\x1b[J[approval] rm -rf ./build\r\n\xe2\x9c\x93 chosen: Run once\r\n");
    let (cursor_row, _) = screen(&handle, 40, 12, "").screen().cursor_position();
    handle.set_cursor_row(Some(cursor_row + 1));

    live.on_event(
        &AgentEvent::ApprovalResolved {
            name: "shell".into(),
            command: "rm -rf ./build".into(),
            decision: "approved".into(),
        },
        &paused,
    );
    live.tick_spinner();

    let rows = rows_text(&screen(&handle, 40, 12, ""));
    let approved = rows
        .iter()
        .position(|r| r.contains("approved — rm -rf ./build"))
        .expect("decision line");
    let divider = rows
        .iter()
        .position(|r| r.starts_with("──"))
        .expect("divider");
    assert_eq!(
        rows[approved + 1],
        "",
        "blank row under the decision:\n{}",
        rows.join("\n")
    );
    assert!(
        !rows[approved + 2].is_empty(),
        "spinner:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        rows[approved + 3],
        "",
        "blank row under the spinner:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        divider,
        approved + 4,
        "the divider must sit right under the spinner's breathing row, not at the bottom:\n{}",
        rows.join("\n")
    );
    assert!(
        rows[divider + 2..].iter().all(String::is_empty),
        "the void moves below the composer:\n{}",
        rows.join("\n")
    );

    // Output walks the pinned area down one row per line, never nudging the
    // rows already on screen…
    live.print_line("after 1");
    live.tick_spinner();
    let rows = rows_text(&screen(&handle, 40, 12, ""));
    assert!(
        rows[approved].contains("approved — rm -rf ./build"),
        "no nudge:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        rows[approved + 1],
        "after 1",
        "output lands where the spinner sat:\n{}",
        rows.join("\n")
    );
    let divider = rows
        .iter()
        .position(|r| r.starts_with("──"))
        .expect("divider");
    assert_eq!(
        divider,
        approved + 5,
        "divider walked down one row:\n{}",
        rows.join("\n")
    );

    // …until it reaches the bottom rows and anchors there for good.
    for i in 2..=8 {
        live.print_line(&format!("after {i}"));
    }
    live.tick_spinner();
    let rows = rows_text(&screen(&handle, 40, 12, ""));
    assert!(
        rows.last().is_some_and(|r| !r.is_empty()),
        "composer back on the bottom row:\n{}",
        rows.join("\n")
    );
    assert!(
        rows[10].starts_with("──"),
        "divider anchored above it:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        rows[9],
        "",
        "blank row under the spinner:\n{}",
        rows.join("\n")
    );
    assert!(!rows[8].is_empty(), "spinner:\n{}", rows.join("\n"));
    assert_eq!(
        rows[7],
        "",
        "blank row above the spinner:\n{}",
        rows.join("\n")
    );
    assert_eq!(rows[6], "after 8", "last output line:\n{}", rows.join("\n"));
}

/// Once a conversation is under way a layout starts right under it: the
/// idle prompt after a short reply sits one blank row below the output, the
/// rest of the screen empty beneath the composer — not the composer on the
/// bottom row with a void above it.
#[test]
fn a_layout_started_mid_conversation_sits_right_under_the_output() {
    let prelude = "reply line 1\r\nreply line 2\r\n";
    let (mut live, handle) = capture_live_at(40, 12, 3);
    live.render();
    let rows = rows_text(&screen(&handle, 40, 12, prelude));
    assert_eq!(rows[1], "reply line 2");
    assert_eq!(
        rows[2],
        "",
        "one blank row under the output:\n{}",
        rows.join("\n")
    );
    assert!(
        rows[3].starts_with("──"),
        "divider right under it:\n{}",
        rows.join("\n")
    );
    assert!(
        !rows[4].is_empty(),
        "composer under the divider:\n{}",
        rows.join("\n")
    );
    assert!(
        rows[5..].iter().all(String::is_empty),
        "nothing below:\n{}",
        rows.join("\n")
    );
}

/// The spinner's breathing rows vanish when the turn ends; the layout that
/// follows (the idle prompt) closes up under the output instead of leaving
/// three blank rows above the divider.
#[test]
fn the_idle_layout_after_a_turn_closes_the_spinners_rows() {
    let (mut live, handle) = capture_live(40, 12);
    live.begin_turn(&[]);
    for i in 1..=20 {
        live.print_line(&format!("line {i}"));
    }
    live.tick_spinner();
    live.finish();
    // The turn's layout is torn down (chrome erased, cursor parked under the
    // output) and the idle layout probes the cursor there.
    let region_bottom = live.region_bottom;
    handle.inject(super::terminal::teardown_sequence(region_bottom, 12).as_bytes());
    let (cursor_row, _) = screen(&handle, 40, 12, "").screen().cursor_position();
    let (mut idle, idle_handle) = capture_live_at(40, 12, cursor_row + 1);
    idle.render();
    let mut all = handle.bytes();
    all.extend(idle_handle.bytes());
    let mut parser = vt100::Parser::new(12, 40, 0);
    parser.process(&all);
    let rows = rows_text(&parser);
    let last = rows
        .iter()
        .rposition(|r| r == "line 20")
        .expect("last output line");
    assert_eq!(
        rows[last + 1],
        "",
        "one blank row under the output:\n{}",
        rows.join("\n")
    );
    assert!(
        rows[last + 2].starts_with("──"),
        "divider right under that blank row:\n{}",
        rows.join("\n")
    );
}

/// Without a probe answer (no terminal to ask) reclaiming anchors the
/// layout at the bottom as it always did — no following, no surprises.
#[test]
fn reclaim_without_a_cursor_probe_anchors_at_the_bottom() {
    let (mut live, handle) = capture_live(40, 12);
    let paused = Arc::new(AtomicBool::new(false));
    live.begin_turn(&[]);
    live.print_line("line 1");
    live.on_event(
        &AgentEvent::ApprovalPending {
            name: "shell".into(),
            command: "ls".into(),
        },
        &paused,
    );
    live.on_event(
        &AgentEvent::ApprovalResolved {
            name: "shell".into(),
            command: "ls".into(),
            decision: "approved".into(),
        },
        &paused,
    );
    let rows = rows_text(&screen(&handle, 40, 12, ""));
    assert!(
        rows[10].starts_with("──"),
        "divider on its anchored row:\n{}",
        rows.join("\n")
    );
    assert!(
        !rows[11].is_empty(),
        "composer on the bottom row:\n{}",
        rows.join("\n")
    );
}

/// Typing enough to wrap the composer onto extra rows re-carves the region
/// smaller — but must never scroll or nudge conversation rows already on
/// screen (only the first paint is allowed to lift content).
#[test]
fn composer_growth_never_nudges_the_conversation() {
    let (mut live, handle) = capture_live(30, 12);
    live.render();
    for ch in "the quick brown fox jumps over the lazy dog".chars() {
        live.handle_key(key(KeyCode::Char(ch)), 0);
        live.render();
    }
    // A conversation line with the output cursor on the blank row under it,
    // well above the rows the composer will claim as it wraps: nothing is in
    // the way, so no keystroke may scroll it (the transition only lifts the
    // conversation when the cursor row itself would be claimed — see the
    // `region_transition` tests).
    let parser = screen(&handle, 30, 12, "\x1b[3;1Hconvo line\r\n");
    let rows = rows_text(&parser);
    assert_eq!(
        rows.iter().position(|r| r == "convo line"),
        Some(2),
        "conversation must not move as the composer grows:\n{}",
        rows.join("\n")
    );
    // The composer actually wrapped: the draft spans the bottom rows.
    let tail = rows[9..].join(" ");
    assert!(
        tail.contains("lazy") || tail.contains("dog"),
        "wrapped composer rows must hold the draft:\n{}",
        rows.join("\n")
    );
}

/// The approval hand-off: pending pauses input forwarding and stops all
/// painting; resolved un-pauses, prints the decision, and a forced repaint
/// restores the complete pinned layout the picker drew over.
#[test]
fn approval_hand_off_pauses_input_and_reclaim_restores_the_layout() {
    let (mut live, handle) = capture_live(40, 12);
    let paused = Arc::new(AtomicBool::new(false));
    live.begin_turn(&[]);
    live.on_event(&token("before approval\n"), &paused);

    live.on_event(
        &AgentEvent::ApprovalPending {
            name: "shell".into(),
            command: "rm -rf ./build".into(),
        },
        &paused,
    );
    assert!(
        paused.load(Ordering::Relaxed),
        "input thread must yield keys to the picker"
    );
    // Painting while the picker owns the screen is a hard no-op.
    let before = handle.bytes().len();
    live.render();
    live.tick_spinner();
    assert_eq!(
        handle.bytes().len(),
        before,
        "no bytes may be painted while suspended"
    );

    live.on_event(
        &AgentEvent::ApprovalResolved {
            name: "shell".into(),
            command: "rm -rf ./build".into(),
            decision: "approved".into(),
        },
        &paused,
    );
    assert!(
        !paused.load(Ordering::Relaxed),
        "input forwarding must resume after the decision"
    );

    let rows = rows_text(&screen(&handle, 40, 12, ""));
    let all = rows.join("\n");
    assert!(all.contains("before approval"), "scrollback intact:\n{all}");
    assert!(
        all.contains("approved — rm -rf ./build"),
        "decision line must print after reclaim:\n{all}"
    );
    assert!(
        rows.iter().any(|r| r.starts_with("──")),
        "forced repaint must restore the divider:\n{all}"
    );
    assert!(
        !rows.last().unwrap().is_empty(),
        "forced repaint must restore the composer:\n{all}"
    );
}

/// If the turn dies while a picker owns the screen (interrupt, error), the
/// suspension's Drop must un-pause the input thread — the session can never be
/// stranded with input forwarding off.
#[test]
fn dropping_a_suspended_live_unsticks_the_input_thread() {
    let (mut live, _handle) = capture_live(40, 12);
    let paused = Arc::new(AtomicBool::new(false));
    live.begin_turn(&[]);
    live.on_event(
        &AgentEvent::ApprovalPending {
            name: "shell".into(),
            command: "make deploy".into(),
        },
        &paused,
    );
    assert!(paused.load(Ordering::Relaxed));
    drop(live);
    assert!(
        !paused.load(Ordering::Relaxed),
        "the lease's Drop must clear `paused` when the turn dies mid-approval"
    );
}

/// Ending a live session must erase every piece of its chrome — the meters,
/// compression line, divider, completion hint, and composer — leaving only
/// conversation in the scrollback, with the cursor parked just below it so
/// the next cooked-mode print (the echoed submission) continues seamlessly.
/// Guards the "stale meters stack up after /model and after every turn" bug.
#[test]
fn teardown_erases_all_chrome_from_the_screen() {
    let (mut live, handle) = capture_live(60, 20);
    live.status_lines = vec!["compass context meter".into(), "usage meter line".into()];
    live.compression_line = Some("compression savings line".into());
    live.render();
    // An idle composer with the completion hint showing (the worst case: the
    // hint rows sit above the box and used to leak on submit).
    live.handle_key(key(KeyCode::Char('/')), 0);
    live.render();
    let bottom = live.region_bottom;

    let mut parser = screen(&handle, 60, 20, "\x1b[12;1Hlast reply line\r\n");
    let before = rows_text(&parser).join("\n");
    assert!(
        before.contains("usage meter line") && before.contains("compression savings line"),
        "chrome must be on screen before teardown:\n{before}"
    );

    parser.process(super::terminal::teardown_sequence(bottom, 20).as_bytes());
    let rows = rows_text(&parser);
    let all = rows.join("\n");
    assert!(
        all.contains("last reply line"),
        "conversation must survive teardown:\n{all}"
    );
    for chrome in [
        "compression savings line",
        "compass context meter",
        "usage meter line",
        "──",     // the divider rule
        "/model", // the completion hint's rows
        "❯",      // the composer prompt / hint pointers
    ] {
        assert!(
            !all.contains(chrome),
            "chrome must not survive into the scrollback ({chrome:?}):\n{all}"
        );
    }
    // The cursor stays right below the conversation (the reply's newline left
    // it there; the tall completion hint had scrolled both up to fit inside
    // the region) — never dragged down to the freed rows — so the next cooked
    // print continues without a void.
    let reply_row = rows
        .iter()
        .position(|r| r.contains("last reply line"))
        .expect("reply row") as u16;
    assert_eq!(
        parser.screen().cursor_position(),
        (reply_row + 1, 0),
        "cursor must stay just below the conversation:\n{all}"
    );
    assert!(
        reply_row + 1 < bottom,
        "the conversation must have been kept inside the region ({bottom}):\n{all}"
    );
}

/// The first paint claims the pinned rows without painting over conversation
/// output already on screen (the banner tail): the region scrolls up by the
/// claimed rows first.
#[test]
fn first_paint_preserves_the_banner_tail() {
    let (mut live, handle) = capture_live(40, 10);
    live.render();
    // Banner occupying the bottom rows of the initial (full-height) region.
    let prelude = "\x1b[8;1Hwelcome to the trail\r\nbanner tail";
    let parser = screen(&handle, 40, 10, prelude);
    let rows = rows_text(&parser);
    let all = rows.join("\n");
    assert!(
        all.contains("welcome to the trail") && all.contains("banner tail"),
        "banner must survive the first paint:\n{all}"
    );
    // The divider rule sits above the composer, in the reserved area.
    assert!(
        rows.iter().any(|r| r.starts_with("──")),
        "divider must be painted:\n{all}"
    );
    // The bottom row holds the composer prompt (non-empty).
    assert!(
        !rows.last().unwrap().is_empty(),
        "composer row must be painted:\n{all}"
    );
}

/// While a turn runs the meter line leads with a braille spinner and a
/// whole-second timer, so "how long has this been going" is readable at a
/// glance; at idle the line is just the meters again.
#[test]
fn the_meter_line_carries_the_turn_spinner_and_timer() {
    let (mut live, handle) = capture_live(60, 12);
    live.status_lines = vec![
        "  🧭 context 10.0k / 200.0k tokens (5%)".into(),
        "  📊 10.0k tokens used".into(),
    ];
    live.begin_elapsed();
    live.render();

    let rows = rows_text(&screen(&handle, 60, 12, ""));
    let meter = rows
        .iter()
        .find(|r| r.contains("context 10.0k"))
        .unwrap_or_else(|| panic!("meter line missing:\n{}", rows.join("\n")));
    assert!(
        meter.contains("0s") && meter.chars().any(|c| ('⠀'..='⣿').contains(&c)),
        "meter must lead with a braille spinner and a timer: {meter:?}"
    );
    // The second meter line is left alone — one clock is enough.
    let totals = rows
        .iter()
        .find(|r| r.contains("tokens used"))
        .expect("the totals line must be painted");
    assert!(!totals.contains("0s"), "only one clock: {totals:?}");

    // Once the turn ends the clock disappears rather than freezing on screen.
    live.end_elapsed();
    live.render();
    let rows = rows_text(&screen(&handle, 60, 12, ""));
    let meter = rows.iter().find(|r| r.contains("context 10.0k")).unwrap();
    assert!(
        !meter.contains("0s") && !meter.chars().any(|c| ('⠀'..='⣿').contains(&c)),
        "the idle meter must be the meter alone: {meter:?}"
    );
}

/// The terminal title tracks the session state (so a backgrounded tab says
/// what it's doing), and the bell rings only where it's wanted: when a prompt
/// starts waiting on the user — never as a side effect of re-titling.
#[test]
fn the_title_tracks_the_session_and_a_waiting_prompt_rings() {
    let (mut live, handle) = capture_live(40, 12);
    let paused = Arc::new(AtomicBool::new(false));

    // Idle: the ox is waiting for you.
    live.end_elapsed();
    let parser = screen(&handle, 40, 12, "");
    assert!(
        parser.screen().title().starts_with("🐂 > "),
        "idle title: {:?}",
        parser.screen().title()
    );
    // Re-titling must not ring: OSC 0's BEL terminator is a string terminator.
    assert_eq!(parser.screen().audible_bell_count(), 0, "titles are silent");

    // Working: the spinner frame rides in the title too.
    live.begin_turn(&[]);
    live.begin_elapsed();
    let parser = screen(&handle, 40, 12, "");
    let title = parser.screen().title().to_string();
    assert!(title.starts_with("🐂 "), "working title: {title:?}");
    assert!(
        !title.starts_with("🐂 > ") && !title.starts_with("🐂 ! "),
        "working title must carry the spinner: {title:?}"
    );

    // Waiting on an approval: the title flags it and the bell rings once.
    live.on_event(
        &AgentEvent::ApprovalPending {
            name: "shell".into(),
            command: "rm -rf ./build".into(),
        },
        &paused,
    );
    let parser = screen(&handle, 40, 12, "");
    assert!(
        parser.screen().title().starts_with("🐂 ! "),
        "waiting title: {:?}",
        parser.screen().title()
    );
    assert_eq!(
        parser.screen().audible_bell_count(),
        1,
        "a waiting prompt must ring exactly once"
    );
}
