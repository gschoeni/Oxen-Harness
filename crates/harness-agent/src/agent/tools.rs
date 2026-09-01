//! Running the tool calls of one reply, and asking the permission gate first.
//!
//! Split from the turn loop because the interesting logic here is about a
//! call's lifecycle — approval, invocation, event bracketing, and turning
//! failures into results the model can read and correct — none of which the
//! loop needs to see.
//!
//! Calls run in *waves* (see [`plan_waves`]): consecutive calls whose tools
//! are [`Concurrency::Shared`] run at the same time, while an
//! [`Concurrency::Exclusive`] call waits for everything before it and runs
//! alone. Approval prompts are always sequential and always precede the
//! first execution, so the user never sees two pickers at once and a refused
//! call never runs.
//!
//! [`Concurrency::Shared`]: harness_tools::Concurrency::Shared
//! [`Concurrency::Exclusive`]: harness_tools::Concurrency::Exclusive

use std::sync::Arc;

use harness_llm::ToolCall;
use harness_tools::{Concurrency, Tool, ToolError};

use crate::event::AgentEvent;

use super::Agent;

/// The most of a tool call's arguments carried on an event (the UI renders a
/// preview, not the payload).
const TOOL_ARGUMENT_EVENT_CHARS: usize = 16_000;
/// The most of a tool result carried on an event.
const TOOL_RESULT_EVENT_CHARS: usize = 4_000;

/// What a model sees when a tool returns nothing at all. Some providers
/// reject an empty tool result outright, and a blank one reads as "the
/// tool is broken" to the model, so the absence is spelled out.
const EMPTY_RESULT: &str = "(the tool returned no output)";

/// Prefixed to a result whose arguments arrived unparseable and were healed.
const HEALED_NOTE: &str = "[note: the call's JSON arguments were cut short and auto-healed; \
emit complete JSON next time]";
/// Prefixed to a result whose arguments the tool rejected until they were
/// coerced to its schema.
const COERCED_NOTE: &str = "[note: argument types were auto-corrected to match the tool's \
schema (e.g. \"20\" → 20); send the declared types next time]";

/// A tool call after the permission gate and argument parsing, ready to run.
pub(super) struct PreparedCall {
    /// Index of the call in the reply, so results are folded back in order.
    pub index: usize,
    pub call_id: String,
    pub name: String,
    /// What the UI shows as the arguments (the gate may have rewritten them).
    display_arguments: String,
    outcome: Prepared,
}

enum Prepared {
    /// Run the tool with these parsed arguments; `note` is prefixed to the
    /// result when the gate rewrote the call (a move-to-trash, say).
    Run {
        tool: Arc<dyn Tool>,
        args: serde_json::Value,
        note: Option<String>,
    },
    /// Nothing runs: this is the call's whole result (a gate refusal, an
    /// unknown tool, or arguments that didn't parse).
    Resolved(String),
}

/// Group a reply's calls into waves by their tools' concurrency: a run of
/// shared calls is one wave (they run together), and every exclusive call —
/// or a call to a tool the registry doesn't know — is a wave of its own.
pub(super) fn plan_waves(
    calls: &[ToolCall],
    concurrency_of: impl Fn(&str) -> Concurrency,
) -> Vec<Vec<usize>> {
    let mut waves: Vec<Vec<usize>> = Vec::new();
    let mut shared: Vec<usize> = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        match concurrency_of(&call.function.name) {
            Concurrency::Shared => shared.push(index),
            Concurrency::Exclusive => {
                if !shared.is_empty() {
                    waves.push(std::mem::take(&mut shared));
                }
                waves.push(vec![index]);
            }
        }
    }
    if !shared.is_empty() {
        waves.push(shared);
    }
    waves
}

impl Agent {
    /// The scheduling class of a named tool; unknown tools are treated as
    /// exclusive so they get their own (error-producing) wave.
    pub(super) fn concurrency_of(&self, name: &str) -> Concurrency {
        self.tools
            .get(name)
            .map(|t| t.concurrency())
            .unwrap_or(Concurrency::Exclusive)
    }

