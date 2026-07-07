//! Painting the pinned bottom area: the framed queue table, the meters, the
//! divider, and the frameless composer box.
//!
//! Everything here writes below the DECSTBM scroll region, bracketed by
//! save/restore-cursor so the streaming output above is never disturbed. The
//! reserved height changes as the composer grows or the queue list appears,
//! which re-carves the region (see [`Live::paint`]).

use std::io::Write;

use crate::render::truncate;

use super::keys::Mode;
use super::layout::{queue_rows, Focus, QueueRow, MAX_QUEUE_ROWS, QUEUE_FRAME_ROWS};
use super::text::{composer_prompt, render_buffer, render_text_line, wrap_line};
use super::{Live, DIVIDER_ROWS, MAX_INPUT_ROWS, SPACER_ROWS};

impl Live {
    /// Repaint the input area (used during streaming and after each keystroke).
    /// The box can change height as lines are added/removed, which re-carves the
    /// scroll region, so this defers to the full [`Live::paint`] — bracketed by
    /// save/restore-cursor, it leaves the streaming output position untouched.
    pub(super) fn render_composer(&mut self) {
        self.paint(false);
    }

    /// Repaint the whole bottom area — the stacked queue list plus the composer
    /// — re-carving the scroll region only when the reserved height changed.
    pub(super) fn render(&mut self) {
        self.paint(false);
    }

    /// Like [`Live::render`] but unconditionally re-issues the scroll region —
    /// used after a resize or after reclaiming the screen from the picker, where
    /// the terminal's region no longer matches our state.
    pub(super) fn render_forcing_region(&mut self) {
        self.paint(true);
    }

    fn paint(&mut self, force_region: bool) {
        if self.suspended {
            return;
        }
        let len = self.previews.len();
        // Reserve frame rows up front so the header/footer borders never push the
        // last line of streamed output off-screen.
        let frame = if len == 0 { 0 } else { QUEUE_FRAME_ROWS };
        let plan = queue_rows(len, self.focus, self.rows, MAX_QUEUE_ROWS, frame);
        let chrome = if plan.is_empty() { 0 } else { QUEUE_FRAME_ROWS };
        // The input area's height grows with the lines typed.
        let box_lines = self.composer_box_lines();
        let box_h = box_lines.len() as u16;
        // Between the agent's output and the pinned input area: a blank spacer,
        // the compression savings (when active), the context-usage status, then
        // a faint divider rule (output · blank · compression · status · rule ·
        // input), so the prompt always has breathing room and a clear edge, and
        // the meters sit right above the input instead of trailing the last
        // message.
        let status_rows: u16 =
            self.compression_line.is_some() as u16 + self.status_line.is_some() as u16;
        let reserved = (plan.len() + chrome) as u16
            + SPACER_ROWS as u16
            + status_rows
            + DIVIDER_ROWS as u16
            + box_h;
        let new_bottom = self.rows.saturating_sub(reserved).max(1);

        let mut buf = String::new();
        if force_region || new_bottom != self.region_bottom {
            // On an incremental change, clear the rows that move between the
            // output region and the reserved area so no stale text lingers.
            if !force_region {
                let lo = self.region_bottom.min(new_bottom) + 1;
                for r in lo..=self.rows {
                    buf.push_str(&format!("\x1b[{r};1H\x1b[2K"));
                }
            }
            // Re-carve the region and park the output cursor at its new bottom.
            buf.push_str(&format!("\x1b[1;{new_bottom}r\x1b[{new_bottom};1H"));
            self.region_bottom = new_bottom;
        }

        // Paint the framed queue table + composer below the region, bracketed by
        // save/restore so the output cursor inside the region is left undisturbed.
        buf.push_str("\x1b7");
        // Keep the spacer row(s) directly below the output region blank.
        for s in 0..SPACER_ROWS as u16 {
            buf.push_str(&format!("\x1b[{};1H\x1b[2K", new_bottom + 1 + s));
        }
        // The compression savings and context-usage meters sit under the
        // spacer, just above the divider (compression on top of context).
        let mut next_row = new_bottom + 1 + SPACER_ROWS as u16;
        for line in [&self.compression_line, &self.status_line]
            .into_iter()
            .flatten()
        {
            buf.push_str(&format!("\x1b[{next_row};1H\x1b[2K{line}"));
            next_row += 1;
        }
        // Then a faint full-width divider rule, just above the input area.
        let divider_row = next_row;
        buf.push_str(&format!(
            "\x1b[{divider_row};1H\x1b[2K{}",
            self.ui.dim(&"─".repeat(self.cols as usize))
        ));
        if !plan.is_empty() {
            let box_w = self.queue_box_w();
            let mut r = divider_row + DIVIDER_ROWS as u16;
            buf.push_str(&format!("\x1b[{r};1H\x1b[2K{}", self.queue_header(box_w)));
            for row in &plan {
                r += 1;
                buf.push_str(&format!(
                    "\x1b[{r};1H\x1b[2K{}",
                    self.queue_row_line(*row, box_w)
                ));
            }
            r += 1;
            buf.push_str(&format!("\x1b[{r};1H\x1b[2K{}", self.queue_footer(box_w)));
        }
        // The input box occupies the bottom `box_h` rows, pinned to row H.
        let box_start = self.rows.saturating_sub(box_h).saturating_add(1);
        for (i, line) in box_lines.iter().enumerate() {
            let row = box_start + i as u16;
            buf.push_str(&format!("\x1b[{row};1H\x1b[2K{line}"));
        }
        buf.push_str("\x1b8");
        let _ = write!(self.out, "{buf}");
        let _ = self.out.flush();
    }

