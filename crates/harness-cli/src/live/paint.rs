//! Painting the pinned bottom area: the framed queue table, the meters, the
//! divider, and the frameless composer box.
//!
//! Everything here writes below the DECSTBM scroll region, bracketed by
//! save/restore-cursor so the streaming output above is never disturbed. The
//! reserved height changes as the composer grows or the queue list appears,
//! which re-carves the region (see [`Live::paint`]).

use std::io::Write;

use crate::ansi::{SYNC_BEGIN, SYNC_END};
use crate::render::truncate;
use crate::width::str_width;

use super::keys::Mode;
use super::layout::{queue_rows, Focus, QueueRow, MAX_QUEUE_ROWS, QUEUE_FRAME_ROWS};
use super::pinned::{PinnedPlan, Section, SectionKind};
use super::text::{composer_prompt, render_buffer, render_text_line, wrap_line};
use super::{Live, DIVIDER_ROWS, MAX_INPUT_ROWS, SPACER_ROWS};

/// The escape sequence to (re)carve the DECSTBM scroll region so its bottom is
/// `new_bottom` (rows `1..=new_bottom`), reserving `rows - new_bottom` pinned
/// rows below it, while keeping the output cursor right after the last line of
/// conversation.
///
/// The cursor may be anywhere on screen when this runs: under a short banner
/// on the first paint, at the bottom of a full region mid-stream, wherever a
/// picker or a resize left it. Rather than parking it at the new bottom
/// (which opened a void of blank rows under a short conversation, and painted
/// the pinned area over the last lines of a full one), the sequence makes
/// room *from the cursor*: with the region reset so the whole screen scrolls,
/// it feeds `reserved` line feeds and steps back up by the same count. When
/// the cursor sits above `new_bottom` the terminal clamps the walk at the
/// bottom row and nothing scrolls; when it sits below, the screen scrolls by
/// exactly the overshoot, so the conversation lands with its last line on
/// `new_bottom` and not a row higher. The cursor is saved around the region
/// changes (DECSTBM homes it) and restored inside the new region.
///
/// Rows that leave the region (`min(old, new) + 1..=rows`) are cleared so no
/// stale chrome lingers in either area; the pinned paint that follows
/// rewrites every reserved row anyway.
fn region_transition(old_bottom: u16, new_bottom: u16, rows: u16) -> String {
    let reserved = rows.saturating_sub(new_bottom);
    let mut buf = String::from("\x1b7\x1b[r\x1b8");
    if reserved > 0 {
        buf.push_str(&"\n".repeat(reserved as usize));
        buf.push_str(&format!("\x1b[{reserved}A"));
    }
    buf.push_str(&format!("\x1b7\x1b[1;{new_bottom}r"));
    let lo = old_bottom.min(new_bottom) + 1;
    for r in lo..=rows {
        buf.push_str(&format!("\x1b[{r};1H\x1b[2K"));
    }
    buf.push_str("\x1b8");
    buf
}

/// The pending repaint level for the pinned area. Handlers *request*; the
/// event/key loops *flush* — one paint per handled event instead of a paint
/// buried in every handler. `ForceRegion` dominates `Paint`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(super) enum Repaint {
    #[default]
    None,
    Paint,
    ForceRegion,
}

impl Live {
    /// Mark the pinned area dirty; the next [`Live::flush_paint`] repaints it.
    pub(super) fn request_paint(&mut self) {
        self.repaint = self.repaint.max(Repaint::Paint);
    }

    /// Like [`Live::request_paint`] but the flush will unconditionally re-issue
    /// the scroll region — for after a resize or a reclaimed screen, where the
    /// terminal's region no longer matches our state.
    pub(super) fn request_paint_forced(&mut self) {
        self.repaint = Repaint::ForceRegion;
    }

    /// Repaint the pinned area if anything requested it since the last flush.
    /// Called at the loops' choke points (after each handled event/key batch).
    pub(super) fn flush_paint(&mut self) {
        match std::mem::take(&mut self.repaint) {
            Repaint::None => {}
            Repaint::Paint => self.paint(false),
            Repaint::ForceRegion => self.paint(true),
        }
    }

    /// Repaint the whole bottom area immediately (request + flush) — for
    /// one-off call sites outside the event loops (initial paint, cooked-mode
    /// transitions). Inside handlers, prefer [`Live::request_paint`].
    pub(super) fn render(&mut self) {
        self.request_paint();
        self.flush_paint();
    }

    /// Immediate forced repaint — see [`Live::request_paint_forced`].
    pub(super) fn render_forcing_region(&mut self) {
        self.request_paint_forced();
        self.flush_paint();
    }

