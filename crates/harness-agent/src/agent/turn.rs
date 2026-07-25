//! The turn loop: drive the model/tool cycle from a user message to a final
//! reply.
//!
//! One turn is a sequence of model calls. Each call may request tool calls;
//! the loop runs them, appends the results, and calls the model again until it
//! answers in prose. Along the way the loop budgets the context window
//! (compacting when it would overflow), retries transient model failures with
//! backoff, and emits [`AgentEvent`]s so a host can render progress live.

use harness_llm::types::ChatMessage;
use harness_llm::Attachment;

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::{budget, cache, prompt};

use super::call::error_kind;
use super::{build_user_message, Agent};

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
        let attachments = self.attachments_for_turn(attachments)?;
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
        // Request fingerprinting feeds only the request log's cache
        // diagnostics; when no log is configured (fleet/review subagents, the
        // default config) skip the per-round transcript serialization entirely
        // — an image-bearing transcript is megabytes of JSON per hash pass.
        // Tool defs are fixed for the turn, so their hash is too.
        let tools_hash = self
            .config
            .request_log
            .is_some()
            .then(|| cache::hash_tools(&tool_defs));
        let window = self.context_window();
        let budget = budget::prompt_budget(window, self.config.effective_response_reserve());

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

        // Consecutive degenerate rounds: a reply with no prose and no tool
        // calls (a provider anomaly — the stream completed but carried
        // nothing). Re-sampled a bounded number of times before giving up, so
        // one bad generation doesn't silently end the turn with an empty
        // answer, and a provider stuck returning nothing can't loop forever.
        const MAX_EMPTY_RESAMPLES: u32 = 2;
        let mut empty_rounds: u32 = 0;

        // Unproductive-loop tracking: identical (tool, arguments, result)
        // repeats get one nudge, then end the turn (see [`crate::loopguard`]).
        let mut loop_guard = crate::loopguard::LoopGuard::default();
        // The soft budget warning is logged once per turn, not per round.
        let mut budget_warned = false;

        // No fixed iteration cap: the loop runs until the model returns a final
        // answer, bounded only by how much fits in the context window.
        loop {
            // Honor a stop requested between model calls (e.g. while tools ran).
            if cancel.is_cancelled() {
                return Ok(String::new());
            }

            // Messages the user sent while the last round ran enter the
            // transcript here — before the next model call — so steering
            // lands mid-work, not after the turn ends.
            self.drain_interjections()?;

            // Make room for the next request, then send it and fold the round's
            // token usage back into the running totals.
            let raw_prompt_tokens = self
                .fit_context(budget, window, &tool_defs, &mut on_event)
                .await?;
            let prompt_tokens = self.calibrated(raw_prompt_tokens);

            // Session budget: refuse a call whose prompt alone would push the
            // session past its ceiling — stop gracefully with state preserved
            // rather than silently running past the cap.
            if let Some(session_budget) = self.config.budget {
                if self.tokens_used + prompt_tokens > session_budget.max_session_tokens {
                    let message = format!(
                        "Session token budget reached (~{} used of {} allowed; the next \
                         call needs ~{prompt_tokens} more). Stopping here so spending \
                         stays inside the cap. All work so far is saved — raise the \
                         budget or start a new session to continue.",
                        self.tokens_used, session_budget.max_session_tokens
                    );
                    crate::errlog::record(
                        self.config.error_log.as_deref(),
                        "budget_exhausted",
                        serde_json::json!({
                            "session": self.session_id(),
                            "model": self.config.model,
                            "tokens_used": self.tokens_used,
                            "max_session_tokens": session_budget.max_session_tokens,
                        }),
                    );
                    self.push(ChatMessage::assistant(message.clone()))?;
                    return Ok(message);
                }
                if !budget_warned && self.tokens_used >= session_budget.warn_threshold() {
                    budget_warned = true;
                    crate::errlog::record(
                        self.config.error_log.as_deref(),
                        "budget_warning",
                        serde_json::json!({
                            "session": self.session_id(),
                            "tokens_used": self.tokens_used,
                            "max_session_tokens": session_budget.max_session_tokens,
                        }),
                    );
                }
            }

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

            // Fingerprint what's about to be sent and classify it against the
            // previous request, so a cache miss is attributable (append-only
            // requests are the shape a provider prefix cache rewards). Only
            // when the request log will actually record it.
            let (prefix_diff, tools_changed) = match tools_hash {
                Some(tools_hash) => {
                    let request_hashes = cache::fingerprints(&outbound);
                    let prefix_diff =
                        cache::diff_prefix(&self.prev_request_hashes, &request_hashes);
                    let tools_changed = self.prev_tools_hash.is_some_and(|prev| prev != tools_hash);
                    self.prev_request_hashes = request_hashes;
                    self.prev_tools_hash = Some(tools_hash);
                    (prefix_diff, tools_changed)
                }
                None => (cache::PrefixDiff::First, false),
            };
            let outbound_len = outbound.len();

            let (assembled, mut outcome) = self
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
                    self.account_for_usage(&assembled, raw_prompt_tokens, prompt_tokens, outcome);
                }
                return Ok(assembled.content);
            }

            outcome = self.account_for_usage(&assembled, raw_prompt_tokens, prompt_tokens, outcome);
            self.log_request(
                prompt_tokens,
                outbound_len,
                &assembled,
                &outcome,
                prefix_diff,
                tools_changed,
            );

            // A round that produced neither prose nor a tool call is
            // re-sampled (nothing is persisted, so re-asking is safe); past
            // the bound it falls through and ends the turn empty as before.
            if assembled.content.is_empty() && assembled.tool_calls.is_empty() {
                if empty_rounds < MAX_EMPTY_RESAMPLES {
                    empty_rounds += 1;
                    crate::errlog::record(
                        self.config.error_log.as_deref(),
                        "empty_reply_resampled",
                        serde_json::json!({
                            "session": self.session_id(),
                            "model": self.config.model,
                            "attempt": empty_rounds,
                            "max_attempts": MAX_EMPTY_RESAMPLES,
                        }),
                    );
                    continue;
                }
            } else {
                empty_rounds = 0;
            }

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
                // A message the user sent while the final reply streamed must
                // be seen before the turn ends: drain it and go around again.
                // Checked before the nudges — a real user message outranks a
                // synthetic corrective.
                if self.drain_interjections()? {
                    continue;
                }
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

            // Set when identical repeats hit the stop line — the turn ends
            // after this round's results are recorded.
            let mut loop_stop: Option<(String, u32)> = None;

            // A reply cut off at the response token limit leaves its trailing
            // tool call's JSON unfinished (e.g. a large `write_file`).
            let reply_truncated = matches!(
                assembled.finish_reason.as_deref(),
                Some("length" | "max_tokens")
            );

            for call in &assembled.tool_calls {
                let result = self.run_tool(call, reply_truncated, &mut on_event).await;
                match loop_guard.observe(&call.function.name, &call.function.arguments, &result) {
                    crate::loopguard::LoopVerdict::Fine => {}
                    crate::loopguard::LoopVerdict::Nudge => {
                        nudge = Some(ChatMessage::user(prompt::LOOP_NUDGE.to_string()));
                    }
                    crate::loopguard::LoopVerdict::Stop { name, repeats } => {
                        loop_stop = Some((name, repeats));
                    }
                }
                // Track the latest plan state from successful `update_plan` calls
                // (invalid arguments were rejected, so they changed nothing).
                if call.function.name == harness_tools::PLAN_TOOL {
                    if let Some(items) =
                        harness_tools::parse_plan_arguments(&call.function.arguments)
                    {
                        plan_open_this_turn = harness_tools::plan_is_open(&items);
                    }
                }
                // A tool that produced an image (e.g. the preview screenshot)
                // marks it in-band; the `tool` role is text-only, so the image
                // rides in as a user message right after the result.
                match harness_core::attach::extract_image_markers(&result, "(image attached below)")
                {
                    Some((cleaned, paths)) => {
                        self.push(ChatMessage::tool_result(call.id.clone(), cleaned))?;
                        self.push_tool_images(&paths)?;
                    }
                    None => self.push(ChatMessage::tool_result(call.id.clone(), result))?,
                }
            }

            // An unproductive loop hit the stop line: every round was
            // re-billing the whole context for an identical result, and the
            // nudge didn't break the cycle. End the turn with the state
            // preserved rather than letting it spin.
            if let Some((name, repeats)) = loop_stop {
                let message = format!(
                    "Stopping this turn: the `{name}` tool was called with identical \
                     arguments and returned an identical result {repeats} times in a row, \
                     so continuing would spend tokens without making progress. Tell me how \
                     you'd like to proceed, or rephrase the request."
                );
                crate::errlog::record(
                    self.config.error_log.as_deref(),
                    "loop_detected",
                    serde_json::json!({
                        "session": self.session_id(),
                        "model": self.config.model,
                        "tool": name,
                        "repeats": repeats,
                    }),
                );
                self.push(ChatMessage::assistant(message.clone()))?;
                return Ok(message);
            }
        }
    }

    /// Move everything the user sent mid-turn into the transcript, each as
    /// its own user message (FIFO, never merged — see [`crate::interject`]).
    /// Returns whether anything was drained, so the caller can force another
    /// model round when the turn was about to end.
    fn drain_interjections(&mut self) -> Result<bool, AgentError> {
        let pending = self.interjections.take_all();
        let drained = !pending.is_empty();
        for text in pending {
            self.push(ChatMessage::user(prompt::clip_interjection(&text)))?;
        }
        Ok(drained)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use harness_llm::OxenClient;
    use harness_store::HistoryStore;
    use harness_tools::ToolRegistry;

    use crate::test_support::{
        fast_retry, retry_test_agent, sse_prose, sse_snap_call, test_session,
    };
    use crate::{Agent, AgentConfig, AgentEvent};
    use tokio_util::sync::CancellationToken;

    use super::*;

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

    /// SSE for a reply whose `write_file` call was cut off at the response
    /// token limit: unterminated arguments JSON, `finish_reason: "length"`.
    fn sse_truncated_tool_call() -> String {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {
                            "name": "write_file",
                            "arguments": "{\"path\":\"index.html\",\"contents\":\"<!doctype h"
                        }
                    }]
                },
                "finish_reason": "length"
            }]
        });
        format!("data: {chunk}\n\ndata: [DONE]\n\n")
    }

    #[tokio::test]
    async fn a_tool_call_truncated_at_the_output_limit_gets_a_targeted_error() {
        let mut server = mockito::Server::new_async().await;
        // Bottom-up: the base reply is the truncated call; the follow-up
        // carrying its tool result must see the split-the-work error, not a
        // bare JSON parse failure.
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_truncated_tool_call())
            .create_async()
            .await;
        let recovery = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("output-token limit".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("The write was too large and needs splitting."))
            .expect(1)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        let out = agent.run_turn("build the page", |_| {}).await.unwrap();
        assert_eq!(out, "The write was too large and needs splitting.");
        recovery.assert_async().await;
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

    #[tokio::test]
    async fn image_marker_in_a_tool_result_attaches_the_image() {
        // A tool that "captures" an image file and marks it in its result.
        struct SnapTool(std::path::PathBuf);
        #[derive(serde::Deserialize, schemars::JsonSchema)]
        struct SnapArgs {}
        #[async_trait::async_trait]
        impl harness_tools::TypedTool for SnapTool {
            const NAME: &'static str = "snap";
            type Args = SnapArgs;
            fn description(&self) -> &str {
                "take a snapshot"
            }
            async fn run(&self, _: SnapArgs) -> Result<String, harness_tools::ToolError> {
                // A minimal 1x1 PNG so attachment classification sees an image.
                const PNG: &[u8] = &[
                    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D',
                    b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89,
                ];
                std::fs::write(&self.0, PNG).unwrap();
                Ok(format!(
                    "Captured the preview. {}",
                    harness_core::attach::image_marker(&self.0.display().to_string())
                ))
            }
        }

        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_snap_call())
            .create_async()
            .await;
        // The follow-up call (the one carrying the image) ends the turn.
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("image attached below".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("looks good"))
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let mut tools = ToolRegistry::new();
        tools.register_typed(SnapTool(dir.path().join("shot.png")));
        let config = AgentConfig {
            system_prompt: None,
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, tools, store, session, config).unwrap();

        let out = agent.run_turn("check the preview", |_| {}).await.unwrap();
        assert_eq!(out, "looks good");

        // The tool result the model reads has the marker replaced…
        let tool_msg = agent
            .messages()
            .iter()
            .find(|m| m.role == "tool")
            .and_then(ChatMessage::content_text)
            .unwrap();
        assert!(tool_msg.contains("(image attached below)"), "{tool_msg}");
        assert!(!tool_msg.contains("<<attach-image:"), "{tool_msg}");
        // …and the image follows as a multimodal user message (text + image).
        let image_msg = agent
            .messages()
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .unwrap();
        match &image_msg.content {
            Some(harness_llm::types::MessageContent::Parts(parts)) => {
                assert_eq!(parts.len(), 2, "text part + image part");
            }
            other => panic!("expected multimodal user message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_interjection_sent_during_the_final_reply_forces_another_round() {
        let mut server = mockito::Server::new_async().await;
        // The follow-up round is the request that carries the interjection;
        // define it first so body matching routes it here.
        let followup = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("also check the tests".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("addressed the steering"))
            .expect(1)
            .create_async()
            .await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("first answer"))
            .expect(1)
            .create_async()
            .await;

        let mut agent = retry_test_agent(server.url(), fast_retry(2));
        let handle = agent.interjections();

        // Push from the event callback — i.e. while the first reply is
        // streaming, exactly when a real user would be typing.
        let pushed = std::cell::Cell::new(false);
        let out = agent
            .run_turn("hello", |e| {
                if matches!(e, AgentEvent::Token(_)) && !pushed.get() {
                    pushed.set(true);
                    handle.push("wait — also check the tests");
                }
            })
            .await
            .unwrap();

        // The turn did not end on "first answer": the interjection forced a
        // second round whose reply is the final one.
        assert_eq!(out, "addressed the steering");
        // The transcript carries the user's clean text — no framing wrapper
        // (the store is verbatim history: renderers and exports read it back).
        let stored = agent
            .messages()
            .iter()
            .find(|m| {
                m.role == "user"
                    && m.content_text()
                        .is_some_and(|t| t.contains("also check the tests"))
            })
            .expect("the interjection should be in the transcript");
        assert_eq!(
            stored.content_text().as_deref(),
            Some("wait — also check the tests")
        );
        followup.assert_async().await;
    }

    #[tokio::test]
    async fn interjections_pending_at_turn_start_land_before_the_first_model_call() {
        let mut server = mockito::Server::new_async().await;
        // One call total: both the prompt and the interjection are in it.
        let only = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex(
                "left over from the last turn".into(),
            ))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("done"))
            .expect(1)
            .create_async()
            .await;

        let mut agent = retry_test_agent(server.url(), fast_retry(2));
        agent.interjections().push("left over from the last turn");

        let out = agent.run_turn("hello", |_| {}).await.unwrap();
        assert_eq!(out, "done");
        only.assert_async().await;
        // FIFO in the transcript: prompt first, then the framed interjection.
        let users: Vec<String> = agent
            .messages()
            .iter()
            .filter(|m| m.role == "user")
            .filter_map(|m| m.content_text())
            .collect();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0], "hello");
        assert_eq!(users[1], "left over from the last turn");
    }

    #[tokio::test]
    async fn an_empty_reply_is_resampled_instead_of_ending_the_turn_silently() {
        let mut server = mockito::Server::new_async().await;
        // First call: the stream completes cleanly but carries no content and
        // no tool calls — a degenerate generation. The turn must ask again.
        let empty = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose(""))
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

        let mut agent = retry_test_agent(server.url(), fast_retry(2));
        let out = agent.run_turn("hello", |_| {}).await.unwrap();

        assert_eq!(out, "recovered");
        // The degenerate round left nothing behind: exactly one assistant
        // message, the real one.
        let assistant_count = agent
            .messages()
            .iter()
            .filter(|m| m.role == "assistant")
            .count();
        assert_eq!(assistant_count, 1);
        empty.assert_async().await;
        good.assert_async().await;
    }

    #[tokio::test]
    async fn empty_reply_resampling_is_bounded() {
        let mut server = mockito::Server::new_async().await;
        // Every call returns the degenerate empty reply: first round + two
        // re-samples = exactly three calls, then the turn ends empty rather
        // than looping forever.
        let empty = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose(""))
            .expect(3)
            .create_async()
            .await;

        let mut agent = retry_test_agent(server.url(), fast_retry(2));
        let out = agent.run_turn("hello", |_| {}).await.unwrap();

        assert_eq!(out, "");
        empty.assert_async().await;
    }

    #[tokio::test]
    async fn session_budget_stops_the_turn_before_the_model_is_called() {
        // An unroutable endpoint proves the stop happens before any network
        // call — the budget check refuses the request outright.
        let client = OxenClient::new("http://127.0.0.1:1/api/ai", "key", "claude-opus-4-8");
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            budget: Some(crate::SessionBudget::new(1)),
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        let out = agent.run_turn("do something big", |_| {}).await.unwrap();
        assert!(out.contains("budget"), "should explain the stop: {out}");
        // The explanation is a normal assistant message, so the transcript
        // (and any resumed session) records why the turn ended.
        assert_eq!(agent.messages().last().unwrap().role, "assistant");
    }

    /// SSE for a reply that always makes the same `echo` tool call.
    fn sse_echo_call() -> String {
        let chunk = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "content": "",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_echo",
                        "function": { "name": "echo", "arguments": "{}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        format!("data: {chunk}\n\ndata: [DONE]\n\n")
    }

    #[tokio::test]
    async fn an_identical_tool_loop_is_nudged_then_stopped() {
        struct EchoTool;
        #[derive(serde::Deserialize, schemars::JsonSchema)]
        struct EchoArgs {}
        #[async_trait::async_trait]
        impl harness_tools::TypedTool for EchoTool {
            const NAME: &'static str = "echo";
            type Args = EchoArgs;
            fn description(&self) -> &str {
                "always returns the same thing"
            }
            async fn run(&self, _: EchoArgs) -> Result<String, harness_tools::ToolError> {
                Ok("the same output".into())
            }
        }

        let mut server = mockito::Server::new_async().await;
        // The model is stuck: every request gets the identical tool call back.
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_echo_call())
            .create_async()
            .await;
        // The corrective nudge must appear in at least one request.
        let nudged = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("identical arguments".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_echo_call())
            .expect_at_least(1)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let mut tools = ToolRegistry::new();
        tools.register_typed(EchoTool);
        let config = AgentConfig {
            system_prompt: None,
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, tools, store, session, config).unwrap();

        let out = agent.run_turn("loop forever", |_| {}).await.unwrap();
        assert!(
            out.contains("identical") && out.contains("echo"),
            "should stop with an explanation naming the tool: {out}"
        );
        nudged.assert_async().await;
        // Exactly STOP_AFTER identical executions happened — not an unbounded spin.
        let tool_results = agent.messages().iter().filter(|m| m.role == "tool").count();
        assert_eq!(tool_results as u32, crate::loopguard::STOP_AFTER);
    }
}
