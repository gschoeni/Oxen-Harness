//! Live rendering of an agent turn: a trail-status spinner while the model
//! thinks or a tool runs, assistant text streamed through the Markdown
//! renderer, and themed tool lines. Shared by the REPL prompt flow and the
//! loop runner so both surfaces feel identical.
//!
//! Per-event formatting lives in [`crate::event_lines`] (shared with the live
//! composer); this renderer only maps each [`Cue`] onto cooked-mode stdout
//! and the threaded spinner.

use harness_agent::AgentEvent;

use crate::event_lines::{cue_for, Cue, NextSpinner};
use crate::markdown;
use crate::theme::{self, Ui};

/// Renders one turn's live progress.
pub(crate) struct TurnRenderer {
    ui: Ui,
    spinner: Option<theme::Spinner>,
    md: Option<markdown::MarkdownStream<std::io::Stdout>>,
    /// Set when a `web_search` call failed for a missing Brave API key, so the
    /// caller can offer a key prompt once the turn ends.
    needs_brave_key: bool,
}

impl TurnRenderer {
    pub(crate) fn new(ui: Ui) -> Self {
        Self {
            ui,
            spinner: None,
            md: None,
            needs_brave_key: false,
        }
    }

    /// Whether the turn hit a web search with no API key configured.
    pub(crate) fn needs_brave_key(&self) -> bool {
        self.needs_brave_key
    }

    /// Flush and drop the active Markdown segment, if any.
    fn end_markdown(&mut self) {
        if let Some(mut md) = self.md.take() {
            md.finish();
        }
    }

    pub(crate) fn begin_thinking(&mut self) {
        self.stop_spinner();
        self.spinner = Some(theme::Spinner::start(&self.ui, self.ui.thinking()));
    }

    fn begin_working(&mut self, tool: &str, target: Option<String>) {
        self.stop_spinner();
        self.spinner = Some(theme::Spinner::start_with_target(
            &self.ui,
            self.ui.tool_verbs(tool),
            target,
        ));
    }

    fn stop_spinner(&mut self) {
        if let Some(s) = self.spinner.take() {
            s.stop();
        }
    }

    pub(crate) fn on_event(&mut self, event: &AgentEvent) {
        match cue_for(&self.ui, event) {
            Cue::Token(t) => {
                if self.md.is_none() {
                    self.stop_spinner();
                    println!();
                    self.md = Some(markdown::MarkdownStream::new(
                        self.ui.clone(),
                        std::io::stdout(),
                    ));
                }
                if let Some(md) = self.md.as_mut() {
                    md.push(&t);
                }
            }
            Cue::Block { lines, then } => {
                self.stop_spinner();
                self.end_markdown();
                for line in lines {
                    println!("{line}");
                }
                self.begin(then);
            }
            // The fleet's cooked-mode painter owns the terminal (and the keys)
            // while it runs; keep the spinner out of its way.
            Cue::FleetStart { lines } => {
                self.stop_spinner();
                self.end_markdown();
                for line in lines {
                    println!("{line}");
                }
            }
            // The picker draws its own UI in cooked mode; suppress all chrome
            // while it owns the screen (it prints the chosen answer itself).
            Cue::AskUserStart => {
                self.stop_spinner();
                self.end_markdown();
            }
            Cue::AskUserEnd => {
                self.stop_spinner();
                self.begin_thinking();
            }
            // The approval picker is about to own the terminal; stop the
            // spinner so they don't fight over the line.
            Cue::ApprovalPending => self.stop_spinner(),
            Cue::ApprovalResolved { line } => {
                println!("{line}");
                self.begin_thinking();
            }
            Cue::BraveKeyMissing { line } => {
                self.stop_spinner();
                self.needs_brave_key = true;
                println!("{line}");
                self.begin_thinking();
            }
            // Usage is surfaced in the banner/status, not inline during a turn.
            Cue::Usage { .. } => {}
            Cue::Compression { scroll_line, .. } => {
                self.stop_spinner();
                println!("{scroll_line}");
                self.begin_thinking();
            }
            Cue::Ignore => {}
        }
    }

    fn begin(&mut self, next: NextSpinner) {
        match next {
            NextSpinner::Thinking => self.begin_thinking(),
            NextSpinner::Working { tool, target } => self.begin_working(&tool, target),
        }
    }

    pub(crate) fn finish(&mut self) {
        self.stop_spinner();
        self.end_markdown();
    }
}

/// Collapse newlines and cap a string to `max` terminal cells, adding an
/// ellipsis. Measured in cells (not chars) so previews with CJK/emoji fit the
/// frames they're padded into.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    crate::width::ellipsize(&s.replace('\n', " "), max)
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_collapses_newlines_and_caps_length() {
        assert_eq!(truncate("a\nb", 10), "a b");
        let long = "x".repeat(50);
        let out = truncate(&long, 10);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 11);
    }
}
