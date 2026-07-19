//! The one place an [`AgentEvent`] becomes terminal presentation.
//!
//! Both CLI surfaces — the classic cooked-mode renderer
//! ([`crate::render::TurnRenderer`]) and the live composer
//! (`live::events`) — consume the [`Cue`] produced here, so per-event
//! formatting (tool lines, diffs, plan checklists, retry notices) exists
//! once and the two surfaces cannot drift apart. The writers keep only what
//! genuinely differs: spinner mechanics, the scroll region, pinned meters,
//! and the picker screen hand-off.
//!
//! A new [`AgentEvent`] variant is one arm in [`cue_for`] — the exhaustive
//! match makes the compiler point here, and both surfaces pick it up.

use harness_agent::AgentEvent;

use crate::render::truncate;
use crate::theme::Ui;

/// Which indicator should run after a cue's lines are printed.
pub(crate) enum NextSpinner {
    /// The between-steps "thinking" indicator.
    Thinking,
    /// A tool is running; show its verb (and target) with the timer.
    Working {
        tool: String,
        target: Option<String>,
    },
}

/// What one agent event asks the rendering surface to do.
pub(crate) enum Cue {
    /// Assistant text: append to the (surface-owned) markdown stream.
    Token(String),
    /// Styled lines for the transcript, then switch the indicator.
    Block {
        lines: Vec<String>,
        then: NextSpinner,
    },
    /// `spawn_agents` started: print the announcement; the fleet display owns
    /// activity from here (the cooked painter or the live pinned block).
    FleetStart { lines: Vec<String> },
    /// The ask-user picker takes the screen (suppress all tool chrome).
    AskUserStart,
    /// The picker returned the screen; resume thinking.
    AskUserEnd,
    /// A gated tool call awaits the user's approval decision.
    ApprovalPending,
    /// The decision landed; print it and resume thinking.
    ApprovalResolved { line: String },
    /// `web_search` ran without a key: flag the post-turn prompt, show a
    /// friendlier line than the raw error.
    BraveKeyMissing { line: String },
    /// Live pins the context meters from these figures; classic ignores it.
    Usage {
        context_tokens: usize,
        prompt_tokens_used: usize,
        completion_tokens_used: usize,
    },
    /// Compression savings: live updates `pinned_line` in place, classic
    /// scrolls `scroll_line` into the transcript.
    Compression {
        pinned_line: String,
        scroll_line: String,
    },
    /// Nothing to show on this surface.
    Ignore,
}

/// Map one event to its presentation. Pure — no terminal, fully testable.
pub(crate) fn cue_for(ui: &Ui, event: &AgentEvent) -> Cue {
    match event {
        AgentEvent::Token(t) => Cue::Token(t.clone()),
        // The model started writing a canvas; surface it while its content
        // streams in (the full preview prints on ToolStart).
        AgentEvent::ToolPending { name } if name == harness_tools::CANVAS_TOOL => Cue::Block {
            lines: vec![format!(
                "  {} {}",
                ui.green("📄"),
                ui.dim("writing canvas…")
            )],
            then: NextSpinner::Working {
                tool: name.clone(),
                target: None,
            },
        },
        AgentEvent::ToolPending { .. } => Cue::Ignore,
        AgentEvent::ToolStart { name, arguments } => tool_start_cue(ui, name, arguments),
        AgentEvent::ToolEnd { name, result } => tool_end_cue(ui, name, result),
        AgentEvent::ApprovalPending { .. } => Cue::ApprovalPending,
        AgentEvent::ApprovalResolved {
            command, decision, ..
        } => Cue::ApprovalResolved {
            line: format!(
                "  {} {}",
                ui.brown("🛡"),
                ui.dim(&format!("{decision} — {}", truncate(command, 100))),
            ),
        },
        AgentEvent::Usage {
            context_tokens,
            prompt_tokens_used,
            completion_tokens_used,
            ..
        } => Cue::Usage {
            context_tokens: *context_tokens,
            prompt_tokens_used: *prompt_tokens_used,
            completion_tokens_used: *completion_tokens_used,
        },
        // The context filled and was compacted to keep the session going;
        // surface a quiet notice so the trimming isn't invisible.
        AgentEvent::Compacted { detail } => Cue::Block {
            lines: vec![format!(
                "  {} {}",
                ui.brown("⊙"),
                ui.dim(&format!("compacted context — {detail}")),
            )],
            then: NextSpinner::Thinking,
        },
        AgentEvent::Compression {
            mode,
            saved_tokens,
            total_saved_tokens,
            ..
        } => {
            let verb = if mode == "audit" {
                "would save"
            } else {
                "saved"
            };
            Cue::Compression {
                pinned_line: format!(
                    "  {} {} {} {}",
                    ui.brown("⊙"),
                    ui.dim("compression:"),
                    ui.accent(mode),
                    ui.dim(&format!(
                        "· {verb} ~{saved_tokens} tokens this call ({total_saved_tokens} total) · /compression to switch"
                    )),
                ),
                scroll_line: format!(
                    "  {} {}",
                    ui.brown("⊙"),
                    ui.dim(&format!(
                        "compression {verb} ~{saved_tokens} tokens this call ({total_saved_tokens} total)"
                    )),
                ),
            }
        }
        // A transient provider/network failure being retried with backoff;
        // show it so the pause reads as a hiccup, not a hang.
        AgentEvent::Retrying {
            attempt,
            max_attempts,
            delay_ms,
            error,
        } => Cue::Block {
            lines: vec![format!(
                "  {} {}",
                ui.red("⚠"),
                ui.dim(&crate::turn::retry_notice(
                    *attempt,
                    *max_attempts,
                    *delay_ms,
                    error
                )),
            )],
            then: NextSpinner::Thinking,
        },
        // Streaming tool-argument fragments drive the desktop UI only.
        AgentEvent::ToolDelta { .. } => Cue::Ignore,
    }
}

