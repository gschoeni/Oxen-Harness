//! The turn loop: drive the model/tool cycle from a user message to a final
//! reply.
//!
//! One turn is a sequence of model calls. Each call may request tool calls;
//! the loop runs them, appends the results, and calls the model again until it
//! answers in prose. Along the way the loop budgets the context window
//! (compacting when it would overflow), retries transient model failures with
//! backoff, and emits [`AgentEvent`]s so a host can render progress live.

use harness_llm::stream::AssembledMessage;
use harness_llm::types::ChatMessage;
use harness_llm::Attachment;

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::{budget, cache, prompt};

use super::call::error_kind;
use super::{build_user_message, Agent};

/// Consecutive degenerate rounds tolerated before a turn gives up on getting
/// a non-empty reply.
const MAX_EMPTY_RESAMPLES: u32 = 2;

/// The bookkeeping one turn carries across its rounds.
///
/// All of it is per-turn by design: a corrective that fired in an earlier turn
/// must not suppress itself now, and an older incomplete plan must not hijack
/// an unrelated follow-up question.
#[derive(Default)]
struct TurnState {
    /// A one-shot corrective appended to the *next* request only. Never
    /// persisted, so it stays out of the stored transcript and the chat.
    nudge: Option<ChatMessage>,
    /// Whether the "announced an action but didn't call a tool" corrective has
    /// already fired this turn.
    intent_nudged: bool,
    /// Whether the "your plan still has open items" corrective has fired.
    plan_nudged: bool,
    /// Whether a plan updated *this turn* still has unfinished items.
    plan_open: bool,
    /// Consecutive rounds that produced neither prose nor a tool call.
    empty_rounds: u32,
    /// Identical (tool, arguments, result) repeats — one nudge, then stop.
    loop_guard: crate::loopguard::LoopGuard,
    /// The soft spend warning is logged once per turn, not per round.
    budget_warned: bool,
}

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

        let mut turn = TurnState::default();

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

            // Stop gracefully at the session's spend ceiling rather than
            // silently running past it.
            if let Some(message) = self.session_budget_stop(prompt_tokens, &mut turn) {
                self.push(ChatMessage::assistant(message.clone()))?;
                return Ok(message);
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
            self.report_compression(&report, &mut on_event);

            let (prefix_diff, tools_changed) = self.classify_prefix(&outbound, tools_hash);
            let outbound_len = outbound.len();

            let (assembled, mut outcome, rule_hits) = self
                .stream_reply(
                    outbound,
                    &tool_defs,
                    turn.nudge.as_ref(),
                    &cancel,
                    &mut on_event,
                )
                .await?;
            self.rule_history.next_round();

            if cancel.is_cancelled() {
                return self.finish_cancelled(
                    &assembled,
                    raw_prompt_tokens,
                    prompt_tokens,
                    outcome,
                );
            }

            // A stream rule matched. An interrupting one abandons what was
            // written — the point is to correct *before* the output lands — so
            // the partial reply is dropped and the round re-runs carrying the
            // reminder. The spend is still counted: the provider generated
            // those tokens whether or not we keep them.
            if self.apply_rule_hits(rule_hits, &mut turn) {
                self.account_for_usage(&assembled, raw_prompt_tokens, prompt_tokens, outcome);
                continue;
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
            if self.resample_empty_round(&assembled, &mut turn) {
                continue;
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
                // Otherwise: a corrective, or the final answer.
                match self.nudge_before_ending(&assembled.content, &mut turn) {
                    true => continue,
                    false => return Ok(assembled.content),
                }
            }

            // A tool call landed; the corrective (if any) served its purpose.
            turn.nudge = None;
            let loop_stop = self
                .run_tool_calls(&assembled, &mut turn, &mut on_event)
                .await?;

            // An unproductive loop hit the stop line: every round was
            // re-billing the whole context for an identical result, and the
            // nudge didn't break the cycle. End with the state preserved
            // rather than letting it spin.
            if let Some((name, repeats)) = loop_stop {
                let message = self.loop_stop_message(&name, repeats);
                self.push(ChatMessage::assistant(message.clone()))?;
                return Ok(message);
            }
        }
    }

    /// Fold matched stream rules into the turn: arm their reminder for the
    /// next request, and report whether the reply just streamed must be
    /// discarded and re-run.
    ///
    /// Rules the history has already spent are dropped here rather than at
    /// match time, so the watcher stays free of session state.
    fn apply_rule_hits(&mut self, hits: Vec<crate::rules::RuleHit>, turn: &mut TurnState) -> bool {
        if hits.is_empty() {
            return false;
        }
        let admitted = self.rule_history.admit(hits, &self.rules);
        if admitted.is_empty() {
            return false;
        }
        let reminder = admitted
            .iter()
            .map(|hit| hit.reminder())
            .collect::<Vec<_>>()
            .join("\n");
        turn.nudge = Some(ChatMessage::user(reminder));
        admitted.iter().any(|hit| hit.interrupt)
    }

    /// Emit the per-request compression event when a pass actually saved
    /// something, folding the saving into the session total.
    fn report_compression<F>(
        &mut self,
        report: &super::compression::CompressionReport,
        on_event: &mut F,
    ) where
        F: FnMut(&AgentEvent),
    {
        if report.saved_chars == 0 {
            return;
        }
        let saved_tokens = self.calibrated(budget::estimate_tokens_for_chars(report.saved_chars));
        self.tokens_saved += saved_tokens;
        on_event(&AgentEvent::Compression {
            mode: self.config.compression.as_str().to_string(),
            saved_tokens,
            total_saved_tokens: self.tokens_saved,
            results_compressed: report.results_compressed,
        });
    }

    /// End a turn the user stopped mid-stream.
    ///
    /// Persists only the partial prose — a half-formed tool call would be
    /// malformed and must never be replayed — and still counts the spend: the
    /// provider processed the prompt and generated this much whether or not a
    /// final usage chunk arrived. Returns the partial text so the UI settles
    /// to an ordinary reply rather than an error.
    fn finish_cancelled(
        &mut self,
        assembled: &AssembledMessage,
        raw_prompt_tokens: usize,
        prompt_tokens: usize,
        outcome: super::CallOutcome,
    ) -> Result<String, AgentError> {
        if !assembled.content.is_empty() {
            self.push(ChatMessage::assistant(assembled.content.clone()))?;
        }
        if assembled.usage.is_some()
            || !assembled.content.is_empty()
            || !assembled.tool_calls.is_empty()
        {
            self.account_for_usage(assembled, raw_prompt_tokens, prompt_tokens, outcome);
        }
        Ok(assembled.content.clone())
    }

    /// The reply that ends a turn spinning on an identical tool call, and the
    /// developer-log entry that goes with it.
    fn loop_stop_message(&self, name: &str, repeats: u32) -> String {
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
        format!(
            "Stopping this turn: the `{name}` tool was called with identical \
             arguments and returned an identical result {repeats} times in a row, \
             so continuing would spend tokens without making progress. Tell me how \
             you'd like to proceed, or rephrase the request."
        )
    }

    /// Whether the session's spend ceiling stops the turn here, and the
    /// message to end on. Also logs the soft warning, once per turn.
    fn session_budget_stop(&self, prompt_tokens: usize, turn: &mut TurnState) -> Option<String> {
        let budget = self.config.budget?;
        if self.tokens_used + prompt_tokens > budget.max_session_tokens {
            let message = format!(
                "Session token budget reached (~{} used of {} allowed; the next \
                 call needs ~{prompt_tokens} more). Stopping here so spending \
                 stays inside the cap. All work so far is saved — raise the \
                 budget or start a new session to continue.",
                self.tokens_used, budget.max_session_tokens
            );
            crate::errlog::record(
                self.config.error_log.as_deref(),
                "budget_exhausted",
                serde_json::json!({
                    "session": self.session_id(),
                    "model": self.config.model,
                    "tokens_used": self.tokens_used,
                    "max_session_tokens": budget.max_session_tokens,
                }),
            );
            return Some(message);
        }
        if !turn.budget_warned && self.tokens_used >= budget.warn_threshold() {
            turn.budget_warned = true;
            crate::errlog::record(
                self.config.error_log.as_deref(),
                "budget_warning",
                serde_json::json!({
                    "session": self.session_id(),
                    "tokens_used": self.tokens_used,
                    "max_session_tokens": budget.max_session_tokens,
                }),
            );
        }
        None
    }

    /// Fingerprint what's about to be sent and classify it against the previous
    /// request, so a cache miss is attributable (append-only requests are the
    /// shape a provider prefix cache rewards). Skipped entirely when no request
    /// log is configured — hashing an image-bearing transcript is megabytes of
    /// JSON per round, for a diagnostic nobody asked for.
    fn classify_prefix(
        &mut self,
        outbound: &[ChatMessage],
        tools_hash: Option<u64>,
    ) -> (cache::PrefixDiff, bool) {
        let Some(tools_hash) = tools_hash else {
            return (cache::PrefixDiff::First, false);
        };
        let request_hashes = cache::fingerprints(outbound);
        let prefix_diff = cache::diff_prefix(&self.prev_request_hashes, &request_hashes);
        let tools_changed = self.prev_tools_hash.is_some_and(|prev| prev != tools_hash);
        self.prev_request_hashes = request_hashes;
        self.prev_tools_hash = Some(tools_hash);
        (prefix_diff, tools_changed)
    }

    /// Whether to re-sample a round that produced neither prose nor a tool
    /// call — a provider anomaly, since the stream completed but carried
    /// nothing. Bounded, so a provider stuck returning nothing can't spin.
    fn resample_empty_round(&self, assembled: &AssembledMessage, turn: &mut TurnState) -> bool {
        if !assembled.content.is_empty() || !assembled.tool_calls.is_empty() {
            turn.empty_rounds = 0;
            return false;
        }
        if turn.empty_rounds >= MAX_EMPTY_RESAMPLES {
            return false;
        }
        turn.empty_rounds += 1;
        crate::errlog::record(
            self.config.error_log.as_deref(),
            "empty_reply_resampled",
            serde_json::json!({
                "session": self.session_id(),
                "model": self.config.model,
                "attempt": turn.empty_rounds,
                "max_attempts": MAX_EMPTY_RESAMPLES,
            }),
        );
        true
    }

    /// The model answered in prose with no tool call. Arm a one-shot corrective
    /// if this looks like a turn ending prematurely, returning whether to go
    /// around again; `false` means the reply is the final answer.
    ///
    /// Each corrective fires at most once per turn and is never persisted, so
    /// it stays out of both the stored transcript and the visible chat.
    fn nudge_before_ending(&self, reply: &str, turn: &mut TurnState) -> bool {
        // "I'll go and do X" with no call to actually do it.
        if !turn.intent_nudged && prompt::looks_like_unfulfilled_intent(reply) {
            turn.intent_nudged = true;
            turn.nudge = Some(ChatMessage::user(prompt::INTENT_NUDGE.to_string()));
            return true;
        }
        // Ending while this turn's own plan has unfinished items is almost
        // always a stall (a failed step made the model give up); one chance to
        // continue or tidy the checklist.
        if !turn.plan_nudged && turn.plan_open {
            turn.plan_nudged = true;
            turn.nudge = Some(ChatMessage::user(prompt::PLAN_STALL_NUDGE.to_string()));
            return true;
        }
        false
    }

    /// Run every tool call in a reply, recording results (and any images they
    /// produced) into the transcript. Returns the loop-guard's stop verdict
    /// when identical calls have repeated past the line.
    async fn run_tool_calls<F>(
        &mut self,
        assembled: &AssembledMessage,
        turn: &mut TurnState,
        on_event: &mut F,
    ) -> Result<Option<(String, u32)>, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        // A reply cut off at the response token limit leaves its trailing tool
        // call's JSON unfinished (e.g. a large `write_file`).
        let reply_truncated = matches!(
            assembled.finish_reason.as_deref(),
            Some("length" | "max_tokens")
        );
        let mut loop_stop = None;

        for call in &assembled.tool_calls {
            let result = self.run_tool(call, reply_truncated, on_event).await;
            match turn
                .loop_guard
                .observe(&call.function.name, &call.function.arguments, &result)
            {
                crate::loopguard::LoopVerdict::Fine => {}
                crate::loopguard::LoopVerdict::Nudge => {
                    turn.nudge = Some(ChatMessage::user(prompt::LOOP_NUDGE.to_string()));
                }
                crate::loopguard::LoopVerdict::Stop { name, repeats } => {
                    loop_stop = Some((name, repeats));
                }
            }
            // Track the latest plan state from successful `update_plan` calls
            // (invalid arguments were rejected, so they changed nothing).
            if call.function.name == harness_tools::PLAN_TOOL {
                if let Some(items) = harness_tools::parse_plan_arguments(&call.function.arguments) {
                    turn.plan_open = harness_tools::plan_is_open(&items);
                }
            }
            // A tool that produced an image (e.g. the preview screenshot) marks
            // it in-band; the `tool` role is text-only, so the image rides in
            // as a user message right after the result.
            match harness_core::attach::extract_image_markers(&result, "(image attached below)") {
                Some((cleaned, paths)) => {
                    self.push(ChatMessage::tool_result(call.id.clone(), cleaned))?;
                    self.push_tool_images(&paths)?;
                }
                None => self.push(ChatMessage::tool_result(call.id.clone(), result))?,
            }
        }
        Ok(loop_stop)
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

    /// A rule that fires on `.unwrap()` anywhere in the reply.
    fn no_unwrap_rule(interrupt: bool) -> crate::rules::RuleSet {
        crate::rules::RuleSet::new(vec![crate::rules::Rule {
            name: "no-unwrap".into(),
            pattern: regex::Regex::new(r"\.unwrap\(\)").unwrap(),
            scopes: vec![crate::rules::Scope::Text],
            message: "This project forbids `.unwrap()` — return a Result.".into(),
            interrupt,
            repeat: crate::rules::Repeat::Once,
        }])
    }

    #[tokio::test]
    async fn an_interrupting_rule_discards_the_reply_and_corrects_the_model() {
        let mut server = mockito::Server::new_async().await;
        // First call writes the forbidden thing; the retry (carrying the
        // reminder) answers properly.
        let offending = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("here you go: let x = thing.unwrap();"))
            .expect(1)
            .create_async()
            .await;
        let corrected = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("system-reminder".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("let x = thing?;"))
            .expect(1)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let mut agent = Agent::new(
            client,
            ToolRegistry::new(),
            store,
            session,
            AgentConfig {
                system_prompt: None,
                ..AgentConfig::default()
            },
        )
        .unwrap();
        agent.set_rules(no_unwrap_rule(true));

        let out = agent.run_turn("write it", |_| {}).await.unwrap();

        assert_eq!(out, "let x = thing?;");
        // The offending reply never reached the transcript — that is the whole
        // point of interrupting rather than reminding afterwards.
        let text: String = agent
            .messages()
            .iter()
            .filter_map(|m| m.content_text())
            .collect();
        assert!(!text.contains("unwrap()"), "the bad reply was kept: {text}");
        offending.assert_async().await;
        corrected.assert_async().await;
    }

    #[tokio::test]
    async fn a_rule_that_never_matches_costs_nothing() {
        let mut server = mockito::Server::new_async().await;
        let once = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("all good here"))
            .expect(1)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let mut agent = Agent::new(
            client,
            ToolRegistry::new(),
            store,
            session,
            AgentConfig {
                system_prompt: None,
                ..AgentConfig::default()
            },
        )
        .unwrap();
        agent.set_rules(no_unwrap_rule(true));

        assert_eq!(agent.run_turn("hi", |_| {}).await.unwrap(), "all good here");
        once.assert_async().await;
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
