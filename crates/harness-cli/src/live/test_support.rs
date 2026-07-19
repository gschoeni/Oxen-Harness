//! Shared constructors for the live-composer unit tests (compiled only under
//! `cfg(test)`): headless [`Live`] instances (no TTY is touched — the tests
//! drive `handle_key` and inspect state, never paint), key-event builders, and
//! the vt100 screen harness for golden paint tests (a capturing [`Live`] whose
//! painted bytes are replayed through a terminal emulator and asserted on as a
//! screen grid).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::theme::Ui;

use super::sink::{CaptureHandle, Sink};
use super::terminal::region_bottom;
use super::Live;

/// A [`Live`] with color enabled, for behavior tests.
pub(super) fn live(cols: u16, rows: u16) -> Live {
    Live::new(
        Ui::with(true, std::sync::Arc::new(harness_theme::Theme::default())),
        cols,
        rows,
    )
}

/// A [`Live`] with color disabled, for width/alignment assertions where escape
/// codes would get in the way.
pub(super) fn plain_live(cols: u16, rows: u16) -> Live {
    Live::new(
        Ui::with(false, std::sync::Arc::new(harness_theme::Theme::default())),
        cols,
        rows,
    )
}

/// A [`Live`] writing into a capture buffer, for golden screen tests. Color
/// (and with it the spinner) is on — the vt100 emulator absorbs SGR sequences
/// into cell attributes, so grid text assertions still read plain characters.
pub(super) fn capture_live(cols: u16, rows: u16) -> (Live, CaptureHandle) {
    let (sink, handle) = Sink::capture();
    let live = Live::with_sink(
        Ui::with(true, std::sync::Arc::new(harness_theme::Theme::default())),
        cols,
        rows,
        sink,
    );
    (live, handle)
}

/// Replay everything the captured [`Live`] painted through a terminal emulator,
/// preceded by the same setup `LiveTerminal::new` performs (carve the scroll
/// region over rows `1..=H-1`, park the cursor at its bottom), and return the
/// resulting screen. `prelude` is written before the setup — use it to seed
/// banner-style scrollback that predates live mode.
pub(super) fn screen(handle: &CaptureHandle, cols: u16, rows: u16, prelude: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(prelude.as_bytes());
    let bottom = region_bottom(rows);
    parser.process(format!("\x1b[1;{bottom}r\x1b[{bottom};1H").as_bytes());
    parser.process(&handle.bytes());
    parser
}

/// The visible text of every screen row, right-trimmed.
pub(super) fn rows_text(parser: &vt100::Parser) -> Vec<String> {
    let (rows, cols) = parser.screen().size();
    (0..rows)
        .map(|r| {
            let mut line: String = (0..cols)
                .map(|c| {
                    parser
                        .screen()
                        .cell(r, c)
                        .map(|cell| {
                            let s = cell.contents();
                            if s.is_empty() {
                                ' '.to_string()
                            } else {
                                s
                            }
                        })
                        .unwrap_or_else(|| ' '.to_string())
                })
                .collect();
            line.truncate(line.trim_end().len());
            line
        })
        .collect()
}

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

pub(super) fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}