fn tool_start_cue(ui: &Ui, name: &str, arguments: &str) -> Cue {
    // The picker draws its own UI and reads keys — no tool chrome at all.
    if name == harness_tools::ASK_USER_TOOL {
        return Cue::AskUserStart;
    }
    // The fleet paints its own multi-lane display while it runs.
    if name == harness_agent::FLEET_TOOL {
        return Cue::FleetStart {
            lines: vec![format!(
                "  {} {}",
                ui.green("🐂"),
                ui.dim("spawning agents…")
            )],
        };
    }
    let target = crate::live::tool_target(arguments);
    // A plan update prints the full checklist block (its result is just a
    // text echo, suppressed on ToolEnd).
    if name == harness_tools::PLAN_TOOL {
        return Cue::Block {
            lines: crate::plan::render_plan_block(ui, arguments).unwrap_or_default(),
            then: NextSpinner::Thinking,
        };
    }
    // A canvas previews the document inline; the result line then reports
    // the saved path / browser open.
    if name == harness_tools::CANVAS_TOOL {
        return Cue::Block {
            lines: crate::canvas::render_canvas_block(ui, arguments).unwrap_or_default(),
            then: NextSpinner::Working {
                tool: name.to_string(),
                target,
            },
        };
    }
    // File writes/edits show a colored diff instead of the generic one-line
    // tool preview.
    let lines = match crate::diff::render_file_change(ui, name, arguments) {
        Some(block) => block,
        None => {
            let verbs = ui.tool_verbs(name);
            let verb = verbs.first().map(String::as_str).unwrap_or("Working");
            vec![format!(
                "  {} {}  {}",
                ui.green("◆"),
                ui.accent(verb),
                ui.dim(&format!("{name}({})", truncate(arguments, 100))),
            )]
        }
    };
    Cue::Block {
        lines,
        then: NextSpinner::Working {
            tool: name.to_string(),
            target,
        },
    }
}

