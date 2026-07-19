//! Writing into the scroll region with a tracked, transient tail line.
//!
//! The spinner lives on the row at the region cursor — *below* the streamed
//! output — and must move down as new content arrives. Historically that was
//! done with raw `\r\x1b[K` writes scattered across the event handlers, whose
//! correctness depended on calling them in exactly the right order. Here the
//! tail is state: [`RegionWriter::write`] lifts it, writes the content where
//! it sat, and redraws it below — so no caller can interleave them wrongly.
//!
//! While an interactive tool owns the screen the writer is muted: tail
//! changes update state but paint nothing, and the reclaim repaint restores
//! the picture.

use std::io::Write;

use super::sink::Sink;
use super::terminal::CrlfWriter;

pub(super) struct RegionWriter {
    out: Sink,
    /// The transient line currently at the region cursor (the spinner), if any.
    tail: Option<String>,
    /// Set while an interactive tool owns the screen: state updates, no paint.
    muted: bool,
}

impl RegionWriter {
    pub(super) fn new(out: Sink) -> Self {
        Self {
            out,
            tail: None,
            muted: false,
        }
    }

    pub(super) fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Replace the tail line: redraw in place when `Some`, erase when `None`.
    pub(super) fn set_tail(&mut self, line: Option<String>) {
        if !self.muted {
            match &line {
                Some(l) => {
                    let _ = write!(self.out, "\r{l}\x1b[K");
                    let _ = self.out.flush();
                }
                None => {
                    if self.tail.is_some() {
                        let _ = write!(self.out, "\r\x1b[K");
                        let _ = self.out.flush();
                    }
                }
            }
        }
        self.tail = line;
    }

    /// Erase the tail from screen without dropping it, so content can be
    /// written where it sat. Pair with [`RegionWriter::redraw_tail`]; prefer
    /// [`RegionWriter::write`], which encloses the whole dance.
    pub(super) fn lift_tail(&mut self) {
        if !self.muted && self.tail.is_some() {
            let _ = write!(self.out, "\r\x1b[K");
            let _ = self.out.flush();
        }
    }

    /// Re-emit the current tail at the region cursor (one row below whatever
    /// was just written).
    pub(super) fn redraw_tail(&mut self) {
        if self.muted {
            return;
        }
        if let Some(l) = &self.tail {
            let _ = write!(self.out, "\r{l}\x1b[K");
            let _ = self.out.flush();
        }
    }

    /// Write content into the region (newlines become `\r\n`): the tail is
    /// lifted first and redrawn after, so the content lands where the tail sat
    /// and the tail follows below it. The lift → write → redraw dance is held
    /// as one synchronized frame (mode 2026) so the spinner never visibly
    /// blinks out between its lift and its redraw.
    pub(super) fn write(&mut self, text: &str) {
        let synced = !self.muted && self.tail.is_some();
        if synced {
            let _ = write!(self.out, "{}", crate::ansi::SYNC_BEGIN);
        }
        self.lift_tail();
        let mut w = CrlfWriter::over(self.out.clone());
        let _ = w.write_all(text.as_bytes());
        let _ = w.flush();
        self.redraw_tail();
        if synced {
            let _ = write!(self.out, "{}", crate::ansi::SYNC_END);
            let _ = self.out.flush();
        }
    }
}
