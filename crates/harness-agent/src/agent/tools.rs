//! Running one tool call, and asking the permission gate first.
//!
//! Split from the turn loop because the interesting logic here is about a
//! single call's lifecycle — approval, invocation, event bracketing, and
//! turning failures into results the model can read and correct — none of
//! which the loop needs to see.

use harness_llm::ToolCall;

use crate::event::AgentEvent;

use super::Agent;

/// The most of a tool call's arguments carried on an event (the UI renders a
/// preview, not the payload).
const TOOL_ARGUMENT_EVENT_CHARS: usize = 16_000;
/// The most of a tool result carried on an event.
const TOOL_RESULT_EVENT_CHARS: usize = 4_000;

impl Agent {
    /// Run one tool call, bracketing it with [`AgentEvent::ToolStart`] and
    /// [`AgentEvent::ToolEnd`]. Failures come back as ordinary `tool error: …`
    /// results, so the model can read the error and self-correct in the turn.
    ///
    /// `reply_truncated` is whether the reply carrying this call was cut off at
    /// the response token limit — unparseable arguments then get a targeted
    /// error (split the content up) instead of a bare JSON parse failure the
    /// model tends to misdiagnose.
    ///
    /// When a permission gate is configured, it is consulted *before* the tool
    /// executes (and before `ToolStart`, so an approval prompt isn't rendered
    /// under a spinner line): a refusal becomes the tool result, and an
    /// approved move-to-trash rewrites the call's arguments.
    pub(super) async fn run_tool<F>(
        &self,
        call: &ToolCall,
        reply_truncated: bool,
        on_event: &mut F,
    ) -> String
    where
        F: FnMut(&AgentEvent),
    {
        let mut rewritten: Option<(serde_json::Value, String)> = None;
        if let Some(gate) = &self.config.permissions {
            match self.consult_gate(gate, call, on_event).await {
                GateVerdict::Proceed => {}
                GateVerdict::ProceedRewritten { args, note } => rewritten = Some((args, note)),
                GateVerdict::Refused(message) => {
                    // The refusal is the call's result: bracket it with the
                    // usual events so hosts render it like any tool outcome.
                    on_event(&AgentEvent::ToolStart {
                        name: call.function.name.clone(),
                        arguments: harness_core::text::truncate_with_marker(
                            &call.function.arguments,
                            TOOL_ARGUMENT_EVENT_CHARS,
                            "\n… [arguments omitted from display]",
                        ),
                    });
                    on_event(&AgentEvent::ToolEnd {
                        name: call.function.name.clone(),
                        result: message.clone(),
                    });
                    return message;
                }
            }
        }

        // Display (and run) the rewritten arguments when the gate substituted
        // them, so what the user sees is what actually executes.
        let display_arguments = rewritten
            .as_ref()
            .map(|(args, _)| args.to_string())
            .unwrap_or_else(|| call.function.arguments.clone());
        on_event(&AgentEvent::ToolStart {
            name: call.function.name.clone(),
            arguments: harness_core::text::truncate_with_marker(
                &display_arguments,
                TOOL_ARGUMENT_EVENT_CHARS,
                "\n… [arguments omitted from display]",
            ),
        });

        let parsed = match rewritten {
            Some((args, note)) => Ok((args, Some(note))),
            None => call.function.parsed_arguments().map(|args| (args, None)),
        };
        let result = match parsed {
            Ok((args, note)) => {
                let output = match self.tools.invoke(&call.function.name, args).await {
                    Ok(output) => output,
                    Err(e) => format!("tool error: {e}"),
                };
                match note {
                    Some(note) => format!("{note}\n{output}"),
                    None => output,
                }
            }
            Err(e) if reply_truncated => format!(
                "tool error: the arguments were cut off — the reply hit its output-token \
                 limit before this call's JSON finished streaming ({e}). Don't retry the \
                 same call; produce less output per call, e.g. write the file in parts \
                 (a `write_file` with the first portion, then `edit_file` to extend it)."
            ),
            Err(e) => format!("tool error: invalid arguments JSON: {e}"),
        };

        on_event(&AgentEvent::ToolEnd {
            name: call.function.name.clone(),
            result: harness_core::text::truncate_with_marker(
                &result,
                TOOL_RESULT_EVENT_CHARS,
                "\n… [full result retained in history]",
            ),
        });
        result
    }

    /// Put one tool call through the permission gate. Emits the approval
    /// events only for an interactive approver — a subagent's auto-deny must
    /// not flicker the host's screen hand-off.
    async fn consult_gate<F>(
        &self,
        gate: &std::sync::Arc<harness_permissions::PermissionGate>,
        call: &ToolCall,
        on_event: &mut F,
    ) -> GateVerdict
    where
        F: FnMut(&AgentEvent),
    {
        // Unparseable arguments take the normal invoke path so the model gets
        // the standard "invalid arguments JSON" error.
        let Ok(args) = call.function.parsed_arguments() else {
            return GateVerdict::Proceed;
        };
        match gate.review(&call.function.name, &args) {
            harness_permissions::GateReview::Allow => GateVerdict::Proceed,
            harness_permissions::GateReview::Deny { message } => GateVerdict::Refused(message),
            harness_permissions::GateReview::Ask(request) => {
                let interactive = gate.is_interactive();
                let command = request.command.clone();
                if interactive {
                    on_event(&AgentEvent::ApprovalPending {
                        name: call.function.name.clone(),
                        command: command.clone(),
                    });
                }
                let (outcome, decision) = gate.resolve(*request).await;
                if interactive {
                    on_event(&AgentEvent::ApprovalResolved {
                        name: call.function.name.clone(),
                        command,
                        decision: decision.label().to_string(),
                    });
                }
                match outcome {
                    harness_permissions::GateOutcome::Allow => GateVerdict::Proceed,
                    harness_permissions::GateOutcome::AllowRewritten { args, note } => {
                        GateVerdict::ProceedRewritten { args, note }
                    }
                    harness_permissions::GateOutcome::Deny { message } => {
                        GateVerdict::Refused(message)
                    }
                }
            }
        }
    }
}

/// What the permission gate decided about one tool call.
enum GateVerdict {
    Proceed,
    ProceedRewritten {
        args: serde_json::Value,
        note: String,
    },
    Refused(String),
}