    /// Put one call through the permission gate and parse its arguments.
    /// Nothing executes here, and no `ToolStart` fires yet — the approval
    /// prompt (if any) is rendered without a spinner line under it.
    ///
    /// `reply_truncated` is whether the reply carrying this call was cut off at
    /// the response token limit — unparseable arguments then get a targeted
    /// error (split the content up) instead of a bare JSON parse failure the
    /// model tends to misdiagnose.
    pub(super) async fn prepare_tool<F>(
        &self,
        index: usize,
        call: &ToolCall,
        reply_truncated: bool,
        on_event: &mut F,
    ) -> PreparedCall
    where
        F: FnMut(&AgentEvent),
    {
        let name = call.function.name.clone();
        let call_id = call.id.clone();
        let mut rewritten: Option<(serde_json::Value, String)> = None;
        if let Some(gate) = &self.config.permissions {
            match self.consult_gate(gate, call, on_event).await {
                GateVerdict::Proceed => {}
                GateVerdict::ProceedRewritten { args, note } => rewritten = Some((args, note)),
                GateVerdict::Refused(message) => {
                    return PreparedCall {
                        index,
                        call_id,
                        name,
                        display_arguments: call.function.arguments.clone(),
                        outcome: Prepared::Resolved(message),
                    };
                }
            }
        }

        // Display (and run) the rewritten arguments when the gate substituted
        // them, so what the user sees is what actually executes.
        let display_arguments = rewritten
            .as_ref()
            .map(|(args, _)| args.to_string())
            .unwrap_or_else(|| call.function.arguments.clone());

        let parsed = match rewritten {
            Some((args, note)) => Ok((args, Some(note))),
            None => call.function.parsed_arguments().map(|args| (args, None)),
        };
        // Arguments one bracket short of valid are healed rather than bounced
        // (a whole model round to re-emit the same call); the model is told,
        // so it can tighten up next time.
        let parsed = match parsed {
            Err(e) if !reply_truncated => {
                match super::repair::heal_json(&call.function.arguments) {
                    Some(args) => Ok((args, Some(HEALED_NOTE.to_string()))),
                    None => Err(e),
                }
            }
            other => other,
        };
        let outcome = match parsed {
            Ok((args, note)) => match self.tools.get(&name) {
                Some(tool) => Prepared::Run {
                    tool: tool.clone(),
                    args,
                    note,
                },
                None => Prepared::Resolved(format!(
                    "tool error: {}",
                    ToolError::UnknownTool(name.clone())
                )),
            },
            Err(e) if reply_truncated => Prepared::Resolved(format!(
                "tool error: the arguments were cut off — the reply hit its output-token \
                 limit before this call's JSON finished streaming ({e}). Don't retry the \
                 same call; produce less output per call, e.g. write the file in parts \
                 (a `write_file` with the first portion, then `edit_file` to extend it)."
            )),
            Err(e) => Prepared::Resolved(format!("tool error: invalid arguments JSON: {e}")),
        };
        PreparedCall {
            index,
            call_id,
            name,
            display_arguments,
            outcome,
        }
    }

    /// Announce that a prepared call is running.
    pub(super) fn emit_tool_start<F>(prepared: &PreparedCall, on_event: &mut F)
    where
        F: FnMut(&AgentEvent),
    {
        on_event(&AgentEvent::ToolStart {
            call_id: prepared.call_id.clone(),
            name: prepared.name.clone(),
            arguments: harness_core::text::truncate_with_marker(
                &prepared.display_arguments,
                TOOL_ARGUMENT_EVENT_CHARS,
                "\n… [arguments omitted from display]",
            ),
        });
    }

    /// Announce a call's result.
    pub(super) fn emit_tool_end<F>(call_id: &str, name: &str, result: &str, on_event: &mut F)
    where
        F: FnMut(&AgentEvent),
    {
        on_event(&AgentEvent::ToolEnd {
            call_id: call_id.to_string(),
            name: name.to_string(),
            result: harness_core::text::truncate_with_marker(
                result,
                TOOL_RESULT_EVENT_CHARS,
                "\n… [full result retained in history]",
            ),
        });
    }

