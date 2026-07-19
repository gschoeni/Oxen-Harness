//! The screen hand-off to an interactive tool (the picker), as a single owned
//! value instead of hand-flipped flags.
//!
//! While an approval or ask-user picker runs, the composer must stop drawing
//! *and* the input thread must stop stealing keys — two facts that used to
//! live in separate places (`Live.suspended` and the `paused` atomic) and had
//! to be toggled together by hand at every hand-off. A [`ScreenSuspension`]
//! owns both: creating it pauses input and tears the live layout down;
//! reclaiming it restores everything. If the turn dies while a picker is up
//! (interrupt, error), Drop restores the input thread and the terminal modes
//! anyway — the session can never be stranded in cooked mode with input
//! paused.
//!
//! The hand-off spans two separate agent events (`ApprovalPending` →
//! `ApprovalResolved`, or the ask-user tool's start → end) with the picker
//! running on a blocking thread in between, so this is a *stored* lease on
//! [`Live`], not a scoped guard. The agent emits the pending event before the
//! picker reads keys, so input is always paused first.
//!
//! [`Live`]: super::Live

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::sink::Sink;
use super::terminal::{resume_sequence, suspend_sequence};

pub(super) struct ScreenSuspension {
    paused: Arc<AtomicBool>,
    out: Sink,
    rows: u16,
    reclaimed: bool,
}

impl ScreenSuspension {
    /// Hand the screen over: pause input forwarding, reset the scroll region
    /// and clear the whole reserved input area, show the cursor, and leave raw
    /// mode — so the tool draws into a clean cooked-mode screen.
    pub(super) fn begin(
        out: Sink,
        paused: &Arc<AtomicBool>,
        region_bottom: u16,
        rows: u16,
    ) -> Self {
        paused.store(true, Ordering::Relaxed);
        let mut w = out.clone();
        let _ = write!(w, "{}", suspend_sequence(region_bottom, rows));
        let _ = w.flush();
        let _ = crossterm::terminal::disable_raw_mode();
        Self {
            paused: paused.clone(),
            out,
            rows,
            reclaimed: false,
        }
    }

    /// Reclaim the screen after the tool finishes: re-enter raw mode, resume
    /// input forwarding, and re-carve a composer-only region (the caller
    /// follows with a forced full repaint to restore the real layout).
    pub(super) fn reclaim(mut self) {
        self.reclaimed = true;
        self.restore();
    }

    fn restore(&mut self) {
        let _ = crossterm::terminal::enable_raw_mode();
        self.paused.store(false, Ordering::Relaxed);
        let mut w = self.out.clone();
        let _ = write!(w, "{}", resume_sequence(self.rows));
        let _ = w.flush();
    }
}

impl Drop for ScreenSuspension {
    fn drop(&mut self) {
        // Safety net for a turn that ends while a picker is still up: restore
        // the terminal and un-pause the input thread so the session continues
        // in a sane state. (`LiveTerminal`'s own Drop runs after and settles
        // the final cooked-mode screen; both sequences are idempotent.)
        if !self.reclaimed {
            self.restore();
        }
    }
}
