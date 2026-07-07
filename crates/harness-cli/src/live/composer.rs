//! The composer's edit buffer and recallable input history — the pure,
//! terminal-free core of the live input box, so the editing rules can be
//! unit-tested in isolation.

/// The composer's edit buffer: text (which may contain `\n` for multi-line
/// input) and a caret position. Pure and terminal-free so the editing rules can
/// be unit-tested in isolation. The caret is a character index in `0..=buf.len()`.
///
/// Single-line behavior is unchanged when the buffer holds no `\n`: `move_home`
/// / `move_end` and the line helpers all collapse to the whole buffer.
pub(super) struct Composer {
    buf: Vec<char>,
    cursor: usize,
}

impl Composer {
    pub(super) fn new() -> Self {
        Self {
            buf: Vec::new(),
            cursor: 0,
        }
    }

    /// A buffer pre-loaded with `text`, caret at the end — used to seed inline
    /// editing of a queued message and history recall.
    pub(super) fn seeded(text: &str) -> Self {
        let buf: Vec<char> = text.chars().collect();
        let cursor = buf.len();
        Self { buf, cursor }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The buffer's contents as a string (newlines included).
    pub(super) fn text(&self) -> String {
        self.buf.iter().collect()
    }

    pub(super) fn chars(&self) -> &[char] {
        &self.buf
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }

    /// The buffer split into its visual lines (on `\n`).
    pub(super) fn lines(&self) -> Vec<String> {
        self.text().split('\n').map(str::to_string).collect()
    }

    /// How many lines the buffer spans (at least 1).
    pub(super) fn line_count(&self) -> usize {
        1 + self.buf.iter().filter(|&&c| c == '\n').count()
    }

    /// The caret's `(line, column)`, both 0-based.
    pub(super) fn line_col(&self) -> (usize, usize) {
        let mut line = 0;
        let mut col = 0;
        for &c in &self.buf[..self.cursor] {
            if c == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    /// Buffer index where each line begins.
    fn line_starts(&self) -> Vec<usize> {
        let mut starts = vec![0];
        for (i, &c) in self.buf.iter().enumerate() {
            if c == '\n' {
                starts.push(i + 1);
            }
        }
        starts
    }

    /// Length (in chars) of line `n`, excluding its trailing newline.
    fn line_len(&self, n: usize) -> usize {
        let starts = self.line_starts();
        let start = starts[n];
        let end = starts.get(n + 1).map(|s| s - 1).unwrap_or(self.buf.len());
        end - start
    }

    /// Insert `c` at the caret and advance past it.
    pub(super) fn insert_char(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Insert a hard line break at the caret (multi-line input).
    pub(super) fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    /// Delete the character before the caret (Backspace).
    pub(super) fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buf.remove(self.cursor);
        }
    }

    /// Delete the character at the caret (Delete / forward-delete).
    pub(super) fn delete(&mut self) {
        if self.cursor < self.buf.len() {
            self.buf.remove(self.cursor);
        }
    }

    pub(super) fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(super) fn move_right(&mut self) {
        if self.cursor < self.buf.len() {
            self.cursor += 1;
        }
    }

    /// Move the caret to the start of the previous word (readline `M-b`):
    /// skip separators leftward, then the word itself.
    pub(super) fn move_word_left(&mut self) {
        while self.cursor > 0 && !is_word_char(self.buf[self.cursor - 1]) {
            self.cursor -= 1;
        }
        while self.cursor > 0 && is_word_char(self.buf[self.cursor - 1]) {
            self.cursor -= 1;
        }
    }

    /// Move the caret past the end of the next word (readline `M-f`):
    /// skip separators rightward, then the word itself.
    pub(super) fn move_word_right(&mut self) {
        let n = self.buf.len();
        while self.cursor < n && !is_word_char(self.buf[self.cursor]) {
            self.cursor += 1;
        }
        while self.cursor < n && is_word_char(self.buf[self.cursor]) {
            self.cursor += 1;
        }
    }

    /// Delete from the start of the previous word to the caret
    /// (Alt+Backspace / Ctrl+W).
    pub(super) fn delete_word_back(&mut self) {
        let end = self.cursor;
        self.move_word_left();
        self.buf.drain(self.cursor..end);
    }

    /// Delete from the caret through the end of the next word (Alt+D).
    pub(super) fn delete_word_forward(&mut self) {
        let start = self.cursor;
        self.move_word_right();
        self.buf.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Delete from the caret to the end of the line (readline `C-k`); at the
    /// end of a line, join it with the next (delete the newline).
    pub(super) fn kill_to_end(&mut self) {
        let (line, _) = self.line_col();
        let end = self.line_starts()[line] + self.line_len(line);
        if self.cursor == end {
            if end < self.buf.len() {
                self.buf.remove(end);
            }
        } else {
            self.buf.drain(self.cursor..end);
        }
    }

    /// Delete from the start of the line to the caret (readline `C-u`).
    pub(super) fn kill_to_start(&mut self) {
        let (line, _) = self.line_col();
        let start = self.line_starts()[line];
        self.buf.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Move to the start of the current line.
    pub(super) fn move_home(&mut self) {
        let (line, _) = self.line_col();
        self.cursor = self.line_starts()[line];
    }

    /// Move to the end of the current line.
    pub(super) fn move_end(&mut self) {
        let (line, _) = self.line_col();
        self.cursor = self.line_starts()[line] + self.line_len(line);
    }

    /// Move the caret up one line, keeping the column where possible. Returns
    /// `false` when already on the first line (the caller then recalls history
    /// or focuses the queue).
    pub(super) fn move_up(&mut self) -> bool {
        let (line, col) = self.line_col();
        if line == 0 {
            return false;
        }
        let target = line - 1;
        let col = col.min(self.line_len(target));
        self.cursor = self.line_starts()[target] + col;
        true
    }

    /// Move the caret down one line, keeping the column where possible. Returns
    /// `false` when already on the last line (the caller then recalls history).
    pub(super) fn move_down(&mut self) -> bool {
        let (line, col) = self.line_col();
        if line + 1 >= self.line_count() {
            return false;
        }
        let target = line + 1;
        let col = col.min(self.line_len(target));
        self.cursor = self.line_starts()[target] + col;
        true
    }

    /// Replace the whole buffer (history recall), caret at the end.
    pub(super) fn set_text(&mut self, text: &str) {
        self.buf = text.chars().collect();
        self.cursor = self.buf.len();
    }

    /// Take the buffer's contents, clearing it and resetting the caret.
    pub(super) fn take(&mut self) -> String {
        let line: String = self.buf.drain(..).collect();
        self.cursor = 0;
        line
    }
}

/// What counts as a word for word-wise movement and deletion: alphanumerics
/// (readline's default). Punctuation like `-`, `/`, and `.` separates, so
/// hopping through paths and model ids stops at each segment.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// Recallable input history: past submissions plus a scratch slot for the
/// in-progress draft, so Up/Down walk into the past and back without losing what
/// you were typing. `pos == entries.len()` means "the live draft" (not recalling).
#[derive(Default)]
pub(super) struct History {
    entries: Vec<String>,
    pos: usize,
    /// The live draft stashed when stepping back into history, restored on return.
    draft: String,
}

impl History {
    /// Seed history with prior entries (newest last), recall positioned at the
    /// live draft. Used to carry `prompt_history.txt` into the session.
    pub(super) fn with_entries(entries: Vec<String>) -> Self {
        let pos = entries.len();
        Self {
            entries,
            pos,
            draft: String::new(),
        }
    }

    /// The entries, newest last (for persisting back to disk).
    pub(super) fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Record a submitted line (de-duped against the most recent) and reset the
    /// recall position back to the live draft.
    pub(super) fn push(&mut self, line: &str) {
        if !line.is_empty() && self.entries.last().map(String::as_str) != Some(line) {
            self.entries.push(line.to_string());
        }
        self.pos = self.entries.len();
    }

    /// Recall the previous entry, stashing the live `draft` the first time we
    /// step off it. Returns the text to show, or `None` at the oldest entry.
    pub(super) fn prev(&mut self, draft: &str) -> Option<String> {
        if self.pos == 0 || self.entries.is_empty() {
            return None;
        }
        if self.pos == self.entries.len() {
            self.draft = draft.to_string();
        }
        self.pos -= 1;
        self.entries.get(self.pos).cloned()
    }

    /// Recall the next entry; stepping past the newest restores the stashed draft.
    pub(super) fn next(&mut self) -> Option<String> {
        if self.pos >= self.entries.len() {
            return None;
        }
        self.pos += 1;
        if self.pos == self.entries.len() {
            Some(std::mem::take(&mut self.draft))
        } else {
            self.entries.get(self.pos).cloned()
        }
    }

    pub(super) fn reset(&mut self) {
        self.pos = self.entries.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Composer {
        let mut c = Composer::new();
        for ch in text.chars() {
            c.insert_char(ch);
        }
        c
    }

    #[test]
    fn insert_appends_and_advances_caret() {
        let c = typed("hi");
        assert_eq!(c.chars().iter().collect::<String>(), "hi");
        assert_eq!(c.cursor(), 2);
        assert!(!c.is_empty());
    }

    #[test]
    fn newline_splits_into_lines() {
        let mut c = typed("ab");
        c.insert_newline();
        c.insert_char('c');
        assert_eq!(c.text(), "ab\nc");
        assert_eq!(c.lines(), vec!["ab".to_string(), "c".to_string()]);
        assert_eq!(c.line_count(), 2);
        assert_eq!(c.line_col(), (1, 1));
    }

    #[test]
    fn up_down_move_between_lines_and_keep_column() {
        let mut c = typed("hello");
        c.insert_newline();
        c.insert_char('h');
        c.insert_char('i'); // "hello\nhi", caret (1,2)
        assert_eq!(c.line_col(), (1, 2));
        assert!(c.move_up());
        assert_eq!(c.line_col(), (0, 2));
        assert!(!c.move_up()); // first line — caller recalls history
        assert!(c.move_down());
        assert_eq!(c.line_col(), (1, 2));
        assert!(!c.move_down()); // last line
    }

    #[test]
    fn home_end_line_relative_and_single_line_unchanged() {
        let mut c = typed("ab");
        c.insert_newline();
        c.insert_char('c');
        c.insert_char('d'); // "ab\ncd"
        c.move_home();
        assert_eq!(c.line_col(), (1, 0));
        c.move_end();
        assert_eq!(c.line_col(), (1, 2));

        let mut s = typed("hello");
        s.move_home();
        assert_eq!(s.cursor(), 0);
        s.move_end();
        assert_eq!(s.cursor(), 5);
        assert!(!s.move_up());
        assert!(!s.move_down());
    }

    #[test]
    fn history_walks_back_and_restores_the_draft() {
        let mut h = History::with_entries(vec!["first".into(), "second".into()]);
        assert_eq!(h.prev("typing…").as_deref(), Some("second"));
        assert_eq!(h.prev("typing…").as_deref(), Some("first"));
        assert_eq!(h.prev("typing…"), None); // oldest
        assert_eq!(h.next().as_deref(), Some("second"));
        assert_eq!(h.next().as_deref(), Some("typing…")); // draft restored
        assert_eq!(h.next(), None);
        assert_eq!(h.pos, h.entries.len()); // back at the live draft
    }

    #[test]
    fn history_dedupes_consecutive_and_resets() {
        let mut h = History::default();
        h.push("a");
        h.push("a");
        h.push("b");
        assert_eq!(h.entries, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(h.pos, h.entries.len());
        h.prev("");
        h.reset();
        assert_eq!(h.pos, h.entries.len());
    }

    #[test]
    fn insert_respects_caret_position() {
        let mut c = typed("ac");
        c.move_left(); // between a and c
        c.insert_char('b');
        assert_eq!(c.chars().iter().collect::<String>(), "abc");
        assert_eq!(c.cursor(), 2);
    }

    #[test]
    fn backspace_deletes_before_caret_and_guards_start() {
        let mut c = typed("ab");
        c.backspace();
        assert_eq!(c.chars().iter().collect::<String>(), "a");
        c.backspace();
        assert!(c.is_empty());
        c.backspace(); // no-op at start
        assert!(c.is_empty());
        assert_eq!(c.cursor(), 0);
    }

    #[test]
    fn delete_removes_at_caret_and_guards_end() {
        let mut c = typed("abc");
        c.move_home();
        c.delete();
        assert_eq!(c.chars().iter().collect::<String>(), "bc");
        assert_eq!(c.cursor(), 0);
        c.move_end();
        c.delete(); // no-op at end
        assert_eq!(c.chars().iter().collect::<String>(), "bc");
    }

    #[test]
    fn cursor_movement_clamps_at_both_edges() {
        let mut c = typed("ab");
        c.move_end();
        assert_eq!(c.cursor(), 2);
        c.move_right(); // clamp at end
        assert_eq!(c.cursor(), 2);
        c.move_home();
        assert_eq!(c.cursor(), 0);
        c.move_left(); // clamp at start
        assert_eq!(c.cursor(), 0);
    }

    #[test]
    fn take_returns_line_and_clears() {
        let mut c = typed("send me");
        assert_eq!(c.take(), "send me");
        assert!(c.is_empty());
        assert_eq!(c.cursor(), 0);
        // A fresh take after clearing yields an empty line.
        assert_eq!(c.take(), "");
    }

    #[test]
    fn editing_handles_unicode_by_char_not_byte() {
        let mut c = typed("café");
        assert_eq!(c.cursor(), 4);
        c.backspace();
        assert_eq!(c.chars().iter().collect::<String>(), "caf");
        c.insert_char('é');
        assert_eq!(c.take(), "café");
    }

    #[test]
    fn word_left_and_right_hop_between_words() {
        let mut c = typed("fix the-bug now");
        c.move_word_left();
        assert_eq!(c.cursor(), 12); // before "now"
        c.move_word_left();
        assert_eq!(c.cursor(), 8); // before "bug"
        c.move_word_left();
        assert_eq!(c.cursor(), 4); // before "the"
        c.move_word_left();
        assert_eq!(c.cursor(), 0);
        c.move_word_left(); // clamp at start
        assert_eq!(c.cursor(), 0);
        c.move_word_right();
        assert_eq!(c.cursor(), 3); // after "fix"
        c.move_word_right();
        assert_eq!(c.cursor(), 7); // after "the"
        c.move_word_right();
        c.move_word_right();
        assert_eq!(c.cursor(), 15); // after "now"
        c.move_word_right(); // clamp at end
        assert_eq!(c.cursor(), 15);
    }

    #[test]
    fn delete_word_back_and_forward() {
        let mut c = typed("run the tests");
        c.delete_word_back();
        assert_eq!(c.text(), "run the ");
        c.delete_word_back();
        assert_eq!(c.text(), "run ");

        let mut c = typed("run the tests");
        c.move_home();
        c.delete_word_forward();
        assert_eq!(c.text(), " the tests");
        assert_eq!(c.cursor(), 0);
        c.delete_word_forward();
        assert_eq!(c.text(), " tests");
    }

    #[test]
    fn kill_to_end_and_start_are_line_relative() {
        let mut c = typed("hello world");
        for _ in 0..5 {
            c.move_left(); // caret after "hello "
        }
        c.kill_to_end();
        assert_eq!(c.text(), "hello ");
        c.kill_to_start();
        assert_eq!(c.text(), "");

        // On a multi-line buffer the kill stops at the line boundary; a second
        // C-k at line end joins the lines (deletes the newline).
        let mut c = typed("one\ntwo");
        c.move_up();
        c.move_end();
        c.kill_to_end();
        assert_eq!(c.text(), "onetwo");

        let mut c = typed("one\ntwo");
        c.move_up();
        c.move_home();
        c.kill_to_end();
        assert_eq!(c.text(), "\ntwo");
    }

    #[test]
    fn seeded_composer_loads_text_with_caret_at_end() {
        let mut c = Composer::seeded("hello");
        assert_eq!(c.cursor(), 5);
        c.backspace();
        assert_eq!(c.take(), "hell");
        // Seeding an empty string is a no-op editor.
        assert!(Composer::seeded("").is_empty());
    }
}
