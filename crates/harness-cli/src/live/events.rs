//! Rendering the agent's streamed events into the scroll region.
//!
//! These `impl Live` methods mirror [`crate::render::TurnRenderer`] (the
//! classic, non-live renderer) but write through the scroll region and keep
//! the spinner + pinned meters in play: assistant tokens stream as Markdown,
//! tool calls print transparent activity lines (diffs for file edits, inline
//! previews for canvases), and interactive tools temporarily take the screen.

use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use harness_agent::AgentEvent;

use crate::markdown::MarkdownStream;
use crate::render::truncate;
use crate::theme::LiveSpinner;

use super::terminal::CrlfWriter;
use super::turn::tool_target;
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

    fn begin_thinking(&mut self) {
        self.stop_spinner();
        self.spinner = LiveSpinner::new(&self.ui, self.ui.thinking());
        self.draw_spinner();
    }

    /// Start the indicator shown *while assistant text streams in*, so a pause
    /// between tokens (or a long, not-yet-newline-terminated line like a code
    /// block) keeps animating with a running timer instead of looking frozen.
    fn begin_streaming(&mut self) {
        self.stop_spinner();
        self.spinner = LiveSpinner::new(&self.ui, self.ui.writing());
        self.draw_spinner();
    }

    fn begin_working(&mut self, tool: &str, target: Option<String>) {
        self.stop_spinner();
        self.spinner = LiveSpinner::with_target(&self.ui, self.ui.tool_verbs(tool), target);
        self.draw_spinner();
    }

    /// Erase the spinner's current line without dropping the spinner, so newly
    /// completed streamed output can be written where the spinner sat (it's then
    /// redrawn one line below via [`Live::draw_spinner`]).
    fn clear_spinner_line(&mut self) {
        if self.spinner.is_some() && !self.suspended {
            let _ = write!(self.out, "\r\x1b[K");
            let _ = self.out.flush();
        }
    }

    fn stop_spinner(&mut self) {
        if self.spinner.take().is_some() && !self.suspended {
            let _ = write!(self.out, "\r\x1b[K");
            let _ = self.out.flush();
        }
    }

    fn end_markdown(&mut self) {
        if let Some(mut md) = self.md.take() {
            md.finish();
        }
    }

    // --- event rendering (mirrors render::TurnRenderer) --------------------

    pub(super) fn on_event(&mut self, event: &AgentEvent, paused: &Arc<AtomicBool>) {
        match event {
            AgentEvent::Token(t) => self.on_token(t),
            // The model started writing a canvas; surface it while its content
            // streams in (the full preview prints on ToolStart).
            AgentEvent::ToolPending { name } if name == harness_tools::CANVAS_TOOL => {
                self.on_canvas_pending(name)
            }
            AgentEvent::ToolPending { .. } => {}
            AgentEvent::ToolStart { name, arguments } => {
                self.on_tool_start(name, arguments, paused)
            }
            AgentEvent::ToolEnd { name, result } => self.on_tool_end(name, result, paused),
            // Usage is surfaced in the banner/status, not inline during a turn.
            AgentEvent::Usage { .. } => {}
            AgentEvent::Compacted { detail } => self.on_compacted(detail),
            AgentEvent::Compression {
                mode,
                saved_tokens,
                total_saved_tokens,
                ..
            } => self.on_compression(mode, *saved_tokens, *total_saved_tokens),
            // A transient provider/network failure being retried with backoff;
            // show it so the pause reads as a hiccup, not a hang.
            AgentEvent::Retrying {
                attempt,
                max_attempts,
                delay_ms,
                error,
            } => self.on_retrying(*attempt, *max_attempts, *delay_ms, error),
            // Streaming tool-argument fragments drive the desktop UI only.
            AgentEvent::ToolDelta { .. } => {}
        }
    }

    fn on_retrying(&mut self, attempt: u32, max_attempts: u32, delay_ms: u64, error: &str) {
        self.stop_spinner();
        self.end_markdown();
        let line = format!(
            "  {} {}",
            self.ui.red("⚠"),
            self.ui.dim(&crate::turn::retry_notice(
                attempt,
                max_attempts,
                delay_ms,
                error
            )),
        );
        self.write_region(&format!("{line}\n"));
        self.begin_thinking();
        self.render_composer();
    }

    fn on_token(&mut self, t: &str) {
        if self.md.is_none() {
            self.stop_spinner();
            self.write_region("\n");
            self.md = Some(MarkdownStream::new(self.ui.clone(), CrlfWriter::new()));
            // Keep a live indicator going *below* the streamed text so a pause
            // mid-response (or a long, not-yet-complete line such as a code block)
            // never looks frozen.
            self.begin_streaming();
        } else {
            // Clear the trailing spinner line before emitting newly completed
            // markdown lines, so output and spinner don't collide.
            self.clear_spinner_line();
        }
        if let Some(md) = self.md.as_mut() {
            md.push(t);
        }
        // Redraw the spinner on the fresh line just below the output.
        self.draw_spinner();
        self.render_composer();
    }

    fn on_canvas_pending(&mut self, name: &str) {
        self.stop_spinner();
        self.end_markdown();
        self.write_region(&format!(
            "  {} {}\n",
            self.ui.green("📄"),
            self.ui.dim("writing canvas…")
        ));
        self.begin_working(name, None);
        self.render_composer();
    }

    fn on_tool_start(&mut self, name: &str, arguments: &str, paused: &Arc<AtomicBool>) {
        self.stop_spinner();
        self.end_markdown();
        // The picker draws its own UI and reads keys, so hand the screen over to
        // it instead of printing a tool line + spinner.
        if name == harness_tools::ASK_USER_TOOL {
            self.suspend(paused);
            return;
        }
        let target = tool_target(arguments);
        // For a canvas, preview the document inline; the result line then reports
        // the saved path / browser open.
        if name == harness_tools::CANVAS_TOOL {
            if let Some(block) = crate::canvas::render_canvas_block(&self.ui, arguments) {
                self.write_region(&format!("{}\n", block.join("\n")));
            }
            self.begin_working(name, target);
            self.render_composer();
            return;
        }
        // For file writes/edits, show a colored diff instead of the generic
        // one-line tool preview.
        if let Some(block) = crate::diff::render_file_change(&self.ui, name, arguments) {
            self.write_region(&format!("{}\n", block.join("\n")));
        } else {
            let verbs = self.ui.tool_verbs(name);
            let verb = verbs.first().map(String::as_str).unwrap_or("Working");
            let line = format!(
                "  {} {}  {}",
                self.ui.green("◆"),
                self.ui.accent(verb),
                self.ui
                    .dim(&format!("{name}({})", truncate(arguments, 100))),
            );
            self.write_region(&format!("{line}\n"));
        }
        self.begin_working(name, target);
        self.render_composer();
    }

    fn on_tool_end(&mut self, name: &str, result: &str, paused: &Arc<AtomicBool>) {
        self.stop_spinner();
        if name == harness_tools::ASK_USER_TOOL {
            self.resume(paused);
            self.begin_thinking();
            // Repaint the full list: the picker drew over the screen.
            self.render_forcing_region();
            return;
        }
        // Web search with no key: flag it for a prompt once the composer hands
        // back to cooked mode, and show a friendlier line.
        if name == harness_tools::WEB_SEARCH_TOOL
            && result.contains(harness_tools::web::WEB_SEARCH_NO_KEY)
        {
            self.needs_brave_key = true;
            let line = format!(
                "  {} {}",
                self.ui.brown("└─"),
                self.ui
                    .dim("no Brave API key — you'll be prompted to add one below"),
            );
            self.write_region(&format!("{line}\n"));
            self.begin_thinking();
            self.render_composer();
            return;
        }
        let line = format!(
            "  {} {}",
            self.ui.brown("└─"),
            self.ui.dim(&truncate(result, 140)),
        );
        self.write_region(&format!("{line}\n"));
        self.begin_thinking();
        self.render_composer();
    }

    fn on_compacted(&mut self, detail: &str) {
        self.stop_spinner();
        let line = format!(
            "  {} {}",
            self.ui.brown("⊙"),
            self.ui.dim(&format!("compacted context — {detail}")),
        );
        self.write_region(&format!("{line}\n"));
        self.begin_thinking();
        self.render_composer();
    }

    fn on_compression(&mut self, mode: &str, saved_tokens: usize, total_saved_tokens: usize) {
        // Update the pinned line (above the context meter) in place — the
        // savings are chrome, not conversation, so they never scroll a line
        // into the transcript between tool output and the spinner.
        let verb = if mode == "audit" {
            "would save"
        } else {
            "saved"
        };
        self.compression_line = Some(format!(
            "  {} {} {} {}",
            self.ui.brown("⊙"),
            self.ui.dim("compression:"),
            self.ui.accent(mode),
            self.ui.dim(&format!(
                "· {verb} ~{saved_tokens} tokens this call ({total_saved_tokens} total) · /compression to switch"
            )),
        ));
        self.render();
    }

    // --- spinner -----------------------------------------------------------

    pub(super) fn tick_spinner(&mut self) {
        if self.suspended {
            return;
        }
        if let Some(sp) = self.spinner.as_mut() {
            sp.tick();
        }
        self.draw_spinner();
    }

    pub(super) fn draw_spinner(&mut self) {
        if self.suspended {
            return;
        }
        if let Some(sp) = self.spinner.as_ref() {
            let line = sp.line();
            let _ = write!(self.out, "\r{line}\x1b[K");
            let _ = self.out.flush();
            self.render_composer();
        }
    }

    // --- writing into the scroll region ------------------------------------

    /// Write text into the scroll region (newlines become `\r\n`), where the
    /// output cursor lives. Used for tool lines and the blank separator.
    fn write_region(&mut self, text: &str) {
        let mut w = CrlfWriter::new();
        let _ = w.write_all(text.as_bytes());
        let _ = w.flush();
    }

    /// Print a complete status line into the region, then redraw the composer.
    /// Used for the post-turn context-usage trailer and drain announcements.
    pub(super) fn print_line(&mut self, line: &str) {
        self.write_region(&format!("{line}\n"));
        self.render_composer();
    }
}
