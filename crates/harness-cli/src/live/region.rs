//! Writing into the scroll region with a tracked, transient tail (the spinner).
//!
//! The spinner lives *below* the streamed output and must move down as new
//! content arrives. Historically that was done with raw `\r\x1b[K` writes
//! scattered across the event handlers, whose correctness depended on calling
//! them in exactly the right order. Here the tail is state: the layout lifts
//! it ([`RegionWriter::lift_tail`]), writes content where it sat
//! ([`RegionWriter::write_content`]), and redraws it below
//! ([`RegionWriter::redraw_tail`]) — `Live::region_write` encloses the whole
//! dance so no caller can interleave them wrongly.
//!
//! The tail is padded: one blank row above the spinner and one below, so it
//! breathes instead of butting against the last tool line and the divider.
//! Drawn from the output cursor's (blank) row `R` it occupies `R..=R+2` and
//! leaves the cursor on `R+2`; lifting erases all three rows and returns the
//! cursor to `R`, where content belongs. Every sequence is cursor-relative,
//! so it holds whether or not the draw scrolled the region. A region too
//! short for three rows falls back to a bare one-row tail
//! ([`RegionWriter::set_padded`]).
//!
//! While an interactive tool owns the screen the writer is muted: tail
//! changes update state but paint nothing, and the reclaim repaint restores
//! the picture.

use std::io::Write;

use super::advance::Advance;
use super::sink::Sink;
use super::terminal::CrlfWriter;

/// Rows a padded tail takes below the output cursor's own row (the blank
/// row above the spinner is the cursor row itself; then the spinner, then
/// the blank row under it).
pub(super) const PADDED_TAIL_EXTRA: u16 = 2;

pub(super) struct RegionWriter {
    out: Sink,
    /// Row counter shared with every content writer (see [`Advance`]).
    advance: Advance,
    /// The transient line at the region cursor (the spinner), if any.
    tail: Option<String>,
    /// Whether to draw the tail with a blank row above and below.
    padded: bool,
    /// Rows the tail currently occupies on screen (0 when lifted or absent).
    shown: u16,
    /// Set while an interactive tool owns the screen: state updates, no paint.
    muted: bool,
}

impl RegionWriter {
    pub(super) fn new(out: Sink, advance: Advance) -> Self {
        Self {
            out,
            advance,
            tail: None,
            padded: true,
            shown: 0,
            muted: false,
        }
    }

    pub(super) fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Whether the tail gets its breathing rows. Off only when the region is
    /// too short to hold three rows without scrolling the spinner itself
    /// into the scrollback.
    pub(super) fn set_padded(&mut self, padded: bool) {
        self.padded = padded;
    }

    /// Rows below the output cursor's row a redraw of the tail will claim.
    pub(super) fn tail_extra(&self) -> u16 {
        if self.padded {
            PADDED_TAIL_EXTRA
        } else {
            0
        }
    }

    pub(super) fn has_tail(&self) -> bool {
        self.tail.is_some()
    }

    /// A content writer for the region that reports what it emits to the
    /// row tracker (the streamed-Markdown renderer writes through one).
    pub(super) fn crlf(&self) -> CrlfWriter {
        CrlfWriter::tracked(self.out.clone(), self.advance.clone())
    }

    /// Replace the tail line: redraw in place when `Some`, erase when `None`.
    pub(super) fn set_tail(&mut self, line: Option<String>) {
        if !self.muted {
            match &line {
                Some(l) => {
                    if self.shown == self.rows_to_draw() {
                        // Same footprint as what's on screen: overwrite the
                        // spinner row alone. Padded, the cursor rests on the
                        // row below it; step up, draw, step back down.
                        let seq = if self.padded {
                            format!("\x1b[A\r{l}\x1b[K\x1b[B\r")
                        } else {
                            format!("\r{l}\x1b[K")
                        };
                        let _ = write!(self.out, "{seq}");
                    } else {
                        self.erase();
                        self.draw(l);
                    }
                    let _ = self.out.flush();
                }
                None => {
                    self.erase();
                    let _ = self.out.flush();
                }
            }
        }
        self.tail = line;
    }

    /// Erase the tail from screen without dropping it, so content can be
    /// written where it sat. Pair with [`RegionWriter::redraw_tail`]; prefer
    /// `Live::region_write`, which encloses the whole dance.
    pub(super) fn lift_tail(&mut self) {
        if !self.muted {
            self.erase();
            let _ = self.out.flush();
        }
    }

