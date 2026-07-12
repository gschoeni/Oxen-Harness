//! The turn loop: drive the model/tool cycle from a user message to a final
//! reply.
//!
//! One turn is a sequence of model calls. Each call may request tool calls;
//! the loop runs them, appends the results, and calls the model again until it
//! answers in prose. Along the way the loop budgets the context window
//! (compacting when it would overflow), retries transient model failures with
//! backoff, and emits [`AgentEvent`]s so a host can render progress live.

use harness_llm::stream::{AssembledMessage, StreamEvent};
use harness_llm::types::ChatMessage;
use harness_llm::{Attachment, ChatRequest, ToolCall};
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::{budget, compact, prompt};

use super::{build_user_message, Agent};

const TOOL_ARGUMENT_EVENT_CHARS: usize = 16_000;
const TOOL_RESULT_EVENT_CHARS: usize = 4_000;

impl Agent {
    /// Run one user turn to completion, returning the assistant's final text.
    ///
    /// `on_event` is invoked for streamed tokens and tool activity so callers
    /// can render progress live.
    pub async fn run_turn<F>(
        &mut self,
        user_input: impl Into<String>,
        on_event: F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        self.run_turn_with_attachments(user_input, Vec::new(), on_event)
            .await
    }

    /// Run one user turn that may carry attachments (images/PDFs/videos dropped
    /// into the chat). Attachments become content parts on the user message;
    /// with none, this is identical to [`Agent::run_turn`].
    pub async fn run_turn_with_attachments<F>(
        &mut self,
        user_input: impl Into<String>,
        attachments: Vec<Attachment>,
        on_event: F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        self.push(build_user_message(
            user_input.into(),
            &attachments,
            self.attachments.as_ref(),
        )?)?;
        self.drive_turn(on_event).await
    }

