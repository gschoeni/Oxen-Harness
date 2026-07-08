//! Live rendering of an agent turn: a trail-status spinner while the model
//! thinks or a tool runs, assistant text streamed through the Markdown
//! renderer, and themed tool lines. Shared by the REPL prompt flow and the
//! loop runner so both surfaces feel identical.

use harness_agent::AgentEvent;

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
        match event {
            AgentEvent::Token(t) => {
                if self.md.is_none() {
                    self.stop_spinner();
                    println!();
                    self.md = Some(markdown::MarkdownStream::new(
                        self.ui.clone(),
                        std::io::stdout(),
                    ));
                }
                if let Some(md) = self.md.as_mut() {
                    md.push(t);
                }
            }
            // The model started writing a canvas; show progress while its content
            // streams in (the full preview prints on ToolStart).
            AgentEvent::ToolPending { name } if name == harness_tools::CANVAS_TOOL => {
                self.stop_spinner();
                self.end_markdown();
                println!(
                    "  {} {}",
                    self.ui.green("📄"),
                    self.ui.dim("writing canvas…")
                );
                self.begin_working(name, None);
            }
            AgentEvent::ToolPending { .. } => {}
            AgentEvent::ToolStart { name, arguments } => {
                self.stop_spinner();
                self.end_markdown();
                // The interactive picker draws its own UI and reads keys, so we
                // suppress the tool line + spinner while it owns the screen.
                if name == harness_tools::ASK_USER_TOOL {
                    return;
                }
                // The fleet paints its own multi-lane block (and owns the keys)
                // while it runs; keep the spinner out of its way.
                if name == harness_agent::FLEET_TOOL {
                    println!(
                        "  {} {}",
                        self.ui.green("🐂"),
                        self.ui.dim("spawning agents…")
                    );
                    return;
                }
                let target = crate::live::tool_target(arguments);
                // For a plan update, print the full checklist block (the call's
                // result is just a text echo, so suppress it on ToolEnd below).
                if name == harness_tools::PLAN_TOOL {
                    if let Some(block) = crate::plan::render_plan_block(&self.ui, arguments) {
                        for line in block {
                            println!("{line}");
                        }
                    }
                    self.begin_thinking();
                    return;
                }
                // For a canvas, preview the document inline (the result line then
                // reports where it was saved / that it opened in the browser).
                if name == harness_tools::CANVAS_TOOL {
                    if let Some(block) = crate::canvas::render_canvas_block(&self.ui, arguments) {
                        for line in block {
                            println!("{line}");
                        }
                    }
                    self.begin_working(name, target);
                    return;
                }
                // For file writes/edits, show a colored diff instead of the
                // generic one-line tool preview.
                if let Some(block) = crate::diff::render_file_change(&self.ui, name, arguments) {
                    for line in block {
                        println!("{line}");
                    }
                } else {
                    let verbs = self.ui.tool_verbs(name);
                    let verb = verbs.first().map(String::as_str).unwrap_or("Working");
                    println!(
                        "  {} {}  {}",
                        self.ui.green("◆"),
                        self.ui.accent(verb),
                        self.ui
                            .dim(&format!("{name}({})", truncate(arguments, 100))),
                    );
                }
                self.begin_working(name, target);
            }
            AgentEvent::ToolEnd { name, result } => {
                self.stop_spinner();
                // The picker already showed the chosen answer in place.
                if name == harness_tools::ASK_USER_TOOL {
                    self.begin_thinking();
                    return;
                }
                // The plan block was already printed on ToolStart; its result is
                // just a text echo, so don't print a redundant result line.
                if name == harness_tools::PLAN_TOOL {
                    self.begin_thinking();
                    return;
                }
                // A web search with no key configured: flag it (so the caller can
                // prompt for one) and show a friendlier line than the raw error.
                if name == harness_tools::WEB_SEARCH_TOOL
                    && result.contains(harness_tools::web::WEB_SEARCH_NO_KEY)
                {
                    self.needs_brave_key = true;
                    println!(
                        "  {} {}",
                        self.ui.brown("└─"),
                        self.ui
                            .dim("no Brave API key — set one below to enable web search"),
                    );
                    self.begin_thinking();
                    return;
                }
                println!(
                    "  {} {}",
                    self.ui.brown("└─"),
                    self.ui.dim(&truncate(result, 140)),
                );
                self.begin_thinking();
            }
            // Usage is surfaced in the banner/status, not inline during a turn.
            AgentEvent::Usage { .. } => {}
            // The context filled and was compacted to keep the session going;
            // surface a quiet notice so the trimming isn't invisible.
            AgentEvent::Compacted { detail } => {
                self.stop_spinner();
                println!(
                    "  {} {}",
                    self.ui.brown("⊙"),
                    self.ui.dim(&format!("compacted context — {detail}")),
                );
                self.begin_thinking();
            }
            // Stale tool output was compressed (or measured, in audit mode)
            // before the model call; a quiet note keeps the savings visible.
            AgentEvent::Compression {
                mode,
                saved_tokens,
                total_saved_tokens,
                ..
            } => {
                self.stop_spinner();
                let verb = if mode == "audit" {
                    "would save"
                } else {
                    "saved"
                };
                println!(
                    "  {} {}",
                    self.ui.brown("⊙"),
                    self.ui.dim(&format!(
                        "compression {verb} ~{saved_tokens} tokens this call ({total_saved_tokens} total)"
                    )),
                );
                self.begin_thinking();
            }
            // A transient provider/network failure being retried with backoff;
            // show it so the pause reads as a hiccup, not a hang.
            AgentEvent::Retrying {
                attempt,
                max_attempts,
                delay_ms,
                error,
            } => {
                self.stop_spinner();
                println!(
                    "  {} {}",
                    self.ui.red("⚠"),
                    self.ui.dim(&crate::turn::retry_notice(
                        *attempt,
                        *max_attempts,
                        *delay_ms,
                        error
                    )),
                );
                self.begin_thinking();
            }
            // Streaming tool-argument fragments drive the desktop UI; the CLI
            // shows the assembled call when it starts, so ignore the deltas.
            AgentEvent::ToolDelta { .. } => {}
        }
    }

    pub(crate) fn finish(&mut self) {
        self.stop_spinner();
        self.end_markdown();
    }
}

/// Collapse newlines and cap a string to `max` characters, adding an ellipsis.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    harness_core::text::ellipsize(&s.replace('\n', " "), max)
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