    /// Build the pinned area's layout for this paint: every section with its
    /// exact lines, in top-to-bottom order (blank spacer · fleet lanes ·
    /// compression savings · context meters · divider rule · queue table ·
    /// composer box). The reserved height and the paint walk both derive from
    /// the returned plan, so they cannot disagree.
    fn pinned_plan(&self) -> PinnedPlan {
        let len = self.previews.len();
        // Reserve frame rows up front so the header/footer borders never push the
        // last line of streamed output off-screen.
        let frame = if len == 0 { 0 } else { QUEUE_FRAME_ROWS };
        let plan = queue_rows(len, self.focus, self.rows, MAX_QUEUE_ROWS, frame);
        let mut queue_lines = Vec::new();
        if !plan.is_empty() {
            let box_w = self.queue_box_w();
            queue_lines.push(self.queue_header(box_w));
            for row in &plan {
                queue_lines.push(self.queue_row_line(*row, box_w));
            }
            queue_lines.push(self.queue_footer(box_w));
        }
        let compression_lines: Vec<String> = self.compression_line.iter().cloned().collect();
        let meter_lines = self.meter_lines();
        let divider = vec![self.ui.dim(&"─".repeat(self.cols as usize)); DIVIDER_ROWS];

        // Every section above is bounded (the queue plan caps against `rows`,
        // the composer windows to MAX_INPUT_ROWS), but a running fleet's lanes
        // block can be tall (up to 6 lanes + an 8-row focused tail). On a short
        // terminal it's the one section that could push the reserved area past
        // the screen and smear addressed rows over the composer, so cap it to
        // whatever height is left after the fixed sections, keeping at least
        // one output row; the block trims from the end (hint, then tail rows)
        // so the lane lines themselves survive.
        let mut sections = vec![Section::blank(SectionKind::Spacer, SPACER_ROWS)];
        // The staged-Ctrl-C acknowledgement sits on the very bottom row, under
        // the input, so the ways out are read where the eye already is.
        let mut composer_lines = self.composer_box_lines();
        composer_lines.extend(self.notice.iter().cloned());
        let composer = Section::new(SectionKind::Composer, composer_lines);
        let fixed = (SPACER_ROWS
            + compression_lines.len()
            + meter_lines.len()
            + divider.len()
            + queue_lines.len()
            + composer.lines.len()) as u16;
        let fleet_budget = self.rows.saturating_sub(fixed + 1) as usize;
        let mut fleet_lines = self.fleet_lines();
        fleet_lines.truncate(fleet_budget);
        // The command-output card gets what the fleet left; a card that
        // doesn't fit shows its newest rows.
        let card_budget = fleet_budget.saturating_sub(fleet_lines.len());
        let mut card_lines = self.tool_card_lines();
        if card_lines.len() > card_budget {
            card_lines.drain(..card_lines.len() - card_budget);
        }

        sections.push(Section::new(SectionKind::Fleet, fleet_lines));
        sections.push(Section::new(SectionKind::ToolCard, card_lines));
        sections.push(Section::new(SectionKind::Compression, compression_lines));
        sections.push(Section::new(SectionKind::Status, meter_lines));
        sections.push(Section::new(SectionKind::Divider, divider));
        sections.push(Section::new(SectionKind::Queue, queue_lines));
        sections.push(composer);
        PinnedPlan { sections }
    }

    fn paint(&mut self, force_region: bool) {
        if self.suspended() {
            return;
        }
        let plan = self.pinned_plan();
        let new_bottom = plan.region_bottom(self.rows);

        let mut buf = String::new();
        if force_region || new_bottom != self.region_bottom {
            buf.push_str(&region_transition(
                self.region_bottom,
                new_bottom,
                self.rows,
            ));
            self.region_bottom = new_bottom;
        }

        // Paint every pinned row below the region, bracketed by save/restore so
        // the output cursor inside the region is left undisturbed. The walk is
        // a flat pass over the plan: each line lands one row further down, and
        // the composer (the last section) ends exactly on the bottom row.
        buf.push_str("\x1b7");
        let mut row = new_bottom + 1;
        for section in &plan.sections {
            for line in &section.lines {
                buf.push_str(&format!("\x1b[{row};1H\x1b[2K{line}"));
                row += 1;
            }
        }
        buf.push_str("\x1b8");
        // Synchronized output (mode 2026): the terminal holds the whole frame
        // and presents it at once, so the clear-then-rewrite of every pinned
        // row can never be seen half-painted.
        let _ = write!(self.out, "{SYNC_BEGIN}{buf}{SYNC_END}");
        let _ = self.out.flush();
    }