    /// Execute a prepared call to its result string. Failures come back as
    /// ordinary `tool error: …` results, so the model can read the error and
    /// self-correct in the turn. Owns nothing of the agent, so any number of
    /// these can run at once.
    async fn execute_prepared(outcome: Prepared) -> String {
        let result = match outcome {
            Prepared::Resolved(message) => message,
            Prepared::Run { tool, args, note } => {
                let output = match tool.invoke(args.clone()).await {
                    Ok(output) => output,
                    // Rejected arguments get one repair pass against the
                    // tool's own schema ("20" → 20, "yes" → true, a JSON
                    // array sent as a string) before the error goes back.
                    Err(ToolError::InvalidArguments(reason)) => {
                        match super::repair::coerce_to_schema(&tool.parameters_schema(), &args) {
                            Some(repaired) => match tool.invoke(repaired).await {
                                Ok(output) => format!("{COERCED_NOTE}\n{output}"),
                                Err(e) => format!("tool error: {e}"),
                            },
                            None => format!("tool error: {}", ToolError::InvalidArguments(reason)),
                        }
                    }
                    Err(e) => format!("tool error: {e}"),
                };
                match note {
                    Some(note) => format!("{note}\n{output}"),
                    None => output,
                }
            }
        };
        if result.trim().is_empty() {
            EMPTY_RESULT.to_string()
        } else {
            result
        }
    }

    /// Run one wave of prepared calls, emitting `ToolStart` for each up
    /// front and `ToolEnd` as each finishes, and return `(index, result)`
    /// pairs in completion order. A wave of one runs inline; a larger wave
    /// runs its calls concurrently.
    pub(super) async fn run_wave<F>(
        &self,
        wave: Vec<PreparedCall>,
        on_event: &mut F,
    ) -> Vec<(usize, String)>
    where
        F: FnMut(&AgentEvent),
    {
        for prepared in &wave {
            Self::emit_tool_start(prepared, on_event);
        }
        let mut results = Vec::with_capacity(wave.len());
        if wave.len() == 1 {
            // A lone call (every exclusive tool, notably the shell) is the one
            // whose progress can be attributed: forward the registry's live
            // output chunks tagged with its name while it runs.
            let prepared = wave.into_iter().next().expect("one call");
            let mut progress = self.tools.progress();
            let run = Self::execute_prepared(prepared.outcome);
            tokio::pin!(run);
            let result = loop {
                match progress.as_mut() {
                    Some(rx) => tokio::select! {
                        biased;
                        result = &mut run => break result,
                        received = rx.recv() => match received {
                            Ok(p) if p.name == prepared.name => on_event(&AgentEvent::ToolProgress {
                                call_id: prepared.call_id.clone(),
                                name: prepared.name.clone(),
                                chunk: p.chunk,
                            }),
                            Ok(_) => {}
                            // Lagged: chunks were missed, the result has them all.
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                progress = None;
                            }
                        },
                    },
                    None => break run.await,
                }
            };
            Self::emit_tool_end(&prepared.call_id, &prepared.name, &result, on_event);
            results.push((prepared.index, result));
            return results;
        }
        let mut set = tokio::task::JoinSet::new();
        for prepared in wave {
            let (index, call_id, name) = (prepared.index, prepared.call_id, prepared.name);
            set.spawn(async move {
                let result = Self::execute_prepared(prepared.outcome).await;
                (index, call_id, name, result)
            });
        }
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((index, call_id, name, result)) => {
                    Self::emit_tool_end(&call_id, &name, &result, on_event);
                    results.push((index, result));
                }
                // A panicking tool is a bug in the tool; the model still needs
                // a result for every call it made, so report it as one.
                Err(e) => tracing::error!("tool task failed: {e}"),
            }
        }
        results
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

#[cfg(test)]
mod tests {
    use super::*;
    use harness_llm::types::FunctionCall;

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{name}"),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        }
    }

    fn concurrency(name: &str) -> Concurrency {
        match name {
            "edit" | "shell" => Concurrency::Exclusive,
            _ => Concurrency::Shared,
        }
    }

    #[test]
    fn shared_runs_group_and_exclusive_calls_stand_alone() {
        let calls = [
            call("read"),
            call("grep"),
            call("edit"),
            call("read"),
            call("shell"),
            call("read"),
            call("read"),
        ];
        assert_eq!(
            plan_waves(&calls, concurrency),
            vec![vec![0, 1], vec![2], vec![3], vec![4], vec![5, 6]]
        );
    }

    #[test]
    fn a_reply_of_only_shared_calls_is_one_wave() {
        let calls = [call("read"), call("read"), call("grep")];
        assert_eq!(plan_waves(&calls, concurrency), vec![vec![0, 1, 2]]);
    }

    #[test]
    fn no_calls_no_waves() {
        assert!(plan_waves(&[], concurrency).is_empty());
    }
}
