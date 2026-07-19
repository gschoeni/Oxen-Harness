//! Rendering the agent's streamed events into the scroll region.
//!
//! Per-event formatting lives in [`crate::event_lines`], shared with the
//! classic renderer ([`crate::render::TurnRenderer`]) so the two surfaces
//! cannot drift. These `impl Live` methods map each [`Cue`] onto the live
//! machinery: the scroll region, the tail spinner, the pinned meters, and the
//! picker screen hand-off.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use harness_agent::AgentEvent;

use crate::event_lines::{cue_for, Cue, NextSpinner};
use crate::markdown::MarkdownStream;
use crate::theme::LiveSpinner;

use super::terminal::CrlfWriter;
use super::Live;

impl Live {
    // --- turn lifecycle ----------------------------------------------------

    /// Reset render state, snapshot the queue, and start the thinking spinner
    /// for a fresh turn.
    pub(super) fn begin_turn(&mut self, items: &[String]) {
        self.sync_queue(items);
        self.md = None;
        self.begin_thinking();
        self.render();
    }

    /// Flush any open Markdown and stop the spinner at the end of a turn.
    pub(super) fn finish(&mut self) {
        self.stop_spinner();
        self.end_markdown();
    }

    pub(super) fn begin_thinking(&mut self) {
        self.spinner = LiveSpinner::new(&self.ui, self.ui.thinking());
        self.sync_tail();
    }

    /// Start the indicator shown *while assistant text streams in*, so a pause
    /// between tokens (or a long, not-yet-newline-terminated line like a code
    /// block) keeps animating with a running timer instead of looking frozen.
    fn begin_streaming(&mut self) {
        self.spinner = LiveSpinner::new(&self.ui, self.ui.writing());
        self.sync_tail();
    }

    fn begin_working(&mut self, tool: &str, target: Option<String>) {
        self.spinner = LiveSpinner::with_target(&self.ui, self.ui.tool_verbs(tool), target);
        self.sync_tail();
    }

    fn stop_spinner(&mut self) {
        self.spinner = None;
        self.region.set_tail(None);
    }

    fn end_markdown(&mut self) {
        if let Some(mut md) = self.md.take() {
            md.finish();
        }
    }

    // --- event rendering (cues from crate::event_lines) --------------------

    pub(super) fn on_event(&mut self, event: &AgentEvent, paused: &Arc<AtomicBool>) {
        let cue = cue_for(&self.ui, event);
        // Tokens arrive at streaming rate and change nothing in the pinned
        // chrome — their text lands instantly through the markdown writer, and
        // the paint they request is flushed by the turn loop's ~110ms ticker.
        // Flushing per token would repaint the whole pinned area (composer
        // wrap, queue box, divider, meters) hundreds of times a second.
        let defer_flush = matches!(cue, Cue::Token(_));
        match cue {
            Cue::Token(t) => self.on_token(&t),
            Cue::Block { lines, then } => {
                self.stop_spinner();
                self.end_markdown();
                if !lines.is_empty() {
                    self.write_region(&format!("{}\n", lines.join("\n")));
                }
                self.begin(then);
                self.request_paint();
            }
            // The fleet paints per-lane activity in the pinned block; the
            // between-steps thinking indicator rides alongside it.
            Cue::FleetStart { lines } => {
                self.stop_spinner();
                self.end_markdown();
                self.write_region(&format!("{}\n", lines.join("\n")));
                self.begin_thinking();
                self.request_paint();
            }
            // An interactive tool needs the screen: the ask-user picker and
            // the approval prompt use the same hand-off.
            Cue::AskUserStart | Cue::ApprovalPending => self.hand_off_screen(paused),
            Cue::AskUserEnd => {
                self.stop_spinner();
                self.reclaim_screen();
            }
            Cue::ApprovalResolved { line } => {
                self.reclaim_screen();
                self.print_line(&line);
            }
            Cue::BraveKeyMissing { line } => {
                self.stop_spinner();
                self.needs_brave_key = true;
                self.write_region(&format!("{line}\n"));
                self.begin_thinking();
                self.request_paint();
            }
            // Rebuild the pinned context trailer from the live figures so the
            // tokens-used count and running price climb *during* the turn (each
            // tool-loop iteration re-sends a larger context), not only when it
            // ends. Chrome, not conversation: update it in place, no scrollback.
            Cue::Usage {
                context_tokens,
                prompt_tokens_used,
                completion_tokens_used,
            } => self.on_usage(context_tokens, prompt_tokens_used, completion_tokens_used),
            // The savings are chrome, not conversation: update the pinned line
            // (above the context meter) in place — never scroll a line into
            // the transcript between tool output and the spinner.
            Cue::Compression { pinned_line, .. } => {
                self.compression_line = Some(pinned_line);
                self.request_paint();
            }
            Cue::Ignore => {}
        }
        // The one paint per (non-token) event: handlers above only mark the
        // pinned area dirty. (Events arrive many-per-poll through the turn
        // future's synchronous callback, so the flush lives here, not in the
        // select loop.)
        if !defer_flush {
            self.flush_paint();
        }
    }

