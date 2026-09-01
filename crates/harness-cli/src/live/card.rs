//! The live tool card: a pinned tail of a running command's output.
//!
//! While `run_shell` waits on a command, its stdout/stderr stream in as
//! [`AgentEvent::ToolProgress`] chunks and the last few lines paint here,
//! directly under the conversation and above the meters — so a build or a
//! test run is visibly *doing something* instead of hiding behind a spinner
//! for a minute. When the call ends the card vanishes and the sealed
//! one-line summary lands in the scrollback; Ctrl+O prints the full result.
//!
//! [`AgentEvent::ToolProgress`]: harness_agent::AgentEvent::ToolProgress

use std::collections::VecDeque;

use crate::theme::Ui;
use crate::width::fit;

/// Lines of output the card keeps (and shows) at once.
pub(super) const CARD_TAIL_LINES: usize = 8;
/// Results kept for Ctrl+O, newest last.
pub(super) const KEPT_RESULTS: usize = 10;
/// The most lines Ctrl+O prints for one result.
const EXPAND_MAX_LINES: usize = 200;

/// A running tool call whose output is streaming.
pub(super) struct ToolCard {
    pub(super) call_id: String,
    tail: VecDeque<String>,
    /// Text since the last newline (a progress bar, a prompt, a half line).
    partial: String,
    /// Lines that scrolled out of the tail.
    dropped: usize,
}

impl ToolCard {
    pub(super) fn new(call_id: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            tail: VecDeque::with_capacity(CARD_TAIL_LINES),
            partial: String::new(),
            dropped: 0,
        }
    }

    /// Absorb a chunk of output. A carriage return without a newline (the
    /// way progress bars redraw) restarts the partial line instead of
    /// stacking up.
    pub(super) fn push(&mut self, chunk: &str) {
        for piece in chunk.split_inclusive('\n') {
            if let Some(line) = piece.strip_suffix('\n') {
                self.partial.push_str(line);
                let line = std::mem::take(&mut self.partial);
                self.push_line(&line);
            } else {
                self.partial.push_str(piece);
            }
            if let Some(after_cr) = self.partial.rsplit('\r').next() {
                if after_cr.len() != self.partial.len() {
                    self.partial = after_cr.to_string();
                }
            }
        }
    }

    fn push_line(&mut self, line: &str) {
        let line = line.rsplit('\r').next().unwrap_or(line);
        if self.tail.len() == CARD_TAIL_LINES {
            self.tail.pop_front();
            self.dropped += 1;
        }
        self.tail.push_back(clean(line));
    }

    /// The card's rows at `cols` wide: a faint left rule, the tail, and the
    /// partial line. Empty until the first byte arrives, so a quiet command
    /// takes no space.
    pub(super) fn lines(&self, ui: &Ui, cols: usize) -> Vec<String> {
        if self.tail.is_empty() && self.partial.is_empty() {
            return Vec::new();
        }
        let width = cols.saturating_sub(4).max(8);
        let mut out = Vec::with_capacity(CARD_TAIL_LINES + 2);
        if self.dropped > 0 {
            out.push(format!(
                "  {} {}",
                ui.dim("│"),
                ui.dim(&format!("… {} earlier lines", self.dropped))
            ));
        }
        for line in &self.tail {
            out.push(format!("  {} {}", ui.dim("│"), ui.dim(&fit(line, width))));
        }
        if !self.partial.is_empty() {
            out.push(format!(
                "  {} {}",
                ui.dim("│"),
                ui.dim(&fit(&clean(&self.partial), width))
            ));
        }
        out
    }
}

/// Tabs to spaces and control characters out, so a raw byte can't move
/// the cursor around the pinned area.
fn clean(line: &str) -> String {
    line.replace('\t', "    ")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

/// One sealed tool result, kept for Ctrl+O.
pub(super) struct KeptResult {
    pub(super) name: String,
    pub(super) result: String,
}

/// The block Ctrl+O prints for a kept result.
pub(super) fn expanded_block(ui: &Ui, kept: &KeptResult, cols: usize) -> Vec<String> {
    let width = cols.saturating_sub(4).max(8);
    let mut out = vec![format!(
        "  {} {}",
        ui.dim("┌─"),
        ui.accent(&format!("{} result", kept.name))
    )];
    let total = kept.result.lines().count();
    for line in kept.result.lines().take(EXPAND_MAX_LINES) {
        out.push(format!(
            "  {} {}",
            ui.dim("│"),
            ui.cream(&fit(&clean(line), width))
        ));
    }
    if total > EXPAND_MAX_LINES {
        out.push(format!(
            "  {} {}",
            ui.dim("│"),
            ui.dim(&format!("… {} more lines", total - EXPAND_MAX_LINES))
        ));
    }
    out.push(format!("  {}", ui.dim("└─")));
    out
}

/// The sealed one-line summary for a tool result: the first line, and how
/// much more there is. `run_shell` results lead with their exit code.
pub(crate) fn summarize_result(result: &str, max: usize) -> String {
    let mut lines = result.lines().filter(|l| !l.trim().is_empty());
    let Some(first) = lines.next() else {
        return String::new();
    };
    let rest = lines.count();
    let first = first.trim_end();
    if rest == 0 {
        return crate::render::truncate(first, max);
    }
    let note = format!(" (+{rest} lines · ctrl+o expands)");
    let room = max.saturating_sub(note.len()).max(24);
    format!("{}{note}", crate::render::truncate(first, room))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_keeps_a_bounded_tail_and_counts_what_scrolled_off() {
        let mut card = ToolCard::new("c");
        for i in 0..12 {
            card.push(&format!("line {i}\n"));
        }
        card.push("half");
        let ui = Ui::plain();
        let lines = card.lines(&ui, 80);
        assert_eq!(lines.len(), CARD_TAIL_LINES + 2, "{lines:?}");
        assert!(lines[0].contains("… 4 earlier lines"));
        assert!(lines[1].contains("line 4"));
        assert!(lines.last().unwrap().contains("half"));
    }

    #[test]
    fn a_carriage_return_redraws_the_partial_line_instead_of_stacking() {
        let mut card = ToolCard::new("c");
        card.push("10%\r20%\r30%");
        let lines = card.lines(&Ui::plain(), 80);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("30%") && !lines[0].contains("10%"),
            "{lines:?}"
        );
        card.push("\n");
        let lines = card.lines(&Ui::plain(), 80);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("30%"));
    }

    #[test]
    fn an_empty_card_takes_no_rows() {
        assert!(ToolCard::new("c").lines(&Ui::plain(), 80).is_empty());
    }

    #[test]
    fn summaries_lead_with_the_first_line_and_count_the_rest() {
        assert_eq!(summarize_result("ok", 140), "ok");
        let s = summarize_result("exit_code: 0\n--- stdout ---\na\nb\n", 140);
        assert!(s.starts_with("exit_code: 0 (+3 lines"), "{s}");
        assert!(s.contains("ctrl+o"));
        assert_eq!(summarize_result("\n\n", 140), "");
    }
}
