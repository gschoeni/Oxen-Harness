//! The terminal plumbing behind the live input box: an RAII guard that owns raw
//! mode and the scroll region, a `\n`→`\r\n` writer for raw mode, the escape
//! sequences that hand the screen to (and reclaim it from) an interactive tool,
//! and the background thread that forwards key/resize/paste events.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::{execute, terminal};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// Owns the terminal for the lifetime of a live session: raw mode, a hidden
/// cursor, and a scroll region over every row but the last. Drop restores
/// everything, no matter how the turn ended.
pub(super) struct LiveTerminal {
    pub(super) cols: u16,
    pub(super) rows: u16,
    /// Whether we pushed keyboard-enhancement flags (so Drop pops them).
    kbd_enhanced: bool,
}

impl LiveTerminal {
    pub(super) fn new() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        // Ask the terminal to disambiguate modified keys (the kitty keyboard
        // protocol) so Shift+Enter is reported distinctly from Enter. Harmless
        // and skipped where unsupported — Alt+Enter / Ctrl-J still add a newline.
        let kbd_enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
        if kbd_enhanced {
            let _ = execute!(
                io::stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
        }
        let mut out = io::stdout();
        let bottom = region_bottom(rows);
        // Hide the cursor, enable bracketed paste (so a drag-dropped path arrives
        // as one atomic `Event::Paste` instead of a fragile burst of keystrokes),
        // carve a scroll region over rows 1..=H-1, and park the output cursor at
        // the bottom of that region so output scrolls upward.
        let _ = write!(out, "\x1b[?25l\x1b[?2004h\x1b[1;{bottom}r\x1b[{bottom};1H");
        let _ = out.flush();
        Ok(Self {
            cols,
            rows,
            kbd_enhanced,
        })
    }
}

impl Drop for LiveTerminal {
    fn drop(&mut self) {
        if self.kbd_enhanced {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let mut out = io::stdout();
        // Disable bracketed paste, reset the scroll region, clear the composer
        // row, show the cursor, and drop to a fresh line for the next cooked-mode
        // prompt.
        let _ = write!(
            out,
            "\x1b[?2004l\x1b[r\x1b[{};1H\x1b[2K\x1b[?25h\r\n",
            self.rows
        );
        let _ = out.flush();
        let _ = terminal::disable_raw_mode();
    }
}

/// The last row usable for scrolling output (the composer sits below it).
pub(super) fn region_bottom(rows: u16) -> u16 {
    rows.saturating_sub(1).max(1)
}

/// A `Write` adapter that rewrites bare `\n` as `\r\n`, which raw mode requires
/// to avoid stair-stepped output. `MarkdownStream` writes through this.
pub(super) struct CrlfWriter {
    out: io::Stdout,
}

impl CrlfWriter {
    pub(super) fn new() -> Self {
        Self { out: io::stdout() }
    }
}

impl Write for CrlfWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut handle = self.out.lock();
        let mut last = 0;
        for (i, &b) in buf.iter().enumerate() {
            if b == b'\n' && (i == 0 || buf[i - 1] != b'\r') {
                handle.write_all(&buf[last..i])?;
                handle.write_all(b"\r\n")?;
                last = i + 1;
            }
        }
        handle.write_all(&buf[last..])?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// Escape sequence that hands a clean screen to an interactive tool: reset the
/// scroll region, then **clear the whole reserved input area** (every row below
/// `region_bottom` — the box, divider, spacer, and queue, not just one row) so
/// no stale input UI lingers above the tool's output. The cursor is parked just
/// below the conversation (and shown) so the tool draws in natural reading order.
pub(super) fn suspend_sequence(region_bottom: u16, rows: u16) -> String {
    // `\x1b[r` resets (and may home) the cursor, so we reposition explicitly.
    let mut seq = String::from("\x1b[r");
    for r in region_bottom.saturating_add(1)..=rows {
        seq.push_str(&format!("\x1b[{r};1H\x1b[2K"));
    }
    // Park on the first freed row, right after the conversation, cursor shown.
    let start = region_bottom.saturating_add(1).min(rows);
    seq.push_str(&format!("\x1b[{start};1H\x1b[?25h"));
    seq
}

/// Escape sequence that re-establishes the live layout after an interactive tool
/// finishes: hide the cursor, re-carve the scroll region, and park the output
/// cursor at the bottom of it.
pub(super) fn resume_sequence(rows: u16) -> String {
    let bottom = region_bottom(rows);
    format!("\x1b[?25l\x1b[1;{bottom}r\x1b[{bottom};1H")
}

/// Spawn a thread that polls for terminal events and forwards them. It never
/// writes to the terminal. `stop` ends it; while `paused`, it yields the event
/// stream (so an interactive tool can read keys itself).
pub(super) fn spawn_input(
    stop: &Arc<AtomicBool>,
    paused: &Arc<AtomicBool>,
) -> (UnboundedReceiver<Event>, JoinHandle<()>) {
    let (tx, rx): (UnboundedSender<Event>, UnboundedReceiver<Event>) = mpsc::unbounded_channel();
    let stop = stop.clone();
    let paused = paused.clone();
    let handle = thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if paused.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(ev @ Event::Key(_))
                    | Ok(ev @ Event::Resize(_, _))
                    | Ok(ev @ Event::Paste(_)) => {
                        if tx.send(ev).is_err() {
                            break;
                        }
                    }
                    _ => {}
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
    (rx, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspend_sequence_clears_the_whole_reserved_area_for_the_picker() {
        // region_bottom 20, rows 24 → the input area is rows 21..=24.
        let seq = suspend_sequence(20, 24);
        // The scroll region must be reset before anything else so the picker
        // isn't confined to the old region.
        assert!(
            seq.starts_with("\x1b[r"),
            "region reset must come first: {seq:?}"
        );
        // Every reserved row below region_bottom is cleared (not just one), so a
        // stale multi-row input box can't linger above the tool's output.
        for r in 21..=24 {
            assert!(
                seq.contains(&format!("\x1b[{r};1H\x1b[2K")),
                "must clear reserved row {r}: {seq:?}"
            );
        }
        // The cursor is shown, parked just below the conversation.
        assert!(seq.contains("\x1b[?25h"), "must show the cursor: {seq:?}");
        assert!(
            seq.contains("\x1b[21;1H"),
            "must park on the first freed row: {seq:?}"
        );
        assert!(seq.find("\x1b[r").unwrap() < seq.find("\x1b[24;1H").unwrap());
    }

    #[test]
    fn resume_sequence_reestablishes_the_live_layout() {
        let seq = resume_sequence(24);
        assert!(
            seq.contains("\x1b[?25l"),
            "must hide the cursor again: {seq:?}"
        );
        // Region carved over every row but the composer row (24 -> bottom 23).
        assert!(
            seq.contains("\x1b[1;23r"),
            "must re-carve the scroll region: {seq:?}"
        );
        assert!(
            seq.contains("\x1b[23;1H"),
            "must park at the region bottom: {seq:?}"
        );
    }

    #[test]
    fn suspend_sequence_handles_a_tiny_terminal() {
        // A 1-row terminal must not underflow when computing the region bottom.
        let _ = suspend_sequence(0, 1);
        assert!(resume_sequence(1).contains("\x1b[1;1r"));
    }
}
