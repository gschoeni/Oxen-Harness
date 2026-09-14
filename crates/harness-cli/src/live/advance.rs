//! Estimating how far the output cursor has moved down the scroll region.
//!
//! When the pinned area follows the conversation (see `Live::follow` in the
//! module root) the layout needs to know where the output cursor is without
//! asking the terminal on every write — a cursor-position query is a round
//! trip through the input stream and can't be made hundreds of times a
//! second while tokens stream. So the region writers report every byte they
//! emit here, and the tracker counts the rows those bytes move the cursor
//! down by: one per line feed, plus one for every time a line runs past the
//! terminal width and soft-wraps.
//!
//! Escape sequences (SGR styling, the image-placement payloads) paint no
//! cells, so they are skipped; the skip state survives across writes because
//! a formatted write can split a sequence between its fragments.
//!
//! The estimate is exactly that: an estimate. Where it drifts, the layout
//! merely grows the region a row late or early — the terminal's own scroll
//! region clamps the cursor, so the pinned area is never painted over.

use std::sync::{Arc, Mutex};

use crate::width::char_width;

/// A shared, cloneable row counter — every writer into the region holds one
/// handle, the layout drains it.
#[derive(Clone)]
pub(super) struct Advance(Arc<Mutex<Tracker>>);

struct Tracker {
    /// Terminal width, for soft-wrap accounting.
    cols: usize,
    /// The cursor's column estimate within the current line.
    col: usize,
    /// Rows the cursor moved down since the last [`Advance::take_rows`].
    rows: usize,
    /// Where we are inside an escape sequence, if any, across writes.
    esc: Esc,
    /// A UTF-8 sequence cut off by a write boundary, waiting for its tail.
    partial: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Esc {
    None,
    /// Saw `ESC`; the next byte says what kind of sequence follows.
    Escape,
    /// Inside `ESC [ … <final>` (SGR, cursor moves, clears).
    Csi,
    /// Inside a string sequence (OSC / APC / DCS / PM) until `BEL` or `ESC \`.
    Str,
    /// Saw `ESC` inside a string sequence — `\` ends it, anything else doesn't.
    StrEscape,
}

impl Advance {
    pub(super) fn new(cols: u16) -> Self {
        Self(Arc::new(Mutex::new(Tracker {
            cols: cols.max(1) as usize,
            col: 0,
            rows: 0,
            esc: Esc::None,
            partial: Vec::new(),
        })))
    }

    /// The terminal was resized: wrap at the new width from here on.
    pub(super) fn set_cols(&self, cols: u16) {
        let mut t = self.0.lock().unwrap();
        t.cols = cols.max(1) as usize;
    }

    /// Account for `bytes` just written into the region.
    pub(super) fn note(&self, bytes: &[u8]) {
        let mut t = self.0.lock().unwrap();
        t.note(bytes);
    }

    /// Rows the cursor has moved down since the last call (and reset).
    pub(super) fn take_rows(&self) -> usize {
        let mut t = self.0.lock().unwrap();
        std::mem::take(&mut t.rows)
    }