fn tool_end_cue(ui: &Ui, name: &str, result: &str) -> Cue {
    if name == harness_tools::ASK_USER_TOOL {
        return Cue::AskUserEnd;
    }
    // The plan block already printed on ToolStart; its result is an echo.
    if name == harness_tools::PLAN_TOOL {
        return Cue::Block {
            lines: Vec::new(),
            then: NextSpinner::Thinking,
        };
    }
    // Web search with no key: flag it for a prompt after the turn, and show
    // a friendlier line than the raw error.
    if name == harness_tools::WEB_SEARCH_TOOL
        && result.contains(harness_tools::web::WEB_SEARCH_NO_KEY)
    {
        return Cue::BraveKeyMissing {
            line: format!(
                "  {} {}",
                ui.brown("└─"),
                ui.dim("no Brave API key — you'll be prompted to add one below"),
            ),
        };
    }
    Cue::Block {
        lines: vec![format!(
            "  {} {}",
            ui.brown("└─"),
            ui.dim(&truncate(result, 140)),
        )],
        then: NextSpinner::Thinking,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> Ui {
        Ui::plain()
    }

    #[test]
    fn generic_tool_start_formats_verb_name_and_args() {
        let cue = cue_for(
            &ui(),
            &AgentEvent::ToolStart {
                name: "shell".into(),
                arguments: r#"{"command":"ls -la"}"#.into(),
            },
        );
        let Cue::Block { lines, then } = cue else {
            panic!("generic tool start should be a block");
        };
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("shell("), "line: {}", lines[0]);
        assert!(matches!(
            then,
            NextSpinner::Working { ref tool, ref target }
                if tool == "shell" && target.as_deref() == Some("ls -la")
        ));
    }

    #[test]
    fn plan_updates_render_the_checklist_on_both_surfaces() {
        let args = r#"{"plan":[{"content":"write code","status":"completed"},{"content":"test","status":"pending"}]}"#;
        let cue = cue_for(
            &ui(),
            &AgentEvent::ToolStart {
                name: harness_tools::PLAN_TOOL.into(),
                arguments: args.into(),
            },
        );
        let Cue::Block { lines, then } = cue else {
            panic!("plan start should be a block");
        };
        assert!(
            lines.iter().any(|l| l.contains("write code")),
            "plan checklist must render: {lines:?}"
        );
        assert!(matches!(then, NextSpinner::Thinking));
        // …and its ToolEnd echo is suppressed (an empty block).
        let cue = cue_for(
            &ui(),
            &AgentEvent::ToolEnd {
                name: harness_tools::PLAN_TOOL.into(),
                result: "plan updated".into(),
            },
        );
        let Cue::Block { lines, .. } = cue else {
            panic!("plan end should be a block");
        };
        assert!(lines.is_empty(), "plan result echo must be suppressed");
    }

    #[test]
    fn approval_events_map_to_the_hand_off_cues() {
        assert!(matches!(
            cue_for(
                &ui(),
                &AgentEvent::ApprovalPending {
                    name: "shell".into(),
                    command: "make deploy".into()
                }
            ),
            Cue::ApprovalPending
        ));
        let cue = cue_for(
            &ui(),
            &AgentEvent::ApprovalResolved {
                name: "shell".into(),
                command: "make deploy".into(),
                decision: "approved".into(),
            },
        );
        let Cue::ApprovalResolved { line } = cue else {
            panic!("resolved should carry its line");
        };
        assert!(line.contains("approved — make deploy"));
    }

    #[test]
    fn ask_user_suppresses_all_chrome() {
        assert!(matches!(
            cue_for(
                &ui(),
                &AgentEvent::ToolStart {
                    name: harness_tools::ASK_USER_TOOL.into(),
                    arguments: "{}".into()
                }
            ),
            Cue::AskUserStart
        ));
        assert!(matches!(
            cue_for(
                &ui(),
                &AgentEvent::ToolEnd {
                    name: harness_tools::ASK_USER_TOOL.into(),
                    result: "chose: yes".into()
                }
            ),
            Cue::AskUserEnd
        ));
    }

    #[test]
    fn compression_builds_both_the_pinned_and_scroll_variants() {
        let cue = cue_for(
            &ui(),
            &AgentEvent::Compression {
                mode: "audit".into(),
                saved_tokens: 120,
                total_saved_tokens: 450,
                results_compressed: 3,
            },
        );
        let Cue::Compression {
            pinned_line,
            scroll_line,
        } = cue
        else {
            panic!("compression should carry both lines");
        };
        assert!(pinned_line.contains("would save ~120"));
        assert!(pinned_line.contains("/compression to switch"));
        assert!(scroll_line.contains("would save ~120"));
        assert!(!scroll_line.contains("/compression to switch"));
    }
}
