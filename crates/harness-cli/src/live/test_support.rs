//! Shared constructors for the live-composer unit tests (compiled only under
//! `cfg(test)`): headless [`Live`] instances (no TTY is touched — the tests
//! drive `handle_key` and inspect state, never paint) and key-event builders.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::theme::Ui;

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

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

pub(super) fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}