    /// The pinned meter lines. While a turn runs the first one leads with a
    /// braille spinner and a whole-second elapsed timer (`⠋ 12s`), so the time
    /// a turn has been going is readable without watching the spinner scroll
    /// past. [`crate::turn::TURN_INDICATOR_CELLS`] is reserved for it when the
    /// meters are built, so the prefix can't push the line past the edge.
    fn meter_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self.status_lines.to_vec();
        let (Some(started), Some(first)) = (self.turn_started, lines.first()) else {
            return lines;
        };
        let elapsed = started.elapsed();
        let indicator = format!(
            "{} {}",
            self.ui.accent(&super::braille_frame(elapsed).to_string()),
            self.ui.dim(&format!("{}s", elapsed.as_secs())),
        );
        lines[0] = format!(
            "  {indicator} {}",
            first.strip_prefix("  ").unwrap_or(first)
        );
        lines
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
        let fill = box_w.saturating_sub(1 + str_width(label));
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
            Mode::Compose => "↑ edit queued · alt+↑ un-queue · ctrl+q queue",
        };
        if str_width(hint) < box_w {
            let fill = box_w.saturating_sub(1 + str_width(hint));
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
                let pad = box_w.saturating_sub(str_width(&text));
                format!("  {bar} {}{} {bar}", self.ui.dim(&text), " ".repeat(pad))
            }
            QueueRow::Item(idx) => {
                let num = idx + 1;
                let focused = self.focus == Focus::Item(idx);
                if focused {
                    if let Some(edit) = self.edit.as_ref() {
                        let prefix = format!("✎ {num}. ");
                        let prefix_w = str_width(&prefix);
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
                    let pad = box_w.saturating_sub(str_width(&plain));
                    // Plain text under reverse video (no nested color codes).
                    format!("  {bar} \x1b[7m{plain}{}\x1b[0m {bar}", " ".repeat(pad))
                } else {
                    let prefix = format!("{num}. ");
                    let preview = self.item_preview(idx, box_w.saturating_sub(str_width(&prefix)));
                    let pad = box_w.saturating_sub(str_width(&prefix) + str_width(&preview));
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
            self.item_preview(idx, box_w.saturating_sub(str_width(&prefix)))
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
        let prompt_w = str_width(&plain_prompt);
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
        // Enter's fold keys off the same predicate, so what it accepts is
        // always what's on screen.
        if self.completion_showing() {
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
    use super::super::pinned::SectionKind;
    use super::super::test_support::plain_live;
    use super::*;

    /// Replay `prelude` then a region transition through a terminal emulator.
    fn transition_screen(prelude: &str, old: u16, new: u16, rows: u16) -> vt100::Parser {
        let mut parser = vt100::Parser::new(rows, 40, 0);
        parser.process(prelude.as_bytes());
        parser.process(region_transition(old, new, rows).as_bytes());
        parser
    }

    fn row(parser: &vt100::Parser, r: u16) -> String {
        parser
            .screen()
            .contents_between(r, 0, r, 40)
            .trim_end()
            .to_string()
    }

    #[test]
    fn a_transition_under_a_short_conversation_moves_nothing() {
        // Banner on rows 1-3, cursor on row 4: the region shrinks from the
        // initial rows-1 (23) to 18 (a 6-row pinned area on 24 rows). Nothing
        // is in the way, so nothing scrolls and the cursor stays put — the
        // next output lands right under the banner, not at the bottom.
        let p = transition_screen("one\r\ntwo\r\nthree\r\n", 23, 18, 24);
        assert_eq!(row(&p, 0), "one");
        assert_eq!(row(&p, 2), "three");
        assert_eq!(p.screen().cursor_position(), (3, 0));
        assert!(p.screen().contents().trim().lines().count() == 3);
    }

    #[test]
    fn a_transition_scrolls_exactly_the_rows_in_the_way() {
        // The conversation runs down to row 22 with the cursor on 23 (the
        // initial full-height region's bottom): reserving 6 rows (bottom 18)
        // must lift the whole thing by 5 — the last line ends on row 17, the
        // cursor on 18 — losing nothing and leaving no blank rows below it.
        let mut prelude = String::new();
        for i in 1..=22 {
            prelude.push_str(&format!("line {i}\r\n"));
        }
        let p = transition_screen(&prelude, 23, 18, 24);
        assert_eq!(row(&p, 16), "line 22");
        assert_eq!(row(&p, 0), "line 6");
        assert_eq!(p.screen().cursor_position(), (17, 0));
        for r in 17..24 {
            assert_eq!(row(&p, r), "", "row {r} must be clear");
        }
    }

    #[test]
    fn later_shrinks_do_not_nudge_a_conversation_that_fits() {
        // The composer grew a line (18 -> 17) while the output sits on rows
        // 1-5. A keystroke must never scroll the conversation.
        let p = transition_screen("\x1b[1;18ra\r\nb\r\nc\r\n", 18, 17, 24);
        assert_eq!(row(&p, 0), "a");
        assert_eq!(row(&p, 2), "c");
        assert_eq!(p.screen().cursor_position(), (3, 0));
        // The row that left the region is cleared and the region re-carved.
        let seq = region_transition(18, 17, 24);
        assert!(
            seq.contains("\x1b[18;1H\x1b[2K"),
            "clears moved row: {seq:?}"
        );
        assert!(seq.contains("\x1b[1;17r"), "re-carve: {seq:?}");
    }

    #[test]
    fn later_shrinks_lift_a_full_region_instead_of_painting_over_it() {
        // Same shrink, but the region is full: the last line sits on the row
        // being claimed. It must move up one, not vanish under the composer.
        let mut prelude = String::from("\x1b[1;18r\x1b[18;1H");
        for i in 1..=18 {
            prelude.push_str(&format!("line {i}\r\n"));
        }
        let p = transition_screen(&prelude, 18, 17, 24);
        assert_eq!(row(&p, 15), "line 18");
        assert_eq!(row(&p, 16), "");
        assert_eq!(p.screen().cursor_position(), (16, 0));
    }

    #[test]
    fn a_growing_region_clears_the_rows_it_takes_back() {
        // The reserved area shrinks (region grows back, 17 -> 18): the row that
        // rejoins the region held chrome and must come back blank.
        let p = transition_screen("\x1b[1;17r\x1b[18;1Hchrome\x1b[3;1H", 17, 18, 24);
        assert_eq!(row(&p, 17), "");
        assert_eq!(p.screen().cursor_position(), (2, 0));
        assert!(region_transition(17, 18, 24).contains("\x1b[1;18r"));
    }

    #[test]
    fn ctrl_c_notice_pins_to_the_bottom_row_and_clears() {
        let mut l = plain_live(60, 24);
        let before = l.pinned_plan().rows();
        l.set_notice(Some("ctrl-c again leaves · d labels".into()));
        let plan = l.pinned_plan();
        // One extra reserved row, and it is the very last line painted.
        assert_eq!(plan.rows(), before + 1);
        let last = plan.sections.last().unwrap();
        assert_eq!(last.kind, SectionKind::Composer);
        assert_eq!(last.lines.last().unwrap(), "ctrl-c again leaves · d labels");
        l.set_notice(None);
        assert_eq!(l.pinned_plan().rows(), before);
    }

    #[test]
    fn pinned_plan_orders_sections_and_reserves_exactly_what_it_paints() {
        let mut l = plain_live(60, 24);
        l.sync_queue(&["a".into(), "b".into(), "c".into()]);
        l.status_lines = vec!["ctx".into(), "used".into()];
        l.compression_line = Some("comp".into());

        let plan = l.pinned_plan();
        // spacer(0) + fleet(0) + compression(1) + status(2) + divider(1) +
        // queue(3 items + 2 frame) + composer(1) = 10 reserved rows.
        assert_eq!(plan.rows(), 10);
        assert_eq!(plan.region_bottom(24), 14);

        // The layout order is fixed; every section is present (possibly empty).
        let kinds: Vec<_> = plan.sections.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SectionKind::Spacer,
                SectionKind::Fleet,
                SectionKind::ToolCard,
                SectionKind::Compression,
                SectionKind::Status,
                SectionKind::Divider,
                SectionKind::Queue,
                SectionKind::Composer,
            ]
        );
    }

    #[test]
    fn fleet_block_is_truncated_to_keep_an_output_row_on_a_short_terminal() {
        use crate::fleet_ui::{FleetHub, FleetState};

        let mut l = plain_live(40, 10);
        // A private hub so this test can't race others over the global one.
        let hub = std::sync::Arc::new(FleetHub::default());
        let labels: Vec<String> = (0..6).map(|i| format!("lane {i}")).collect();
        hub.install(FleetState::new(&labels, None));
        l.fleet = hub;

        let plan = l.pinned_plan();
        let fleet = plan.section(SectionKind::Fleet).unwrap();
        // fixed = spacer(1) + divider(1) + composer(1) = 3, so the fleet gets at
        // most rows - fixed - 1 = 6 lines — the block was trimmed, and at least
        // one row of output survives above the pinned area.
        assert!(
            fleet.lines.len() <= 6,
            "fleet must be truncated: {} lines",
            fleet.lines.len()
        );
        assert!(plan.rows() < 10, "reserved must leave an output row");
        assert_eq!(plan.region_bottom(10), 10 - plan.rows());
    }

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