    /// Retry a turn whose user message is already recorded but whose model call
    /// failed before producing a reply (e.g. a 401 before an API key was set).
    ///
    /// Unlike [`Agent::run_turn_with_attachments`], this appends **no** user
    /// message — it drives the same loop against the existing transcript, so the
    /// user's prompt isn't duplicated in the history (or a fine-tuning export).
    /// Call it only when the trailing message is the user turn to re-attempt.
    pub async fn continue_turn<F>(&mut self, on_event: F) -> Result<String, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        self.drive_turn(on_event).await
    }

    /// Drive the model/tool loop against the current transcript to a final reply.
    /// Shared by a fresh turn and a retry — the only difference is whether a user
    /// message was pushed first.
    ///
    /// Every terminal failure is also appended to the developer error log (see
    /// [`crate::errlog`]) so it stays debuggable after the UI moved on.
    async fn drive_turn<F>(&mut self, on_event: F) -> Result<String, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        let result = self.drive_turn_inner(on_event).await;
        if let Err(e) = &result {
            crate::errlog::record(
                self.config.error_log.as_deref(),
                "turn_failed",
                serde_json::json!({
                    "session": self.session_id(),
                    "model": self.config.model,
                    "endpoint": self.client.base_url(),
                    "kind": error_kind(e),
                    "error": e.to_string(),
                }),
            );
        }
        result
    }

    async fn drive_turn_inner<F>(&mut self, mut on_event: F) -> Result<String, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        // Tool definitions are fixed for the turn; compute once.
        let tool_defs = self.tools.definitions();
        let window = self.context_window();
        let budget = budget::prompt_budget(window, self.config.response_reserve);

        // A one-shot corrective for the "announce a plan, then stop" failure: if
        // the model returns a text-only reply that reads as intent-to-act, we
        // append this nudge to the *next* request only and let the loop run once
        // more. It's never persisted, so it stays out of both the stored
        // transcript and the visible chat. Capped at one nudge per turn.
        let mut nudge: Option<ChatMessage> = None;
        let mut nudged = false;

        // A second one-shot corrective, for the "one subtask failed, so the whole
        // checklist silently stalls" failure: if the model updates its plan this
        // turn and then ends the turn with items still unfinished (typically after
        // a tool error), nudge it once to keep working or reconcile the plan.
        // Tracks only plans updated *this* turn — an older incomplete plan must not
        // hijack an unrelated follow-up question. Never persisted.
        let mut plan_nudged = false;
        let mut plan_open_this_turn = false;

        // The stop signal for this turn (a clone, so cancelling it from the host
        // doesn't require the agent lock the turn is holding).
        let cancel = self.cancel.clone();

        // No fixed iteration cap: the loop runs until the model returns a final
        // answer, bounded only by how much fits in the context window.
        loop {
            // Honor a stop requested between model calls (e.g. while tools ran).
            if cancel.is_cancelled() {
                return Ok(String::new());
            }

            // Make room for the next request, then send it and fold the round's
            // token usage back into the running totals.
            let raw_prompt_tokens = self
                .fit_context(budget, window, &tool_defs, &mut on_event)
                .await?;
            let prompt_tokens = self.calibrated(raw_prompt_tokens);

            // Reflect this call's prompt cost the moment it's sent (the transcript
            // is `prompt_tokens` of context), so a live meter accounts for it now
            // rather than jumping when the reply finishes. The reply then streams
            // on top, and the post-call event below snaps to the exact figure.
            on_event(&AgentEvent::Usage {
                tokens_used: self.tokens_used + prompt_tokens,
                context_tokens: prompt_tokens,
                prompt_tokens_used: self.prompt_tokens_used + prompt_tokens,
                completion_tokens_used: self.completion_tokens_used,
            });

            // Compress stale tool output in the outbound copy (or, in audit
            // mode, just measure what compression would save). The in-memory
            // transcript and the store keep the originals either way.
            let (outbound, report) = self.prepare_outbound();
            if report.saved_chars > 0 {
                let saved_tokens =
                    self.calibrated(budget::estimate_tokens_for_chars(report.saved_chars));
                self.tokens_saved += saved_tokens;
                on_event(&AgentEvent::Compression {
                    mode: self.config.compression.as_str().to_string(),
                    saved_tokens,
                    total_saved_tokens: self.tokens_saved,
                    results_compressed: report.results_compressed,
                });
            }

            let assembled = self
                .stream_reply(outbound, &tool_defs, nudge.as_ref(), &cancel, &mut on_event)
                .await?;

            // A stop mid-stream returns whatever assembled so far. Persist only
            // the partial prose (a half-formed tool call would be malformed and
            // must not be replayed), keep it out of the token tally, and end the
            // turn cleanly so the UI settles to a normal reply rather than error.
            if cancel.is_cancelled() {
                if !assembled.content.is_empty() {
                    self.push(ChatMessage::assistant(assembled.content.clone()))?;
                }
                // The provider has already processed the prompt and generated
                // this partial reply. Count that spend even though the user
                // stopped before a final usage chunk arrived.
                if assembled.usage.is_some()
                    || !assembled.content.is_empty()
                    || !assembled.tool_calls.is_empty()
                {
                    self.account_for_usage(&assembled, raw_prompt_tokens, prompt_tokens);
                }
                return Ok(assembled.content);
            }

            self.account_for_usage(&assembled, raw_prompt_tokens, prompt_tokens);

            self.push(ChatMessage::assistant_with_tools(
                assembled.content.clone(),
                assembled.tool_calls.clone(),
            ))?;

            // The exact cumulative + context now that the reply is in the
            // transcript; the UI snaps its live estimate to this.
            on_event(&AgentEvent::Usage {
                tokens_used: self.tokens_used,
                context_tokens: self.context_tokens(),
                prompt_tokens_used: self.prompt_tokens_used,
                completion_tokens_used: self.completion_tokens_used,
            });

            if assembled.tool_calls.is_empty() {
                // The model replied with prose and no tool call. If it reads as
                // an announced-but-unperformed action, nudge it once to actually
                // emit the call; otherwise this is its final answer.
                if !nudged && prompt::looks_like_unfulfilled_intent(&assembled.content) {
                    nudged = true;
                    nudge = Some(ChatMessage::user(prompt::INTENT_NUDGE.to_string()));
                    continue;
                }
                // Ending the turn while this turn's own plan has unfinished items
                // is almost always a stall (a failed step made the model give up);
                // give it one chance to continue or tidy the checklist.
                if !plan_nudged && plan_open_this_turn {
                    plan_nudged = true;
                    nudge = Some(ChatMessage::user(prompt::PLAN_STALL_NUDGE.to_string()));
                    continue;
                }
                return Ok(assembled.content);
            }

            // A tool call landed; the corrective (if any) served its purpose.
            nudge = None;

            for call in &assembled.tool_calls {
                let result = self.run_tool(call, &mut on_event).await;
                // Track the latest plan state from successful `update_plan` calls
                // (invalid arguments were rejected, so they changed nothing).
                if call.function.name == harness_tools::PLAN_TOOL {
                    if let Some(items) =
                        harness_tools::parse_plan_arguments(&call.function.arguments)
                    {
                        plan_open_this_turn = harness_tools::plan_is_open(&items);
                    }
                }
                self.push(ChatMessage::tool_result(call.id.clone(), result))?;
            }
        }
    }

    /// Keep the next request within the context window, returning the raw
    /// (uncalibrated) prompt-token estimate for the transcript that will be sent.
    ///
    /// The estimate is calibrated by the latest real usage before the check, so
    /// it tracks reality (the raw code under-counts at ~4 chars/token). On
    /// overflow it compacts — pruning stale tool output, then summarizing old
    /// turns — rather than hard-stopping, and only errors if even a compacted
    /// transcript still can't fit.
    async fn fit_context<F>(
        &mut self,
        budget: usize,
        window: usize,
        tool_defs: &[serde_json::Value],
        on_event: &mut F,
    ) -> Result<usize, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        let resident_budget =
            budget::estimate_tokens_for_chars(self.config.max_resident_context_chars).min(budget);
        let raw = budget::estimate_prompt_tokens(&self.messages, tool_defs);
        if self.calibrated(raw) > resident_budget && resident_budget < budget {
            // Best effort: a single recent turn may legitimately exceed the soft
            // resident target. The real provider window remains authoritative.
            let _ = self
                .compact_to_fit(resident_budget, tool_defs, on_event)
                .await?;
        }
        let raw = budget::estimate_prompt_tokens(&self.messages, tool_defs);
        if self.calibrated(raw) <= budget {
            return Ok(raw);
        }
        let fit = self.compact_to_fit(budget, tool_defs, on_event).await?;
        let raw = budget::estimate_prompt_tokens(&self.messages, tool_defs);
        if !fit || self.calibrated(raw) > budget {
            return Err(AgentError::ContextWindowExceeded {
                used: self.calibrated(raw),
                window,
            });
        }
        Ok(raw)
    }

    /// Send the prepared outbound transcript (plus the optional one-shot
    /// nudge) to the model and stream the reply, translating provider stream
    /// events into [`AgentEvent`]s as they arrive.
    ///
    /// Transient failures (provider 5xx, rate limits, network blips) are
    /// retried with exponential backoff per [`AgentConfig::retry`], emitting
    /// [`AgentEvent::Retrying`] before each wait so the UI can show the hiccup.
    /// A stream that dies mid-reply retries too — nothing was persisted yet, so
    /// re-sending the same request is safe (the UI may show some text twice).
    /// Non-transient errors (auth, credits, bad request) fail immediately.
    ///
    /// [`AgentConfig::retry`]: crate::AgentConfig::retry
    async fn stream_reply<F>(
        &self,
        mut outbound: Vec<ChatMessage>,
        tool_defs: &[serde_json::Value],
        nudge: Option<&ChatMessage>,
        cancel: &CancellationToken,
        on_event: &mut F,
    ) -> Result<AssembledMessage, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        outbound.extend(nudge.cloned());
        let request = ChatRequest::new(&self.config.model, outbound)
            .with_tools(tool_defs.to_vec())
            .max_tokens(self.config.response_reserve)
            .streaming(true);

        let retry = self.config.retry.clone();
        let mut attempt: u32 = 1;
        loop {
            let result = self
                .client
                .stream_chat(&request, cancel, |event| match event {
                    StreamEvent::Token(t) => on_event(&AgentEvent::Token(t.clone())),
                    StreamEvent::ToolCallStart { name } => {
                        on_event(&AgentEvent::ToolPending { name: name.clone() })
                    }
                    StreamEvent::ToolCallDelta { name, arguments } => {
                        on_event(&AgentEvent::ToolDelta {
                            name: name.clone(),
                            delta: arguments.clone(),
                        })
                    }
                    StreamEvent::Done { .. } => {}
                })
                .await;

            match result {
                Ok(assembled) => return Ok(assembled),
                Err(e) if e.is_transient() && attempt < retry.max_attempts => {
                    let delay = retry.delay_after(attempt);
                    crate::errlog::record(
                        self.config.error_log.as_deref(),
                        "retrying",
                        serde_json::json!({
                            "session": self.session_id(),
                            "model": self.config.model,
                            "endpoint": self.client.base_url(),
                            "attempt": attempt,
                            "max_attempts": retry.max_attempts,
                            "delay_ms": delay.as_millis() as u64,
                            "error": e.to_string(),
                        }),
                    );
                    on_event(&AgentEvent::Retrying {
                        attempt,
                        max_attempts: retry.max_attempts,
                        delay_ms: delay.as_millis() as u64,
                        error: e.to_string(),
                    });
                    // A stop during the backoff wait ends the turn like any
                    // other cancellation: quietly, with nothing assembled.
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Ok(AssembledMessage::default()),
                        _ = tokio::time::sleep(delay) => {}
                    }
                    attempt += 1;
                }
                // Retries were burned and it's still down: report the full
                // picture (attempts, model, endpoint, last error) so the
                // failure is debuggable rather than a bare status code.
                Err(e) if attempt > 1 => {
                    return Err(AgentError::RetriesExhausted {
                        attempts: attempt,
                        model: self.config.model.clone(),
                        endpoint: self.client.base_url().to_string(),
                        source: e,
                    })
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Fold one model round's usage into the running totals: recalibrate the
    /// client-side estimate against the endpoint's real prompt size (so the next
    /// budget check tracks reality), then add this round's prompt + generated
    /// tokens — preferring the endpoint's reported counts, falling back to the
    /// calibrated estimate when it doesn't report any.
    fn account_for_usage(
        &mut self,
        assembled: &AssembledMessage,
        raw_prompt_tokens: usize,
        prompt_tokens: usize,
    ) {
        if let Some(usage) = &assembled.usage {
            if usage.prompt_tokens > 0 && raw_prompt_tokens > 0 {
                self.token_ratio = usage.prompt_tokens as f64 / raw_prompt_tokens as f64;
            }
        }
        let (prompt_delta, completion_delta) = match &assembled.usage {
            Some(u) if u.prompt_tokens + u.completion_tokens > 0 => {
                (u.prompt_tokens as usize, u.completion_tokens as usize)
            }
            _ => {
                let completion =
                    budget::estimate_completion_tokens(&assembled.content, &assembled.tool_calls);
                (prompt_tokens, completion)
            }
        };
        self.prompt_tokens_used += prompt_delta;
        self.completion_tokens_used += completion_delta;
        self.tokens_used += prompt_delta + completion_delta;
        self.record_usage(prompt_delta, completion_delta);
    }

    /// Free context so the next request fits `budget`, in two stages (see
    /// [`compact`]): prune stale tool output, then summarize the oldest turns.
    /// Emits an [`AgentEvent::Compacted`] for each stage that does work and
    /// returns whether the transcript now fits. Mutates only the in-memory
    /// transcript — the history store keeps the full record.
    async fn compact_to_fit<F>(
        &mut self,
        budget: usize,
        tool_defs: &[serde_json::Value],
        on_event: &mut F,
    ) -> Result<bool, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        // Keep the latest few tool outputs and the last few turns verbatim.
        const KEEP_RECENT_TOOLS: usize = 2;
        const KEEP_RECENT_TURNS: usize = 3;

        // Stage 1: prune stale tool output — cheap, no model call.
        let freed = compact::prune_tool_results(&mut self.messages, KEEP_RECENT_TOOLS);
        if freed > 0 {
            on_event(&AgentEvent::Compacted {
                detail: format!("pruned ~{freed} chars of older tool output"),
            });
        }
        if self.fits_budget(budget, tool_defs) {
            if freed > 0 {
                self.save_context_snapshot();
            }
            return Ok(true);
        }

        // Stage 2: summarize the oldest turns into a single message. The cut is
        // on a user-turn boundary, so no tool result is orphaned from its call.
        let Some(cut) = compact::summary_cut_index(&self.messages, KEEP_RECENT_TURNS) else {
            return Ok(self.fits_budget(budget, tool_defs));
        };
        let start = usize::from(self.messages.first().is_some_and(|m| m.role == "system"));
        let rendered = compact::render_for_summary(&self.messages[start..cut]);
        let summary = self.complete(compact::SUMMARY_PROMPT, &rendered).await?;
        let note = ChatMessage::user(format!("{}\n{}", compact::SUMMARY_MARKER, summary));
        self.messages.splice(start..cut, std::iter::once(note));
        self.save_context_snapshot();
        on_event(&AgentEvent::Compacted {
            detail: "summarized earlier conversation".to_string(),
        });
        Ok(self.fits_budget(budget, tool_defs))
    }

    /// Run one tool call, bracketing it with [`AgentEvent::ToolStart`] and
    /// [`AgentEvent::ToolEnd`]. Failures come back as ordinary `tool error: …`
    /// results, so the model can read the error and self-correct in the turn.
    async fn run_tool<F>(&self, call: &ToolCall, on_event: &mut F) -> String
    where
        F: FnMut(&AgentEvent),
    {
        on_event(&AgentEvent::ToolStart {
            name: call.function.name.clone(),
            arguments: harness_core::text::truncate_with_marker(
                &call.function.arguments,
                TOOL_ARGUMENT_EVENT_CHARS,
                "\n… [arguments omitted from display]",
            ),
        });

        let result = match call.function.parsed_arguments() {
            Ok(args) => match self.tools.invoke(&call.function.name, args).await {
                Ok(output) => output,
                Err(e) => format!("tool error: {e}"),
            },
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
}

/// A stable machine-readable tag for an [`AgentError`] variant, so the error
/// log can be filtered (`jq 'select(.kind == "retries_exhausted")'`) without
/// parsing display strings.
fn error_kind(e: &AgentError) -> &'static str {
    match e {
        AgentError::Llm(_) => "llm",
        AgentError::Tool(_) => "tool",
        AgentError::History(_) => "history",
        AgentError::Io(_) => "io",
        AgentError::Json(_) => "json",
        AgentError::AttachmentsTooLarge { .. } => "attachments_too_large",
        AgentError::ContextWindowExceeded { .. } => "context_window_exceeded",
        AgentError::RetriesExhausted { .. } => "retries_exhausted",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use harness_llm::{LlmError, OxenClient};
    use harness_store::HistoryStore;
    use harness_tools::ToolRegistry;

    use crate::test_support::{sse_prose, test_session};
    use crate::{Agent, AgentConfig, AgentError, AgentEvent, RetryPolicy};

    use super::*;

    #[tokio::test]
    async fn run_turn_stops_when_context_window_is_exhausted() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        // A 1-token window can't fit any real prompt, so the budget check trips
        // on the first iteration — before any network call is attempted.
        let config = AgentConfig {
            model: "claude-opus-4-8".into(),
            system_prompt: None,
            context_window: Some(1),
            response_reserve: 0,
            ..AgentConfig::default()
        };
        let client = OxenClient::new("http://127.0.0.1:1/api/ai", "key", "claude-opus-4-8");
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        let err = agent
            .run_turn("please do something that needs more than one token", |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::ContextWindowExceeded { .. }));
    }

    #[tokio::test]
    async fn run_turn_stops_immediately_when_cancelled() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        // Point at an unroutable address: if cancellation didn't short-circuit
        // before the network call, the turn would hang/err on connect instead of
        // returning cleanly.
        let client = OxenClient::new("http://127.0.0.1:1/api/ai", "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        // Pre-cancel the turn's stop signal; the loop bails before any request.
        let token = CancellationToken::new();
        token.cancel();
        agent.set_cancel_token(token);

        let out = agent.run_turn("do a lot of work", |_| {}).await.unwrap();
        assert_eq!(out, "");
        // Only the user message was persisted — no assistant reply for a turn that
        // never reached the model.
        assert_eq!(agent.messages().last().unwrap().role, "user");
    }

    #[tokio::test]
    async fn continue_turn_retries_without_duplicating_the_user_message() {
        // A failed turn leaves its user message in the transcript; retrying via
        // continue_turn (after e.g. authenticating past a 401) must not re-add it.
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");

        // First attempt: an unroutable endpoint makes the model call fail after
        // the user message is pushed — exactly the shape of a 401 mid-turn.
        let dead = OxenClient::new("http://127.0.0.1:1/api/ai", "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            context_window: Some(128_000),
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(dead, ToolRegistry::new(), store, session, config).unwrap();

        agent
            .run_turn("Write me a README", |_| {})
            .await
            .expect_err("the first attempt should fail to reach the model");
        assert_eq!(
            agent.messages().iter().filter(|m| m.role == "user").count(),
            1,
            "the failed turn should have recorded exactly one user message"
        );

        // Now the key is set: swap in a working client and continue the turn.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("done"))
            .create_async()
            .await;
        agent.set_client(OxenClient::new(server.url(), "key", "claude-opus-4-8"));

        let out = agent.continue_turn(|_| {}).await.unwrap();
        assert_eq!(out, "done");
        // Still exactly one user message (not duplicated), now followed by the reply.
        assert_eq!(
            agent.messages().iter().filter(|m| m.role == "user").count(),
            1,
            "the retry must not append a second copy of the user prompt"
        );
        assert_eq!(agent.messages().last().unwrap().role, "assistant");
    }

    /// SSE for a reply that calls `update_plan` with a single item in `status`,
    /// alongside a bit of prose.
    fn sse_plan_update(status: &str) -> String {
        let plan_args = serde_json::json!({
            "plan": [{ "content": "Research", "active_form": "Researching", "status": status }]
        })
        .to_string();
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "content": "Working on it.",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "update_plan", "arguments": plan_args }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        format!("data: {chunk}\n\ndata: [DONE]\n\n")
    }

    fn plan_test_agent(url: String, store: Arc<HistoryStore>) -> Agent {
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(url, "key", "claude-opus-4-8");
        let mut tools = ToolRegistry::new();
        tools.register_typed(harness_tools::PlanTool::new());
        let config = AgentConfig {
            system_prompt: None,
            ..AgentConfig::default()
        };
        Agent::new(client, tools, store, session, config).unwrap()
    }

    #[tokio::test]
    async fn plan_stall_nudge_fires_when_a_turn_abandons_an_open_plan() {
        let mut server = mockito::Server::new_async().await;
        // Mockito serves the most recently defined matching mock, so these read
        // bottom-up: the base reply lays out an open plan; a request carrying
        // the recorded plan result gets the stall (prose, plan unfinished); a
        // request carrying the nudge gets the recovery.
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_plan_update("in_progress"))
            .create_async()
            .await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("0/1 done".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose(
                "The search failed, so that is where things stand.",
            ))
            .create_async()
            .await;
        let recovery = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("unfinished items".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("Recovered: reconciled the plan."))
            .expect(1)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let mut agent = plan_test_agent(server.url(), store);

        let out = agent.run_turn("research this topic", |_| {}).await.unwrap();
        assert_eq!(out, "Recovered: reconciled the plan.");
        recovery.assert_async().await;

        // The nudge is a request-only corrective — never persisted to the
        // transcript, and the user's single message stays the only user turn.
        assert!(agent.messages().iter().all(|m| !m
            .content_text()
            .unwrap_or_default()
            .contains("unfinished items")));
        assert_eq!(
            agent.messages().iter().filter(|m| m.role == "user").count(),
            1
        );
    }

    #[tokio::test]
    async fn no_plan_stall_nudge_when_the_plan_is_complete() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_plan_update("completed"))
            .create_async()
            .await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("1/1 done".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("All done."))
            .create_async()
            .await;
        let nudge = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("unfinished items".into()))
            .expect(0)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let mut agent = plan_test_agent(server.url(), store);

        let out = agent.run_turn("research this topic", |_| {}).await.unwrap();
        assert_eq!(out, "All done.");
        nudge.assert_async().await;
    }

    /// A retry policy with near-zero waits so backoff tests run instantly.
    fn fast_retry(max_attempts: u32) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            base_delay: std::time::Duration::from_millis(1),
        }
    }

    fn retry_test_agent(url: String, retry: RetryPolicy) -> Agent {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(url, "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            retry,
            ..AgentConfig::default()
        };
        Agent::new(client, ToolRegistry::new(), store, session, config).unwrap()
    }

    #[tokio::test]
    async fn transient_provider_errors_are_retried_until_the_call_lands() {
        let mut server = mockito::Server::new_async().await;
        // Mockito serves the first matching mock that hasn't met its expected
        // hits: the 502 mock absorbs the first two calls, then the SSE mock
        // answers the third — a provider that hiccups twice and recovers.
        let bad = server
            .mock("POST", "/chat/completions")
            .with_status(502)
            .with_body(r#"{"error":{"title":"The model provider returned an error."}}"#)
            .expect(2)
            .create_async()
            .await;
        let good = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("recovered"))
            .expect(1)
            .create_async()
            .await;

        let mut agent = retry_test_agent(server.url(), fast_retry(4));
        let mut retries = Vec::new();
        let out = agent
            .run_turn("hello", |e| {
                if let AgentEvent::Retrying {
                    attempt,
                    max_attempts,
                    error,
                    ..
                } = e
                {
                    retries.push((*attempt, *max_attempts, error.clone()));
                }
            })
            .await
            .expect("the turn should survive two 502s and finish");

        assert_eq!(out, "recovered");
        // One Retrying event per failed attempt, numbered and carrying the error.
        assert_eq!(retries.len(), 2);
        assert_eq!((retries[0].0, retries[0].1), (1, 4));
        assert_eq!((retries[1].0, retries[1].1), (2, 4));
        assert!(retries[0].2.contains("502"), "event should carry the error");
        bad.assert_async().await;
        good.assert_async().await;
    }

    #[tokio::test]
    async fn failures_are_appended_to_the_error_log() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(502)
            .with_body(r#"{"error":{"title":"The model provider returned an error."}}"#)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("errors.jsonl");
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            retry: fast_retry(2),
            error_log: Some(log.clone()),
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        agent.run_turn("hello", |_| {}).await.unwrap_err();

        let body = std::fs::read_to_string(&log).unwrap();
        let entries: Vec<serde_json::Value> = body
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        // One "retrying" entry for the backoff attempt, then the terminal
        // failure — each stamped and self-describing for later digging.
        assert_eq!(entries.len(), 2, "log should hold retry + failure: {body}");
        assert_eq!(entries[0]["event"], "retrying");
        assert_eq!(entries[0]["attempt"], 1);
        assert!(entries[0]["error"].as_str().unwrap().contains("502"));
        assert_eq!(entries[1]["event"], "turn_failed");
        assert_eq!(entries[1]["kind"], "retries_exhausted");
        assert_eq!(entries[1]["model"], "claude-opus-4-8");
        assert_eq!(entries[1]["endpoint"], server.url());
        assert!(entries[1]["ts"].as_str().unwrap().ends_with('Z'));
    }

    #[tokio::test]
    async fn a_stream_cut_off_mid_reply_is_retried() {
        let mut server = mockito::Server::new_async().await;
        // First call: a 200 whose SSE body stops mid-reply — tokens flowed but
        // no finish reason / [DONE] ever arrived (an upstream timeout dropping
        // the connection). The turn must retry, not end on the truncated text.
        let cut = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"I'll rewrite \"}}]}\n\n",
            )
            .expect(1)
            .create_async()
            .await;
        let good = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("recovered"))
            .expect(1)
            .create_async()
            .await;

        let mut agent = retry_test_agent(server.url(), fast_retry(4));
        let mut retries = Vec::new();
        let out = agent
            .run_turn("hello", |e| {
                if let AgentEvent::Retrying { error, .. } = e {
                    retries.push(error.clone());
                }
            })
            .await
            .expect("the turn should survive a cut-off stream and finish");

        assert_eq!(out, "recovered");
        assert_eq!(retries.len(), 1);
        assert!(
            retries[0].contains("connection closed"),
            "event should say the stream was cut off: {}",
            retries[0]
        );
        cut.assert_async().await;
        good.assert_async().await;
    }

    #[tokio::test]
    async fn retries_exhausted_reports_attempts_model_and_endpoint() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(502)
            .with_body(r#"{"error":{"title":"The model provider returned an error."}}"#)
            .create_async()
            .await;

        let mut agent = retry_test_agent(server.url(), fast_retry(2));
        let err = agent.run_turn("hello", |_| {}).await.unwrap_err();

        match &err {
            AgentError::RetriesExhausted {
                attempts,
                model,
                endpoint,
                source,
            } => {
                assert_eq!(*attempts, 2);
                assert_eq!(model, "claude-opus-4-8");
                assert_eq!(endpoint, &server.url());
                assert!(matches!(source, LlmError::Api { status: 502, .. }));
            }
            other => panic!("expected RetriesExhausted, got {other:?}"),
        }
        // The display alone should carry everything needed to debug it.
        let msg = err.to_string();
        assert!(msg.contains("2 times"), "attempts missing from: {msg}");
        assert!(msg.contains("claude-opus-4-8"), "model missing from: {msg}");
        assert!(msg.contains("502"), "status missing from: {msg}");
    }

    #[tokio::test]
    async fn non_transient_errors_fail_fast_without_retry() {
        let mut server = mockito::Server::new_async().await;
        // expect(1): a retried 401 would trip this mock's assertion below.
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(401)
            .with_body(r#"{"error":{"message":"Invalid API key"}}"#)
            .expect(1)
            .create_async()
            .await;

        let mut agent = retry_test_agent(server.url(), fast_retry(4));
        let mut retried = false;
        let err = agent
            .run_turn("hello", |e| {
                if matches!(e, AgentEvent::Retrying { .. }) {
                    retried = true;
                }
            })
            .await
            .unwrap_err();

        assert!(!retried, "a 401 must not be retried");
        // Still the plain Llm error, so hosts' auth handling (the inline
        // key-entry card, the /auth hint) keeps matching on it.
        assert!(matches!(
            err,
            AgentError::Llm(LlmError::Api { status: 401, .. })
        ));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn run_turn_compacts_instead_of_erroring_when_over_budget() {
        // A streaming endpoint that returns a short final answer.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("all done"))
            .create_async()
            .await;

        // Seed a transcript with three big tool results — over a small window.
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "qwen3-8b");
        let big = "x".repeat(8000); // ~2000 tokens each
        for (i, _) in (0..3).enumerate() {
            store
                .append_message(&session, &ChatMessage::user(format!("q{i}")))
                .unwrap();
            store
                .append_message(
                    &session,
                    &ChatMessage::tool_result(format!("t{i}"), big.clone()),
                )
                .unwrap();
        }

        let client = OxenClient::new(server.url(), "key", "qwen3-8b");
        let config = AgentConfig {
            model: "qwen3-8b".into(),
            system_prompt: None,
            // Fits two of the three big tool results, not all three.
            context_window: Some(4500),
            response_reserve: 0,
            ..AgentConfig::default()
        };
        let mut agent =
            Agent::resume_from_store(client, ToolRegistry::new(), store, session, config).unwrap();

        let mut compacted = false;
        let out = agent
            .run_turn("continue", |e| {
                if matches!(e, AgentEvent::Compacted { .. }) {
                    compacted = true;
                }
            })
            .await
            .expect("turn should compact and succeed, not error");

        assert_eq!(out, "all done");
        assert!(compacted, "a Compacted event should have fired");
        // The oldest tool result was stubbed; the newest stays verbatim.
        let tool_texts: Vec<String> = agent
            .messages()
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.content_text())
            .collect();
        assert!(tool_texts.first().unwrap().contains("elided"));
        assert!(tool_texts.last().unwrap().contains(&big));

        // The compact working set is durable: a cold resume must not inflate
        // the full verbatim transcript back into memory.
        let store = agent.store.clone();
        let session = agent.session_id().to_string();
        let config = agent.config.clone();
        drop(agent);
        let resumed = Agent::resume_from_store(
            OxenClient::new(server.url(), "key", "qwen3-8b"),
            ToolRegistry::new(),
            store,
            session,
            config,
        )
        .unwrap();
        let first_tool = resumed
            .messages()
            .iter()
            .find(|m| m.role == "tool")
            .and_then(ChatMessage::content_text)
            .unwrap();
        assert!(first_tool.contains("elided"));
    }
}
