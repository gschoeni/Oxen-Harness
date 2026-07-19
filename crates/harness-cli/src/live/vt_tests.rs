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

use super::test_support::{capture_live, key, rows_text, screen};

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
/// lines stack in the region, the spinner rides exactly one row below the
/// last content line, and `finish()` leaves no spinner remnant behind.
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
    // row directly below the last content line.
    let rows = rows_text(&screen(&handle, 40, 12, ""));
    let alpha = rows.iter().position(|r| r == "alpha line").expect("alpha");
    assert_eq!(rows[alpha + 1], "beta line", "content stacks in order");
    assert!(
        !rows[alpha + 2].is_empty(),
        "spinner must ride one row below the output:\n{}",
        rows.join("\n")
    );

    live.finish();
    let rows = rows_text(&screen(&handle, 40, 12, ""));
    let alpha = rows.iter().position(|r| r == "alpha line").expect("alpha");
    assert_eq!(rows[alpha + 1], "beta line");
    assert_eq!(
        rows[alpha + 2],
        "",
        "finish must leave no spinner remnant:\n{}",
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
    assert!(
        !rows[at + 1].is_empty(),
        "spinner must be redrawn below the announcement:\n{}",
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
    // A conversation line written in the rows the first paint claims: it is
    // lifted once (first paint), then must hold still as the composer wraps.
    // (It sits above the region-bottom cursor row — that row is always a fresh
    // blank line in practice, and growth is allowed to claim it.)
    let parser = screen(&handle, 30, 12, "\x1b[8;1Hconvo line");
    let rows = rows_text(&parser);
    // Initial region bottom 11 → first-paint bottom 9 lifts it two rows to 6
    // (index 5); the later composer growth re-carves without scrolling.
    assert_eq!(
        rows.iter().position(|r| r == "convo line"),
        Some(5),
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

    let mut parser = screen(&handle, 60, 20, "\x1b[12;1Hlast reply line");
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
    // The cursor is parked on the first freed row, right below the
    // conversation, so the next cooked print continues without a void.
    assert_eq!(
        parser.screen().cursor_position().0,
        bottom,
        "cursor must park just below the conversation:\n{all}"
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