    /// Re-emit the current tail at the region cursor (right below whatever
    /// was just written).
    pub(super) fn redraw_tail(&mut self) {
        if self.muted {
            return;
        }
        if let Some(l) = self.tail.clone() {
            self.draw(&l);
            let _ = self.out.flush();
        }
    }

    /// Write content into the region (newlines become `\r\n`) at the cursor,
    /// counting the rows it takes. The tail is *not* handled here — lift it
    /// first and redraw it after, or go through `Live::region_write`.
    pub(super) fn write_content(&mut self, text: &str) {
        let mut w = self.crlf();
        let _ = w.write_all(text.as_bytes());
        let _ = w.flush();
    }

    fn rows_to_draw(&self) -> u16 {
        1 + self.tail_extra()
    }

    /// Paint the tail from the cursor row, leaving the cursor on its last row.
    fn draw(&mut self, line: &str) {
        let seq = if self.padded {
            format!("\r\x1b[K\r\n{line}\x1b[K\r\n\x1b[K")
        } else {
            format!("\r{line}\x1b[K")
        };
        let _ = write!(self.out, "{seq}");
        self.shown = self.rows_to_draw();
    }

    /// Clear whatever the tail occupies and put the cursor back on its first
    /// row — the row content belongs on.
    fn erase(&mut self) {
        match self.shown {
            0 => {}
            1 => {
                let _ = write!(self.out, "\r\x1b[K");
            }
            n => {
                let mut seq = String::from("\r\x1b[K");
                for _ in 1..n {
                    seq.push_str("\x1b[A\x1b[K");
                }
                let _ = write!(self.out, "{seq}");
            }
        }
        self.shown = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::super::sink::Sink;
    use super::*;

    fn writer() -> (RegionWriter, super::super::sink::CaptureHandle) {
        let (sink, handle) = Sink::capture();
        (RegionWriter::new(sink, Advance::new(80)), handle)
    }

    fn text(handle: &super::super::sink::CaptureHandle) -> String {
        String::from_utf8(handle.bytes()).unwrap()
    }

    #[test]
    fn a_padded_tail_draws_three_rows_and_lifts_all_of_them() {
        let (mut w, h) = writer();
        w.set_tail(Some("spin".into()));
        assert_eq!(text(&h), "\r\x1b[K\r\nspin\x1b[K\r\n\x1b[K");
        w.lift_tail();
        assert!(
            text(&h).ends_with("\r\x1b[K\x1b[A\x1b[K\x1b[A\x1b[K"),
            "lift must erase all three rows and climb back: {:?}",
            text(&h)
        );
        assert!(w.has_tail(), "lifting keeps the tail for the redraw");
    }

    #[test]
    fn a_padded_tail_redraws_in_place_over_the_spinner_row_only() {
        let (mut w, h) = writer();
        w.set_tail(Some("one".into()));
        let before = h.bytes().len();
        w.set_tail(Some("two".into()));
        let tail = String::from_utf8(h.bytes()[before..].to_vec()).unwrap();
        assert_eq!(tail, "\x1b[A\rtwo\x1b[K\x1b[B\r");
    }

    #[test]
    fn a_bare_tail_is_the_single_row_it_always_was() {
        let (mut w, h) = writer();
        w.set_padded(false);
        w.set_tail(Some("spin".into()));
        assert_eq!(text(&h), "\rspin\x1b[K");
        w.set_tail(None);
        assert!(text(&h).ends_with("\r\x1b[K"));
        assert!(!w.has_tail());
    }

    #[test]
    fn muted_tail_changes_paint_nothing_and_redraw_after_unmute_is_a_full_draw() {
        let (mut w, h) = writer();
        w.set_muted(true);
        w.set_tail(Some("spin".into()));
        assert!(h.bytes().is_empty());
        w.set_muted(false);
        w.set_tail(Some("spin".into()));
        assert_eq!(text(&h), "\r\x1b[K\r\nspin\x1b[K\r\n\x1b[K");
    }

    #[test]
    fn content_writes_report_their_rows() {
        let (mut w, _h) = writer();
        w.write_content("one\ntwo\n");
        assert_eq!(w.advance.take_rows(), 2);
    }
}