    /// The inner content width of the queue table — the columns available
    /// *between* the `│ ` and ` │` of a framed row. Two-space left margin plus
    /// the four border/padding columns are reserved out of the terminal width.
    fn queue_box_w(&self) -> usize {
        (self.cols as usize).saturating_sub(6).max(8)
    }

    /// The table's top border, embedding the `Queued` title:
    /// `┌─ Queued ───────┐`. The title is accented; the rule is brown.
    fn queue_header(&self, box_w: usize) -> String {
        let label = "Queued";
        // `┌─ ` (3) + label + ` ` (1) + fill + `┐` (1) must span `box_w + 4`
        // columns to align with the framed rows below, so fill = box_w - 1 - len.
        let fill = box_w.saturating_sub(1 + label.chars().count());
        format!(
            "  {}{}{}",
            self.ui.brown("┌─ "),
            self.ui.accent(label),
            self.ui.brown(&format!(" {}┐", "─".repeat(fill))),
        )
    }

    /// The table's bottom border, embedding a key hint for what you can do with
    /// the queue right now — so editing a queued prompt is discoverable, not
    /// just deleting it. The hint is mode-aware (`↑ edit queued` while composing,
    /// `enter edit · d delete` while browsing, `enter save · esc cancel` while
    /// editing) and embeds in the rule with the same geometry as the header so
    /// the frame stays aligned; on a terminal too narrow to fit it, it falls back
    /// to a plain border.
    fn queue_footer(&self, box_w: usize) -> String {
        let hint = match self.mode() {
            Mode::Edit => "enter save · esc cancel",
            Mode::Browse => "enter edit · d delete",
            Mode::Compose => "↑ edit queued",
        };
        if hint.chars().count() < box_w {
            let fill = box_w.saturating_sub(1 + hint.chars().count());
            format!(
                "  {}{}{}",
                self.ui.brown("└─ "),
                self.ui.dim(hint),
                self.ui.brown(&format!(" {}┘", "─".repeat(fill))),
            )
        } else {
            format!(
                "  {}",
                self.ui.brown(&format!("└{}┘", "─".repeat(box_w + 2)))
            )
        }
    }

