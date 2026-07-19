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

/// The escape sequence to move the DECSTBM scroll region from `old_bottom` to
/// `new_bottom` (rows `1..=bottom`), preserving already-printed output.
///
/// On the *first* paint the rows the pinned area is about to claim may still
/// hold real conversation output — most visibly the tail of the opening banner,
/// before anything has scrolled. When the region shrinks then, scroll it up by
/// the rows it's losing so that content is pushed up into the rows we keep,
/// instead of being painted over. A line feed at the region's bottom scrolls
/// only *within* the region, so this stays bounded to the output area. Only on
/// the first paint: later shrinks are the blank composer/queue area growing,
/// where scrolling would wrongly nudge the conversation up.
///
/// On an incremental change (not a forced re-carve) the rows that move between
/// the output region and the reserved area are cleared so no stale text lingers.
fn region_transition(
    first_paint: bool,
    force_region: bool,
    old_bottom: u16,
    new_bottom: u16,
    rows: u16,
) -> String {
    let mut buf = String::new();
    if first_paint && new_bottom < old_bottom {
        let lift = old_bottom - new_bottom;
        buf.push_str(&format!("\x1b[{old_bottom};1H"));
        buf.push_str(&"\n".repeat(lift as usize));
    }
    if !force_region {
        let lo = old_bottom.min(new_bottom) + 1;
        for r in lo..=rows {
            buf.push_str(&format!("\x1b[{r};1H\x1b[2K"));
        }
    }
    // Re-carve the region and park the output cursor at its new bottom.
    buf.push_str(&format!("\x1b[1;{new_bottom}r\x1b[{new_bottom};1H"));
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
        let mut meter_lines: Vec<String> = Vec::new();
        let compression_lines: Vec<String> = self.compression_line.iter().cloned().collect();
        meter_lines.extend(self.status_lines.iter().cloned());
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
        let composer = Section::new(SectionKind::Composer, self.composer_box_lines());
        let fixed = (SPACER_ROWS
            + compression_lines.len()
            + meter_lines.len()
            + divider.len()
            + queue_lines.len()
            + composer.lines.len()) as u16;
        let fleet_budget = self.rows.saturating_sub(fixed + 1) as usize;
        let mut fleet_lines = self.fleet_lines();
        fleet_lines.truncate(fleet_budget);

        sections.push(Section::new(SectionKind::Fleet, fleet_lines));
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
                self.first_paint,
                force_region,
                self.region_bottom,
                new_bottom,
                self.rows,
            ));
            self.region_bottom = new_bottom;
        }
        self.first_paint = false;

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
            Mode::Compose => "↑ edit queued",
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

    #[test]
    fn first_paint_scrolls_output_up_to_preserve_the_banner() {
        // On the first paint the region shrinks from the terminal's initial
        // rows-1 (23) down to the real output bottom (say 18, reserving a
        // 6-row pinned area on a 24-row screen). The banner tail sits in the
        // rows being claimed, so the region must scroll up by the lost rows
        // (23 - 18 = 5) before re-carving — otherwise the pinned area paints
        // over it.
        let seq = region_transition(true, false, 23, 18, 24);
        // Cursor to the old region bottom, then 5 line feeds to scroll.
        assert!(
            seq.contains("\x1b[23;1H"),
            "must park at old bottom: {seq:?}"
        );
        assert!(
            seq.contains(&"\n".repeat(5)),
            "must scroll up by the lost rows: {seq:?}"
        );
        // The scroll must precede the re-carve so it happens under the old region.
        let scroll_at = seq.find('\n').unwrap();
        let recarve_at = seq.find("\x1b[1;18r").unwrap();
        assert!(scroll_at < recarve_at, "scroll before re-carve: {seq:?}");
        // And the region is re-carved to the new bottom, cursor parked there.
        assert!(seq.contains("\x1b[1;18r\x1b[18;1H"), "re-carve: {seq:?}");
    }

    #[test]
    fn later_shrinks_do_not_scroll_the_conversation() {
        // A later shrink (composer grew a line: 18 -> 17) must NOT scroll the
        // output — those claimed rows are the blank spacer/composer area, and
        // scrolling would nudge the conversation up on every keystroke.
        let seq = region_transition(false, false, 18, 17, 24);
        assert!(
            !seq.contains('\n'),
            "must not scroll on a later shrink: {seq:?}"
        );
        // It still clears the row that moved out of the region and re-carves.
        assert!(
            seq.contains("\x1b[18;1H\x1b[2K"),
            "clears moved row: {seq:?}"
        );
        assert!(seq.contains("\x1b[1;17r\x1b[17;1H"), "re-carve: {seq:?}");
    }

    #[test]
    fn a_growing_region_never_scrolls() {
        // When the reserved area shrinks (region grows back, 17 -> 18), there is
        // nothing to preserve by scrolling — just clear and re-carve.
        let seq = region_transition(true, false, 17, 18, 24);
        assert!(!seq.contains('\n'), "no scroll when growing: {seq:?}");
        assert!(seq.contains("\x1b[1;18r"), "re-carve: {seq:?}");
    }

    #[test]
    fn forced_region_only_recarves() {
        // A forced re-issue (after resize/picker) clears nothing and doesn't
        // scroll — it just re-establishes the region.
        let seq = region_transition(false, true, 18, 18, 24);
        assert!(!seq.contains("\x1b[2K"), "no clears when forced: {seq:?}");
        assert!(!seq.contains('\n'), "no scroll when forced: {seq:?}");
        assert!(seq.contains("\x1b[1;18r\x1b[18;1H"), "re-carve: {seq:?}");
    }

    #[test]
    fn pinned_plan_orders_sections_and_reserves_exactly_what_it_paints() {
        let mut l = plain_live(60, 24);
        l.sync_queue(&["a".into(), "b".into(), "c".into()]);
        l.status_lines = vec!["ctx".into(), "used".into()];
        l.compression_line = Some("comp".into());

        let plan = l.pinned_plan();
        // spacer(1) + fleet(0) + compression(1) + status(2) + divider(1) +
        // queue(3 items + 2 frame) + composer(1) = 11 reserved rows.
        assert_eq!(plan.rows(), 11);
        assert_eq!(plan.region_bottom(24), 13);

        // The layout order is fixed; every section is present (possibly empty).
        let kinds: Vec<_> = plan.sections.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SectionKind::Spacer,
                SectionKind::Fleet,
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