    fn begin(&mut self, next: NextSpinner) {
        match next {
            NextSpinner::Thinking => self.begin_thinking(),
            NextSpinner::Working { tool, target } => self.begin_working(&tool, target),
        }
    }

    fn on_token(&mut self, t: &str) {
        if self.md.is_none() {
            self.stop_spinner();
            self.write_region("\n");
            self.md = Some(MarkdownStream::new(
                self.ui.clone(),
                CrlfWriter::over(self.out.clone()),
            ));
            // Keep a live indicator going *below* the streamed text so a pause
            // mid-response (or a long, not-yet-complete line such as a code block)
            // never looks frozen.
            self.begin_streaming();
        }
        // Newly completed markdown lines land where the spinner sits; the tail
        // is lifted for the write and redrawn on the fresh line below. (The
        // markdown stream owns its writer, so the lift/redraw pair brackets the
        // push here instead of going through `RegionWriter::write`.)
        self.region.lift_tail();
        if let Some(md) = self.md.as_mut() {
            md.push(t);
        }
        self.region.redraw_tail();
        self.request_paint();
    }

    /// A mid-turn usage update: rebuild the pinned context trailer from the live
    /// figures so tokens-used and the running price climb in real time. Only
    /// touches the pinned meter (no scrollback), then repaints the bottom area.
    fn on_usage(
        &mut self,
        context_tokens: usize,
        prompt_tokens_used: usize,
        completion_tokens_used: usize,
    ) {
        self.status_lines = crate::turn::context_usage_lines_from(
            &self.ui,
            &self.model,
            context_tokens,
            self.context_window,
            prompt_tokens_used,
            completion_tokens_used,
        );
        self.request_paint();
    }

    // --- spinner -----------------------------------------------------------

    pub(super) fn tick_spinner(&mut self) {
        if self.suspended() {
            return;
        }
        if let Some(sp) = self.spinner.as_mut() {
            sp.tick();
        }
        self.sync_tail();
    }

    /// Push the spinner's current frame to the region tail (and repaint the
    /// composer). The tail is the single owner of the spinner's on-screen
    /// line — nothing else writes raw erase sequences for it.
    pub(super) fn sync_tail(&mut self) {
        if self.spinner.is_some() {
            let line = self.spinner.as_ref().map(|sp| sp.line());
            self.region.set_tail(line);
            self.request_paint();
        }
    }

    // --- writing into the scroll region ------------------------------------

    /// Write text into the scroll region (newlines become `\r\n`), where the
    /// output cursor lives. The region tail (the spinner) is lifted for the
    /// write and redrawn below, so mid-turn announcements (steering, retry
    /// notices) can never land on the spinner's line.
    fn write_region(&mut self, text: &str) {
        self.region.write(text);
    }

    /// Print a complete status line into the region, then redraw the composer.
    /// Used for the post-turn context-usage trailer and drain announcements.
    pub(super) fn print_line(&mut self, line: &str) {
        self.write_region(&format!("{line}\n"));
        self.request_paint();
    }
}