    /// Render one framed table row (`│ … │`): an `…(+k more)` overflow marker, a
    /// dimmed preview, the reverse-video highlighted focused item, or the live
    /// inline editor. Every cell is padded to `box_w` so the right border lines up.
    fn queue_row_line(&self, row: QueueRow, box_w: usize) -> String {
        let bar = self.ui.brown("│");
        match row {
            QueueRow::More(k) => {
                let text = format!("…(+{k} more)");
                let pad = box_w.saturating_sub(text.chars().count());
                format!("  {bar} {}{} {bar}", self.ui.dim(&text), " ".repeat(pad))
            }
            QueueRow::Item(idx) => {
                let num = idx + 1;
                let focused = self.focus == Focus::Item(idx);
                if focused {
                    if let Some(edit) = self.edit.as_ref() {
                        let prefix = format!("✎ {num}. ");
                        let prefix_w = prefix.chars().count();
                        // Leave a column for the caret so the editor never spills
                        // past the right border.
                        let avail = box_w.saturating_sub(prefix_w + 1);
                        let (body, width) = render_buffer(edit, avail, true);
                        let pad = box_w.saturating_sub(prefix_w + width);
                        return format!(
                            "  {bar} {}{}{} {bar}",
                            self.ui.accent(&prefix),
                            body,
                            " ".repeat(pad),
                        );
                    }
                    let plain = self.item_text(idx, num, box_w);
                    let pad = box_w.saturating_sub(plain.chars().count());
                    // Plain text under reverse video (no nested color codes).
                    format!("  {bar} \x1b[7m{plain}{}\x1b[0m {bar}", " ".repeat(pad))
                } else {
                    let prefix = format!("{num}. ");
                    let preview =
                        self.item_preview(idx, box_w.saturating_sub(prefix.chars().count()));
                    let pad =
                        box_w.saturating_sub(prefix.chars().count() + preview.chars().count());
                    format!(
                        "  {bar} {}{}{} {bar}",
                        self.ui.accent(&prefix),
                        self.ui.cream(&preview),
                        " ".repeat(pad),
                    )
                }
            }
        }
    }

    /// The numbered, width-fitted plain text for an item (`3. fix the bug`),
    /// clamped so it never exceeds the table's content width.
    fn item_text(&self, idx: usize, num: usize, box_w: usize) -> String {
        let prefix = format!("{num}. ");
        format!(
            "{prefix}{}",
            self.item_preview(idx, box_w.saturating_sub(prefix.chars().count()))
        )
    }

    /// A queued message preview fitted to `width` columns, leaving room for the
    /// `…` truncation marker so the result never overflows the cell.
    fn item_preview(&self, idx: usize, width: usize) -> String {
        let preview = self.previews.get(idx).map(String::as_str).unwrap_or("");
        truncate(preview, width.saturating_sub(1))
    }

