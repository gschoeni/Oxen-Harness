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
use harness_permissions::ToolEffect;
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

/// What the model reads for a call whose tool panicked: the task vanished,
/// so this stands in for the result it never produced.
pub(super) const CRASHED_RESULT: &str = "tool error: the tool crashed before producing a result";

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
    /// result when the call was healed or the gate rewrote it (a
    /// move-to-trash, say).
    Run {
        tool: Arc<dyn Tool>,
        args: serde_json::Value,
        note: Option<String>,
    },
    /// Nothing runs: this is the call's whole result (a gate refusal, an
    /// unknown tool, or arguments that didn't parse).
    Resolved(String),
}

/// What executing a prepared call produced.
enum Executed {
    /// The call's result.
    Done(String),
    /// The tool rejected the arguments and a schema repair produced new
    /// ones. They have not been through the permission gate, and the gate
    /// (with its approval prompt) only runs on the turn's task with the
    /// event sink in hand — so the wave hands them back to be gated and
    /// run sequentially rather than running them here.
    Regate {
        tool: Arc<dyn Tool>,
        repaired: serde_json::Value,
        note: Option<String>,
    },
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

/// What a tool declares about its side effects, for the gate: its scheduling
/// class already says whether it only reads (shared) or mutates/runs a
/// process (exclusive).
fn effect_of(tool: &dyn Tool) -> ToolEffect {
    match tool.concurrency() {
        Concurrency::Shared => ToolEffect::ReadOnly,
        Concurrency::Exclusive => ToolEffect::Mutating,
    }
}

/// Prefix `note` (if any) to a result.
fn with_note(note: Option<String>, output: String) -> String {
    match note {
        Some(note) => format!("{note}\n{output}"),
        None => output,
    }
}

/// Two optional notes, joined.
fn join_notes(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(a), Some(b)) => Some(format!("{a}\n{b}")),
        (a, b) => a.or(b),
    }
}