    /// Rows accumulated since the last [`Advance::take_rows`], left in place.
    pub(super) fn pending_rows(&self) -> usize {
        self.0.lock().unwrap().rows
    }
}

impl Tracker {
    fn note(&mut self, bytes: &[u8]) {
        let buf: Vec<u8> = if self.partial.is_empty() {
            bytes.to_vec()
        } else {
            let mut joined = std::mem::take(&mut self.partial);
            joined.extend_from_slice(bytes);
            joined
        };
        let mut rest = buf.as_slice();
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    self.walk(s);
                    break;
                }
                Err(e) => {
                    let (valid, tail) = rest.split_at(e.valid_up_to());
                    // SAFETY-free: `valid` is valid UTF-8 by construction.
                    self.walk(std::str::from_utf8(valid).unwrap_or(""));
                    match e.error_len() {
                        // Cut off mid-sequence: keep the tail for the next write.
                        None => {
                            self.partial = tail.to_vec();
                            break;
                        }
                        // Garbage byte(s): skip them and carry on.
                        Some(n) => rest = &tail[n..],
                    }
                }
            }
        }
    }

    fn walk(&mut self, s: &str) {
        for c in s.chars() {
            match self.esc {
                Esc::Escape => {
                    self.esc = match c {
                        '[' => Esc::Csi,
                        ']' | '_' | 'P' | '^' => Esc::Str,
                        _ => Esc::None,
                    };
                    continue;
                }
                Esc::Csi => {
                    if ('\x40'..='\x7e').contains(&c) {
                        self.esc = Esc::None;
                    }
                    continue;
                }
                Esc::Str => {
                    if c == '\x07' {
                        self.esc = Esc::None;
                    } else if c == '\x1b' {
                        self.esc = Esc::StrEscape;
                    }
                    continue;
                }
                Esc::StrEscape => {
                    self.esc = if c == '\\' { Esc::None } else { Esc::Str };
                    continue;
                }
                Esc::None => {}
            }
            match c {
                '\x1b' => self.esc = Esc::Escape,
                '\n' => {
                    self.rows += 1;
                    self.col = 0;
                }
                '\r' => self.col = 0,
                // Tabs advance to the next stop but never wrap on their own.
                '\t' => self.col = ((self.col / 8 + 1) * 8).min(self.cols - 1),
                c if c.is_control() => {}
                c => {
                    let w = char_width(c);
                    if w == 0 {
                        continue;
                    }
                    if self.col + w > self.cols {
                        self.rows += 1;
                        self.col = 0;
                    }
                    self.col += w;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_line_feeds() {
        let a = Advance::new(80);
        a.note(b"one\r\ntwo\r\n");
        assert_eq!(a.take_rows(), 2);
        assert_eq!(a.take_rows(), 0, "take resets");
    }

    #[test]
    fn counts_soft_wraps_at_the_terminal_width() {
        let a = Advance::new(10);
        a.note(b"0123456789"); // exactly full: no wrap yet
        assert_eq!(a.pending_rows(), 0);
        a.note(b"x"); // the eleventh cell wraps
        assert_eq!(a.pending_rows(), 1);
        a.note(b"\r\n"); // the line feed from the wrapped row
        assert_eq!(a.take_rows(), 2);
    }

    #[test]
    fn escape_sequences_paint_nothing_even_when_split_across_writes() {
        let a = Advance::new(4);
        a.note(b"\x1b[38;5;");
        a.note(b"120m");
        a.note(b"ab");
        assert_eq!(a.pending_rows(), 0, "SGR bytes must not count as cells");
        a.note(b"\x1b]1337;File=inline=1:AAAA\x07");
        a.note(b"\x1b_Ga=T;\x1b\\");
        assert_eq!(a.pending_rows(), 0, "string sequences must not count");
        a.note(b"cd"); // cells 3-4: still fits
        assert_eq!(a.pending_rows(), 0);
        a.note(b"e"); // the fifth cell wraps
        assert_eq!(a.pending_rows(), 1);
    }

    #[test]
    fn multibyte_chars_split_across_writes_measure_once() {
        let a = Advance::new(2);
        let bytes = "é".as_bytes(); // two bytes, one cell
        a.note(&bytes[..1]);
        a.note(&bytes[1..]);
        a.note("é".as_bytes()); // second cell: the line is now full
        assert_eq!(a.pending_rows(), 0);
        a.note(b"x");
        assert_eq!(a.pending_rows(), 1);
    }

    #[test]
    fn wide_glyphs_take_two_cells() {
        let a = Advance::new(3);
        a.note("字".as_bytes()); // cells 1-2
        a.note("字".as_bytes()); // needs 2, only 1 left: wraps
        assert_eq!(a.pending_rows(), 1);
    }
}