    /// Render the input rows — deliberately **frameless** (no bordered box), so
    /// a terminal selection over the bottom of the screen never picks up border
    /// characters. Long lines **word-wrap** onto the next visual row (rather
    /// than scrolling sideways); the themed prompt sits on the first row and
    /// wrapped/continuation rows align under it. The area grows with the rows
    /// typed (capped at [`MAX_INPUT_ROWS`], windowing around the caret beyond
    /// that). The caret (reverse-video cell) shows only when the composer holds
    /// focus — not while browsing/editing the queue.
    fn composer_box_lines(&self) -> Vec<String> {
        let depth = self.previews.len();
        let (plain_prompt, styled_prompt) = composer_prompt(&self.ui, depth);
        let prompt_w = plain_prompt.chars().count();
        let box_w = self.queue_box_w();
        // Wrap width leaves a column for the caret so it never spills past the
        // right edge; every row is indented under the prompt for alignment.
        let avail = box_w.saturating_sub(prompt_w + 1).max(4);
        let caret_on = matches!(self.focus, Focus::Composer) && self.edit.is_none();

        let lines: Vec<Vec<char>> = self
            .composer
            .lines()
            .into_iter()
            .map(|l| l.chars().collect())
            .collect();
        let (caret_line, caret_col) = self.composer.line_col();

        // Word-wrap each logical line into visual rows, tracking which visual row
        // + column the caret lands on.
        let mut vrows: Vec<Vec<char>> = Vec::new();
        let mut caret_vrow = 0usize;
        let mut caret_vcol = 0usize;
        for (li, line) in lines.iter().enumerate() {
            for (start, chunk) in wrap_line(line, avail) {
                let len = chunk.len();
                let owns_caret = caret_on
                    && li == caret_line
                    && ((caret_col >= start && caret_col < start + len)
                        || (caret_col == start + len && start + len == line.len()));
                if owns_caret {
                    caret_vrow = vrows.len();
                    caret_vcol = caret_col - start;
                }
                vrows.push(chunk);
            }
        }

        // Window the visible rows around the caret when there are more than fit.
        let total = vrows.len();
        let (lo, hi) = if total <= MAX_INPUT_ROWS {
            (0, total)
        } else {
            let lo = caret_vrow
                .saturating_sub(MAX_INPUT_ROWS / 2)
                .min(total - MAX_INPUT_ROWS);
            (lo, lo + MAX_INPUT_ROWS)
        };

        let mut out: Vec<String> = Vec::new();
        // A slash-command / argument suggestion hint sits just above the input
        // while composing at idle (no spinner), so completions are discoverable.
        if self.spinner.is_none()
            && matches!(self.focus, Focus::Composer)
            && self.edit.is_none()
            && !self.completion.is_empty()
        {
            out.extend(self.completion_hint(box_w));
        }
        for (vi, row) in vrows.iter().enumerate().take(hi).skip(lo) {
            let caret = (caret_on && vi == caret_vrow).then_some(caret_vcol);
            let (body, _) = render_text_line(row, caret, avail);
            let prefix = if vi == 0 {
                styled_prompt.clone()
            } else {
                " ".repeat(prompt_w)
            };
            out.push(format!("  {prefix}{body}"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::plain_live;
    use super::*;

    #[test]
    fn queue_table_header_shows_title_and_aligns_with_rows() {
        let mut l = plain_live(60, 24);
        l.sync_queue(&[
            "fix the parser".into(),
            "add tests".into(),
            "write docs".into(),
        ]);
        let box_w = l.queue_box_w();

        let header = l.queue_header(box_w);
        let footer = l.queue_footer(box_w);
        let row = l.queue_row_line(QueueRow::Item(0), box_w);

        // The header carries the "Queued" title and the box-drawing corners.
        assert!(header.contains("Queued"), "header: {header:?}");
        assert!(header.trim_start().starts_with("┌─ "), "header: {header:?}");
        assert!(footer.trim_start().starts_with('└'), "footer: {footer:?}");
        assert!(footer.trim_end().ends_with('┘'), "footer: {footer:?}");
        // The item is framed and numbered.
        assert!(row.contains("│"), "row: {row:?}");
        assert!(row.contains("1. fix the parser"), "row: {row:?}");

        // With color disabled the only escapes are reverse-video on focus, so
        // every framed line is the same visible width as the borders.
        let width = |s: &str| s.chars().count();
        assert_eq!(width(&header), width(&row), "header vs row width");
        assert_eq!(width(&footer), width(&row), "footer vs row width");
    }

    #[test]
    fn queue_footer_hint_surfaces_edit_per_mode() {
        let mut l = plain_live(60, 24);
        l.sync_queue(&["fix the parser".into()]);
        let box_w = l.queue_box_w();

        // Composing with a queue: point the user up into it.
        assert!(l.queue_footer(box_w).contains("↑ edit queued"));

        // Browsing a focused item: edit is offered alongside delete.
        l.focus = Focus::Item(0);
        let browsing = l.queue_footer(box_w);
        assert!(browsing.contains("enter edit"), "footer: {browsing:?}");
        assert!(browsing.contains("d delete"), "footer: {browsing:?}");

        // Inline-editing: save + cancel.
        l.begin_edit("fix the parser");
        let editing = l.queue_footer(box_w);
        assert!(editing.contains("enter save"), "footer: {editing:?}");
        assert!(editing.contains("esc cancel"), "footer: {editing:?}");
    }

    #[test]
    fn focused_and_overflow_rows_keep_the_right_border_aligned() {
        let mut l = plain_live(50, 24);
        l.sync_queue(&(0..3).map(|i| format!("task {i}")).collect::<Vec<_>>());
        let box_w = l.queue_box_w();
        let target = l.queue_row_line(QueueRow::Item(1), box_w).chars().count();

        l.focus = Focus::Item(1);
        let focused = l.queue_row_line(QueueRow::Item(1), box_w);
        // Reverse-video escapes don't count toward visible width, so trim them.
        let visible = focused.replace("\x1b[7m", "").replace("\x1b[0m", "");
        assert_eq!(visible.chars().count(), target, "focused row width");

        let more = l.queue_row_line(QueueRow::More(4), box_w);
        assert!(more.contains("…(+4 more)"), "more: {more:?}");
        assert_eq!(more.chars().count(), target, "overflow row width");
    }
}