/// A result the model can read: an empty one is spelled out.
fn non_empty(result: String) -> String {
    if result.trim().is_empty() {
        EMPTY_RESULT.to_string()
    } else {
        result
    }
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

    /// Parse a call's arguments and put the call through the permission
    /// gate. Nothing executes here, and no `ToolStart` fires yet — the
    /// approval prompt (if any) is rendered without a spinner line under it.
    ///
    /// The gate always sees the arguments that will actually run: arguments
    /// one bracket short of valid are healed *first*, then reviewed, so a
    /// healed `run_shell` is gated exactly like a well-formed one.
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
        let resolved = |message: String| PreparedCall {
            index,
            call_id: call_id.clone(),
            name: name.clone(),
            display_arguments: call.function.arguments.clone(),
            outcome: Prepared::Resolved(message),
        };

        // Arguments one bracket short of valid are healed rather than bounced
        // (a whole model round to re-emit the same call); the model is told,
        // so it can tighten up next time.
        let (args, mut note, healed) = match call.function.parsed_arguments() {
            Ok(args) => (args, None, false),
            Err(e) if reply_truncated => {
                return resolved(format!(
                    "tool error: the arguments were cut off — the reply hit its output-token \
                     limit before this call's JSON finished streaming ({e}). Don't retry the \
                     same call; produce less output per call, e.g. write the file in parts \
                     (a `write_file` with the first portion, then `edit_file` to extend it)."
                ));
            }
            Err(e) => match super::repair::heal_json(&call.function.arguments) {
                Some(args) => (args, Some(HEALED_NOTE.to_string()), true),
                None => return resolved(format!("tool error: invalid arguments JSON: {e}")),
            },
        };
        let Some(tool) = self.tools.get(&name).cloned() else {
            return resolved(format!(
                "tool error: {}",
                ToolError::UnknownTool(name.clone())
            ));
        };

        let mut rewritten = false;
        let args = match &self.config.permissions {
            None => args,
            Some(gate) => {
                match self
                    .consult_gate(gate, &name, &args, effect_of(tool.as_ref()), on_event)
                    .await
                {
                    GateVerdict::Proceed => args,
                    GateVerdict::ProceedRewritten {
                        args,
                        note: gate_note,
                    } => {
                        note = join_notes(note, Some(gate_note));
                        rewritten = true;
                        args
                    }
                    GateVerdict::Refused(message) => return resolved(message),
                }
            }
        };

        // Display the arguments that actually execute when they differ from
        // what the model sent (healed, or substituted by the gate).
        let display_arguments = if rewritten || healed {
            args.to_string()
        } else {
            call.function.arguments.clone()
        };
        PreparedCall {
            index,
            call_id,
            name,
            display_arguments,
            outcome: Prepared::Run { tool, args, note },
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
        // An attach-image marker is plumbing between the tool and the loop;
        // the UI sees the note the model will see in its place.
        let shown = harness_core::attach::extract_image_markers(result, "(image attached below)")
            .map(|(cleaned, _)| cleaned)
            .unwrap_or_else(|| result.to_string());
        on_event(&AgentEvent::ToolEnd {
            call_id: call_id.to_string(),
            name: name.to_string(),
            result: harness_core::text::truncate_with_marker(
                &shown,
                TOOL_RESULT_EVENT_CHARS,
                "\n… [full result retained in history]",
            ),
        });
    }

    /// Execute a prepared call. Failures come back as ordinary `tool error:
    /// …` results, so the model can read the error and self-correct in the
    /// turn. Owns nothing of the agent, so any number of these can run at
    /// once.
    async fn execute_prepared(outcome: Prepared) -> Executed {
        match outcome {
            Prepared::Resolved(message) => Executed::Done(non_empty(message)),
            Prepared::Run { tool, args, note } => {
                let output = match tool.invoke(args.clone()).await {
                    Ok(output) => output,
                    // Rejected arguments get one repair pass against the
                    // tool's own schema ("20" → 20, "yes" → true, a JSON
                    // array sent as a string) before the error goes back.
                    // The repaired call has not been gated, so it goes back
                    // to the wave for review rather than running here.
                    Err(ToolError::InvalidArguments(reason)) => {
                        match super::repair::coerce_to_schema(&tool.parameters_schema(), &args) {
                            Some(repaired) => {
                                return Executed::Regate {
                                    tool,
                                    repaired,
                                    note,
                                }
                            }
                            None => format!("tool error: {}", ToolError::InvalidArguments(reason)),
                        }
                    }
                    Err(e) => format!("tool error: {e}"),
                };
                Executed::Done(non_empty(with_note(note, output)))
            }
        }
    }

    /// Gate and run the schema-repaired arguments of a call the tool
    /// rejected — the same review a first-pass call gets, approval prompt
    /// included, so a payload the repair moved under `command` can't run
    /// unreviewed. A refusal is the call's result.
    async fn run_repaired<F>(
        &self,
        call_id: &str,
        name: &str,
        tool: Arc<dyn Tool>,
        repaired: serde_json::Value,
        note: Option<String>,
        on_event: &mut F,
    ) -> String
    where
        F: FnMut(&AgentEvent),
    {
        let (args, note) = match &self.config.permissions {
            None => (repaired, note),
            Some(gate) => {
                match self
                    .consult_gate(gate, name, &repaired, effect_of(tool.as_ref()), on_event)
                    .await
                {
                    GateVerdict::Proceed => (repaired, note),
                    GateVerdict::ProceedRewritten {
                        args,
                        note: gate_note,
                    } => (args, join_notes(note, Some(gate_note))),
                    GateVerdict::Refused(message) => return message,
                }
            }
        };
        let work = async move {
            match tool.invoke(args).await {
                Ok(output) => format!("{COERCED_NOTE}\n{output}"),
                Err(e) => format!("tool error: {e}"),
            }
        };
        match self.supervise(call_id, name, work, on_event).await {
            Some(output) => non_empty(with_note(note, output)),
            None => CRASHED_RESULT.to_string(),
        }
    }

    /// Run `work` on its own task, so a panicking tool becomes `None` (a
    /// result the caller can stand in for) instead of unwinding the turn,
    /// and forward the registry's live output chunks tagged with this
    /// call's tool name while it runs.
    async fn supervise<T, F>(
        &self,
        call_id: &str,
        name: &str,
        work: impl std::future::Future<Output = T> + Send + 'static,
        on_event: &mut F,
    ) -> Option<T>
    where
        T: Send + 'static,
        F: FnMut(&AgentEvent),
    {
        let mut progress = self.tools.progress();
        let mut handle = tokio::spawn(work);
        let joined = loop {
            match progress.as_mut() {
                Some(rx) => tokio::select! {
                    biased;
                    joined = &mut handle => break joined,
                    received = rx.recv() => match received {
                        Ok(p) if p.name == name => on_event(&AgentEvent::ToolProgress {
                            call_id: call_id.to_string(),
                            name: name.to_string(),
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
                None => break (&mut handle).await,
            }
        };
        match joined {
            Ok(value) => Some(value),
            // A panicking tool is a bug in the tool; the model still needs
            // a result for every call it made, so the caller reports one.
            Err(e) => {
                tracing::error!("tool task `{name}` failed: {e}");
                None
            }
        }
    }

    /// Run one wave of prepared calls, emitting `ToolStart` for each up
    /// front and `ToolEnd` as each finishes, and return `(index, result)`
    /// pairs in completion order. A wave of one runs on its own task with
    /// its live output forwarded; a larger wave runs its calls concurrently.
    /// Every call gets a `ToolEnd`, a crashed tool included.
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
            let (index, call_id, name) = (prepared.index, prepared.call_id, prepared.name);
            let executed = self
                .supervise(
                    &call_id,
                    &name,
                    Self::execute_prepared(prepared.outcome),
                    on_event,
                )
                .await;
            let result = self.settle(&call_id, &name, executed, on_event).await;
            Self::emit_tool_end(&call_id, &name, &result, on_event);
            results.push((index, result));
            return results;
        }
        let mut set = tokio::task::JoinSet::new();
        let mut calls = std::collections::HashMap::new();
        for prepared in wave {
            let (index, call_id, name) = (prepared.index, prepared.call_id, prepared.name);
            let handle = set.spawn(Self::execute_prepared(prepared.outcome));
            calls.insert(handle.id(), (index, call_id, name));
        }
        while let Some(joined) = set.join_next_with_id().await {
            let (id, executed) = match joined {
                Ok((id, executed)) => (id, Some(executed)),
                Err(e) => {
                    tracing::error!("tool task failed: {e}");
                    (e.id(), None)
                }
            };
            let Some((index, call_id, name)) = calls.remove(&id) else {
                continue;
            };
            let result = self.settle(&call_id, &name, executed, on_event).await;
            Self::emit_tool_end(&call_id, &name, &result, on_event);
            results.push((index, result));
        }
        results
    }

    /// Turn what a call's task produced into its result: a repaired call is
    /// gated and run (sequentially, with the event sink), and a vanished
    /// task is reported as a crash.
    async fn settle<F>(
        &self,
        call_id: &str,
        name: &str,
        executed: Option<Executed>,
        on_event: &mut F,
    ) -> String
    where
        F: FnMut(&AgentEvent),
    {
        match executed {
            Some(Executed::Done(result)) => result,
            Some(Executed::Regate {
                tool,
                repaired,
                note,
            }) => {
                self.run_repaired(call_id, name, tool, repaired, note, on_event)
                    .await
            }
            None => CRASHED_RESULT.to_string(),
        }
    }

    /// Put one tool call's arguments through the permission gate. Emits the
    /// approval events only for an interactive approver — a subagent's
    /// auto-deny must not flicker the host's screen hand-off.
    ///
    /// `args` must be the arguments that will run: the caller consults the
    /// gate again whenever it heals, repairs, or rewrites them afterwards.
    async fn consult_gate<F>(
        &self,
        gate: &std::sync::Arc<harness_permissions::PermissionGate>,
        name: &str,
        args: &serde_json::Value,
        effect: ToolEffect,
        on_event: &mut F,
    ) -> GateVerdict
    where
        F: FnMut(&AgentEvent),
    {
        match gate.review(name, args, effect) {
            harness_permissions::GateReview::Allow => GateVerdict::Proceed,
            harness_permissions::GateReview::Deny { message } => GateVerdict::Refused(message),
            harness_permissions::GateReview::Ask(request) => {
                let interactive = gate.is_interactive();
                let command = request.command.clone();
                if interactive {
                    on_event(&AgentEvent::ApprovalPending {
                        name: name.to_string(),
                        command: command.clone(),
                    });
                }
                let (outcome, decision) = gate.resolve(*request).await;
                if interactive {
                    on_event(&AgentEvent::ApprovalResolved {
                        name: name.to_string(),
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
// `env_guard()` serializes the tests that set `OXEN_HARNESS_DIR`, so holding
// it across a test's awaits is the point, not a hazard.
#[allow(clippy::await_holding_lock)]
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

    // ----- the call lifecycle through the real turn loop -----

    use std::sync::{Mutex, MutexGuard};

    use async_trait::async_trait;
    use harness_store::HistoryStore;
    use harness_tools::{ToolRegistry, TypedTool};

    use crate::test_support::{sse_prose, test_session};
    use crate::{AgentConfig, AgentEvent};

    /// Tests that build a permission gate set `OXEN_HARNESS_DIR`, so they
    /// must not interleave (and never unset it, so a neighbour's gate can't
    /// fall back to the real home).
    fn env_guard() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// SSE for a reply that makes the given tool calls (with raw argument
    /// strings, which need not be valid JSON).
    fn sse_calls(calls: &[(&str, &str, &str)]) -> String {
        let tool_calls: Vec<serde_json::Value> = calls
            .iter()
            .enumerate()
            .map(|(i, (id, name, args))| {
                serde_json::json!({
                    "index": i,
                    "id": id,
                    "function": { "name": name, "arguments": args }
                })
            })
            .collect();
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": { "content": "", "tool_calls": tool_calls },
                "finish_reason": "tool_calls"
            }]
        });
        format!("data: {chunk}\n\ndata: [DONE]\n\n")
    }

    /// One model round of tool calls, then a prose reply that ends the turn.
    async fn scripted_server(
        calls: &[(&str, &str, &str)],
    ) -> (mockito::ServerGuard, [mockito::Mock; 2]) {
        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_calls(calls))
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("done"))
            .expect(1)
            .create_async()
            .await;
        (server, [first, second])
    }

    fn agent_with(
        url: String,
        tools: ToolRegistry,
        permissions: Option<Arc<harness_permissions::PermissionGate>>,
    ) -> (Agent, Arc<HistoryStore>, String) {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = harness_llm::OxenClient::new(url, "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            permissions,
            ..AgentConfig::default()
        };
        let agent = Agent::new(client, tools, store.clone(), session.clone(), config).unwrap();
        (agent, store, session)
    }

    /// The `tool` message(s) the model read, in order.
    fn tool_results(store: &HistoryStore, session: &str) -> Vec<String> {
        store
            .messages(session)
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "tool")
            .filter_map(|m| m["content"].as_str().map(str::to_string))
            .collect()
    }

    /// A stand-in `run_shell`: same name and argument shape as the real one
    /// (so the gate's shell rules apply), but it only records what it was
    /// asked to run.
    struct RecordingShell(Arc<Mutex<Vec<String>>>);

    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct RecordingShellArgs {
        command: String,
    }

    #[async_trait]
    impl TypedTool for RecordingShell {
        const NAME: &'static str = harness_tools::RUN_SHELL_TOOL;
        type Args = RecordingShellArgs;
        fn description(&self) -> &str {
            "records"
        }
        fn concurrency(&self) -> Concurrency {
            Concurrency::Exclusive
        }
        async fn run(&self, args: RecordingShellArgs) -> Result<String, ToolError> {
            self.0.lock().unwrap().push(args.command.clone());
            Ok(format!("ran {}", args.command))
        }
    }

    struct NeverAsk;

    #[async_trait]
    impl harness_permissions::CommandApprover for NeverAsk {
        async fn approve(
            &self,
            _request: &harness_permissions::ApprovalRequest,
        ) -> Result<Option<harness_permissions::ApprovalDecision>, String> {
            Ok(Some(harness_permissions::ApprovalDecision::Deny))
        }
    }

    /// A gate in plan mode: the tree is read-only, so a `run_shell` the
    /// classifier can't prove harmless is refused with no prompt — a
    /// deterministic way to observe whether the gate saw a command.
    fn plan_mode_gate(home: &std::path::Path) -> Arc<harness_permissions::PermissionGate> {
        std::env::set_var("OXEN_HARNESS_DIR", home);
        let gate = harness_permissions::PermissionGate::new("/tmp/proj", Arc::new(NeverAsk));
        gate.set_plan_mode(true);
        Arc::new(gate)
    }

    /// Arguments one brace short of valid are healed *before* the gate
    /// reviews them: the healed `rm -rf build` is refused, and only the
    /// healed `ls` runs — carrying the heal note.
    #[tokio::test]
    async fn healed_arguments_are_gated_before_they_run() {
        let _env = env_guard();
        let home = tempfile::tempdir().unwrap();
        for (raw, expect_run) in [
            (r#"{"command":"rm -rf build""#, false),
            (r#"{"command":"ls""#, true),
        ] {
            let (server, mocks) = scripted_server(&[("c1", "run_shell", raw)]).await;
            let ran = Arc::new(Mutex::new(Vec::new()));
            let tools = ToolRegistry::new().with_typed(RecordingShell(ran.clone()));
            let (mut agent, store, session) =
                agent_with(server.url(), tools, Some(plan_mode_gate(home.path())));
            let out = agent.run_turn("go", |_| {}).await.unwrap();
            assert_eq!(out, "done");
            for m in mocks {
                m.assert_async().await;
            }
            let results = tool_results(&store, &session);
            assert_eq!(results.len(), 1, "{results:?}");
            if expect_run {
                assert_eq!(*ran.lock().unwrap(), vec!["ls".to_string()]);
                assert!(results[0].contains(HEALED_NOTE), "{}", results[0]);
                assert!(results[0].contains("ran ls"), "{}", results[0]);
            } else {
                assert!(
                    ran.lock().unwrap().is_empty(),
                    "the gate was bypassed: {ran:?}"
                );
                assert!(results[0].contains("plan mode is on"), "{}", results[0]);
            }
        }
    }

    /// A payload under the wrong key (`input` for `command`) is adopted by
    /// the schema repair — and the repaired call is gated like any other,
    /// so `git push --force` is refused while `ls` runs with the coercion
    /// note.
    #[tokio::test]
    async fn schema_repaired_arguments_are_gated_before_they_run() {
        let _env = env_guard();
        let home = tempfile::tempdir().unwrap();
        for (raw, expect_run) in [
            (r#"{"input":"git push --force"}"#, false),
            (r#"{"input":"ls"}"#, true),
        ] {
            let (server, mocks) = scripted_server(&[("c1", "run_shell", raw)]).await;
            let ran = Arc::new(Mutex::new(Vec::new()));
            let tools = ToolRegistry::new().with_typed(RecordingShell(ran.clone()));
            let (mut agent, store, session) =
                agent_with(server.url(), tools, Some(plan_mode_gate(home.path())));
            let out = agent.run_turn("go", |_| {}).await.unwrap();
            assert_eq!(out, "done");
            for m in mocks {
                m.assert_async().await;
            }
            let results = tool_results(&store, &session);
            assert_eq!(results.len(), 1, "{results:?}");
            if expect_run {
                assert_eq!(*ran.lock().unwrap(), vec!["ls".to_string()]);
                assert!(results[0].contains(COERCED_NOTE), "{}", results[0]);
                assert!(results[0].contains("ran ls"), "{}", results[0]);
            } else {
                assert!(
                    ran.lock().unwrap().is_empty(),
                    "the gate was bypassed: {ran:?}"
                );
                assert!(results[0].contains("plan mode is on"), "{}", results[0]);
            }
        }
    }

    /// A tool that panics instead of answering.
    struct Boom;

    #[async_trait]
    impl Tool for Boom {
        fn name(&self) -> &str {
            "boom"
        }
        fn description(&self) -> &str {
            "panics"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        async fn invoke(&self, _args: serde_json::Value) -> Result<String, ToolError> {
            panic!("boom");
        }
    }

    /// A tool that answers "pong".
    struct Ping;

    #[async_trait]
    impl Tool for Ping {
        fn name(&self) -> &str {
            "ping"
        }
        fn description(&self) -> &str {
            "answers"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        async fn invoke(&self, _args: serde_json::Value) -> Result<String, ToolError> {
            Ok("pong".into())
        }
    }

    /// A panicking tool — alone in its wave, or beside a healthy call — is
    /// a crash *result*: the turn survives, the model reads a result for
    /// every call, and every call gets its `ToolEnd` so no UI card spins
    /// forever.
    #[tokio::test]
    async fn a_panicking_tool_becomes_a_result_and_a_tool_end() {
        for calls in [
            vec![("c1", "boom", "{}")],
            vec![("c1", "boom", "{}"), ("c2", "ping", "{}")],
        ] {
            let (server, mocks) = scripted_server(&calls).await;
            let tools = ToolRegistry::new()
                .with(Arc::new(Boom))
                .with(Arc::new(Ping));
            let (mut agent, store, session) = agent_with(server.url(), tools, None);
            let mut ends = Vec::new();
            let out = agent
                .run_turn("go", |e| {
                    if let AgentEvent::ToolEnd {
                        call_id, result, ..
                    } = e
                    {
                        ends.push((call_id.clone(), result.clone()));
                    }
                })
                .await
                .expect("a tool panic must not fail the turn");
            assert_eq!(out, "done");
            for m in mocks {
                m.assert_async().await;
            }
            ends.sort();
            let mut expected = vec![("c1".to_string(), CRASHED_RESULT.to_string())];
            if calls.len() == 2 {
                expected.push(("c2".to_string(), "pong".to_string()));
            }
            assert_eq!(ends, expected, "{} call(s)", calls.len());
            let results = tool_results(&store, &session);
            assert_eq!(results.len(), calls.len());
            assert_eq!(results[0], CRASHED_RESULT);
        }
    }
}
