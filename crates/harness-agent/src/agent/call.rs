//! One model call: send the prepared request, stream the reply, and account
//! for what it cost.
//!
//! Everything here is per-call rather than per-turn — the turn loop
//! ([`super::turn`]) decides *what* to send and what to do with the answer;
//! this decides how a single request is made, retried, paid for, and logged.
//! Retry policy lives here too: transient provider failures back off and
//! retry, and a model that stays down hands the call to the next configured
//! fallback rather than ending the turn.

use harness_llm::stream::{AssembledMessage, StreamEvent};
use harness_llm::types::ChatMessage;
use harness_llm::ChatRequest;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::{budget, cache};

use super::{Agent, CallOutcome};

fn emit_stream_event<F>(buffered: &mut Option<Vec<AgentEvent>>, event: AgentEvent, on_event: &mut F)
where
    F: FnMut(&AgentEvent),
{
    match buffered {
        Some(events) => events.push(event),
        None => on_event(&event),
    }
}

impl Agent {
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
    pub(super) async fn stream_reply<F>(
        &self,
        mut outbound: Vec<ChatMessage>,
        tool_defs: &[serde_json::Value],
        nudge: Option<&ChatMessage>,
        cancel: &CancellationToken,
        on_event: &mut F,
    ) -> Result<(AssembledMessage, CallOutcome, Vec<crate::rules::RuleHit>), AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        outbound.extend(nudge.cloned());
        // Prompt-cache breakpoints on the growing tip (see [`crate::cache`]).
        // Empty (a plain request) when the mode/model opts out.
        let anchors = self
            .config
            .prompt_cache
            .anchors_for(&self.config.model, &outbound);
        let mut request = ChatRequest::new(&self.config.model, outbound)
            .with_tools(tool_defs.to_vec())
            .max_tokens(self.config.effective_response_reserve())
            .streaming(true)
            .with_cache_anchors(anchors);

        let retry = self.config.retry.clone();
        let started = std::time::Instant::now();
        let mut attempt: u32 = 1;
        // How far down `retry.fallback_models` this call has walked.
        let mut fallbacks = retry.fallback_models.iter();
        loop {
            // A rule that matches abandons the response through this token, a
            // child of the turn's — so a rule interrupt and a user stop both
            // end the stream, and the turn's own cancellation still reaches it.
            let rule_stop = cancel.child_token();
            let mut watcher = self.rule_history.watcher(&self.rules);
            // Once an interrupt-capable rule is eligible, live presentation
            // waits until the call is accepted. If the rule fires, the rejected
            // prose/tool preview is simply never emitted; every host therefore
            // sees only the corrected round without needing a rewind protocol.
            let mut buffered_events = watcher.can_interrupt().then(Vec::new);
            let result = self
                .client
                .stream_chat(&request, &rule_stop, |event| match event {
                    StreamEvent::Token(t) => {
                        if watcher.observe(crate::rules::Scope::Text, t) {
                            rule_stop.cancel();
                        }
                        let event = AgentEvent::Token(t.clone());
                        emit_stream_event(&mut buffered_events, event, on_event);
                    }
                    StreamEvent::ToolCallStart { name } => {
                        let event = AgentEvent::ToolPending { name: name.clone() };
                        emit_stream_event(&mut buffered_events, event, on_event);
                    }
                    StreamEvent::ToolCallDelta { name, arguments } => {
                        if watcher.observe(crate::rules::Scope::ToolArguments, arguments) {
                            rule_stop.cancel();
                        }
                        let event = AgentEvent::ToolDelta {
                            name: name.clone(),
                            delta: arguments.clone(),
                        };
                        emit_stream_event(&mut buffered_events, event, on_event);
                    }
                    StreamEvent::Done { .. } => {}
                })
                .await;
            let hits = watcher.hits();
            let interrupted = hits.iter().any(|hit| hit.interrupt);
            if let Some(events) = buffered_events.filter(|_| !interrupted) {
                for event in events {
                    on_event(&event);
                }
            }

            match result {
                Ok(assembled) => {
                    let outcome = CallOutcome {
                        model: Some(request.model.clone()),
                        latency_ms: Some(started.elapsed().as_millis() as u64),
                        retries: attempt - 1,
                        ..CallOutcome::default()
                    };
                    return Ok((assembled, outcome, hits));
                }
                // Retries on this model are spent but another model is
                // configured: a provider having a bad minute shouldn't end the
                // turn. Switch and start its attempt budget fresh, with no
                // backoff — a different endpoint is a fresh chance, not a
                // repeat of the one that just failed.
                Err(e)
                    if e.is_transient()
                        && attempt >= retry.max_attempts
                        && fallbacks.clone().next().is_some() =>
                {
                    let next = fallbacks.next().expect("checked above").clone();
                    crate::errlog::record(
                        self.config.error_log.as_deref(),
                        "model_fallback",
                        serde_json::json!({
                            "session": self.session_id(),
                            "from": request.model.as_str(),
                            "to": next,
                            "attempts": attempt,
                            "error": e.to_string(),
                        }),
                    );
                    on_event(&AgentEvent::Retrying {
                        attempt,
                        max_attempts: retry.max_attempts,
                        delay_ms: 0,
                        error: e.to_string(),
                        switching_to: Some(next.clone()),
                    });
                    // Cache breakpoints are per model family, so re-derive
                    // them for the model actually about to be called.
                    request.cache_anchors = self
                        .config
                        .prompt_cache
                        .anchors_for(&next, &request.messages);
                    request.model = next;
                    attempt = 1;
                }
                Err(e) if e.is_transient() && attempt < retry.max_attempts => {
                    let delay = retry.delay_after(attempt);
                    crate::errlog::record(
                        self.config.error_log.as_deref(),
                        "retrying",
                        serde_json::json!({
                            "session": self.session_id(),
                            "model": request.model.as_str(),
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
                        switching_to: None,
                    });
                    // A stop during the backoff wait ends the turn like any
                    // other cancellation: quietly, with nothing assembled.
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            return Ok((
                                AssembledMessage::default(),
                                CallOutcome::default(),
                                Vec::new(),
                            ))
                        }
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
                        model: request.model.clone(),
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
    /// calibrated estimate when it doesn't report any. Returns the outcome
    /// enriched with the call's cache-read/write split for the request log.
    pub(super) fn account_for_usage(
        &mut self,
        assembled: &AssembledMessage,
        raw_prompt_tokens: usize,
        prompt_tokens: usize,
        mut outcome: CallOutcome,
    ) -> CallOutcome {
        if let Some(usage) = &assembled.usage {
            // Calibrate against the full prompt the provider processed (see
            // [`budget::reported_full_prompt`] for the two counting styles).
            // Some endpoints additionally *hide* cached tokens entirely
            // (hub.oxen.ai returns prompt_tokens: 3 for a fully-cached
            // 4K-token prompt, with the cache fields stripped) — calibrating
            // on such a report would collapse the estimate and disable
            // compaction, so a report far below the estimate is ignored.
            let reported_full = budget::reported_full_prompt(usage);
            if reported_full > 0 && raw_prompt_tokens > 0 {
                let ratio = reported_full as f64 / raw_prompt_tokens as f64;
                if ratio >= 0.5 {
                    self.token_ratio = ratio;
                }
            }
            outcome.cached_prompt_tokens = usage.cached_prompt_tokens() as usize;
            outcome.cache_write_tokens = usage.cache_write_tokens() as usize;
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
        self.cached_prompt_tokens_used += outcome.cached_prompt_tokens;
        self.cache_write_tokens_used += outcome.cache_write_tokens;
        let answered_by = outcome.model_or(&self.config.model);
        self.record_usage_event(
            answered_by,
            prompt_delta,
            completion_delta,
            "turn",
            &outcome,
        );
        outcome
    }

    /// Append one entry to the developer request log (when configured): the
    /// call's size, how its prefix relates to the previous request (the cache
    /// diagnostic), and what the provider actually reported. Best-effort.
    pub(super) fn log_request(
        &self,
        est_prompt_tokens: usize,
        message_count: usize,
        assembled: &AssembledMessage,
        outcome: &CallOutcome,
        prefix_diff: cache::PrefixDiff,
        tools_changed: bool,
    ) {
        let Some(path) = self.config.request_log.as_deref() else {
            return;
        };
        let (prefix, detail) = match prefix_diff {
            cache::PrefixDiff::First => ("first", serde_json::Value::Null),
            cache::PrefixDiff::AppendOnly { shared } => (
                "append_only",
                serde_json::json!({ "shared_messages": shared }),
            ),
            cache::PrefixDiff::Diverged { at } => (
                "diverged",
                serde_json::json!({ "first_changed_message": at }),
            ),
        };
        let usage = assembled.usage.as_ref();
        let prompt = usage.map(|u| u.prompt_tokens).unwrap_or(0) as usize;
        let cache_hit_ratio = if prompt > 0 {
            outcome.cached_prompt_tokens as f64 / prompt as f64
        } else {
            0.0
        };
        crate::errlog::record(
            Some(path),
            "model_request",
            serde_json::json!({
                "session": self.session_id(),
                "model": outcome.model_or(&self.config.model),
                "kind": "turn",
                "messages": message_count,
                "est_prompt_tokens": est_prompt_tokens,
                "prefix": prefix,
                "prefix_detail": detail,
                "tools_changed": tools_changed,
                "latency_ms": outcome.latency_ms,
                "retries": outcome.retries,
                "usage": usage.map(|u| serde_json::json!({
                    "prompt_tokens": u.prompt_tokens,
                    "completion_tokens": u.completion_tokens,
                    "cached_prompt_tokens": u.cached_prompt_tokens(),
                    "cache_write_tokens": u.cache_write_tokens(),
                })),
                "cache_hit_ratio": (cache_hit_ratio * 1000.0).round() / 1000.0,
            }),
        );
    }
}

/// A stable machine-readable tag for an [`AgentError`] variant, so the error
/// log can be filtered (`jq 'select(.kind == "retries_exhausted")'`) without
/// parsing display strings.
pub(super) fn error_kind(e: &AgentError) -> &'static str {
    match e {
        AgentError::Llm(_) => "llm",
        AgentError::Tool(_) => "tool",
        AgentError::History(_) => "history",
        AgentError::Io(_) => "io",
        AgentError::Attachment(_) => "attachment",
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

    use crate::test_support::{
        fast_retry, retry_test_agent, sse_prose, sse_snap_call, test_session,
    };
    use crate::{Agent, AgentConfig, AgentError, AgentEvent, RetryPolicy};

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
    async fn a_model_that_stays_down_falls_back_instead_of_failing_the_turn() {
        let mut server = mockito::Server::new_async().await;
        // Two mocks keyed on the model in the request body: the session model
        // is having a bad day, the fallback is healthy.
        let down = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("claude-opus-4-8".into()))
            .with_status(503)
            .with_body(r#"{"error":{"title":"The model provider returned an error."}}"#)
            .expect(2)
            .create_async()
            .await;
        let healthy = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("backup-model".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("answered by the backup"))
            .expect(1)
            .create_async()
            .await;

        let retry = RetryPolicy {
            fallback_models: vec!["backup-model".to_string()],
            ..fast_retry(2)
        };
        let mut agent = retry_test_agent(server.url(), retry);
        let mut switches = Vec::new();
        let out = agent
            .run_turn("hello", |e| {
                if let AgentEvent::Retrying {
                    switching_to: Some(model),
                    ..
                } = e
                {
                    switches.push(model.clone());
                }
            })
            .await
            .expect("the fallback model should carry the turn");

        assert_eq!(out, "answered by the backup");
        assert_eq!(switches, vec!["backup-model".to_string()]);
        down.assert_async().await;
        healthy.assert_async().await;
    }

    #[tokio::test]
    async fn fallback_usage_and_request_logs_name_the_model_that_answered() {
        let mut server = mockito::Server::new_async().await;
        let down = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("claude-opus-4-8".into()))
            .with_status(503)
            .with_body(r#"{"error":{"title":"provider down"}}"#)
            .expect(1)
            .create_async()
            .await;
        let healthy = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("backup-model".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("backup answer"))
            .expect(1)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let request_log = dir.path().join("requests.jsonl");
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let mut agent = Agent::new(
            client,
            ToolRegistry::new(),
            store.clone(),
            session,
            AgentConfig {
                system_prompt: None,
                retry: RetryPolicy {
                    fallback_models: vec!["backup-model".into()],
                    ..fast_retry(1)
                },
                request_log: Some(request_log.clone()),
                ..AgentConfig::default()
            },
        )
        .unwrap();

        assert_eq!(
            agent.run_turn("hello", |_| {}).await.unwrap(),
            "backup answer"
        );
        let usage = store.model_usage_breakdown().unwrap();
        assert_eq!(usage.len(), 1, "{usage:?}");
        assert_eq!(usage[0].model, "backup-model");
        let logged: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(request_log).unwrap().trim()).unwrap();
        assert_eq!(logged["model"], "backup-model");
        down.assert_async().await;
        healthy.assert_async().await;
    }

    #[tokio::test]
    async fn the_fallback_chain_is_exhausted_before_the_turn_fails() {
        let mut server = mockito::Server::new_async().await;
        let all_down = server
            .mock("POST", "/chat/completions")
            .with_status(503)
            .with_body(r#"{"error":{"title":"The model provider returned an error."}}"#)
            .expect(4) // 2 attempts on the session model, then 2 on the backup
            .create_async()
            .await;

        let retry = RetryPolicy {
            fallback_models: vec!["backup-model".to_string()],
            ..fast_retry(2)
        };
        let mut agent = retry_test_agent(server.url(), retry);
        agent
            .run_turn("hello", |_| {})
            .await
            .expect_err("every model is down, so the turn must still fail");
        all_down.assert_async().await;
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

    /// SSE for a prose reply whose final chunk reports usage with cached and
    /// cache-write prompt tokens (the OpenAI + Anthropic shapes combined).
    fn sse_prose_with_cached_usage(text: &str) -> String {
        let content = serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": { "content": text },
                "finish_reason": "stop"
            }]
        });
        let usage = serde_json::json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 20,
                "total_tokens": 1020,
                "prompt_tokens_details": { "cached_tokens": 900 },
                "cache_creation_input_tokens": 80
            }
        });
        format!("data: {content}\n\ndata: {usage}\n\ndata: [DONE]\n\n")
    }

    #[tokio::test]
    async fn claude_requests_carry_cache_anchors_and_cached_usage_is_tallied() {
        let mut server = mockito::Server::new_async().await;
        // The mock only matches a request whose body carries a cache_control
        // marker — a request without anchors gets no response and errors.
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("cache_control".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose_with_cached_usage("done"))
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let mut agent = Agent::new(
            client,
            ToolRegistry::new(),
            store.clone(),
            session,
            AgentConfig::default(),
        )
        .unwrap();

        let out = agent.run_turn("hello", |_| {}).await.unwrap();
        assert_eq!(out, "done");
        // The provider's cache split is tallied on the agent…
        assert_eq!(agent.cached_prompt_tokens_used(), 900);
        assert_eq!(agent.cache_write_tokens_used(), 80);
        // …and persisted on the usage ledger.
        let totals = store.cache_usage_totals().unwrap();
        assert_eq!(totals.cached_prompt_tokens, 900);
        assert_eq!(totals.cache_write_tokens, 80);
    }

    #[tokio::test]
    async fn non_anthropic_models_send_plain_requests_in_auto_mode() {
        let mut server = mockito::Server::new_async().await;
        // Match only a request with no cache_control anywhere in the body.
        server
            .mock("POST", "/chat/completions")
            .match_request(|req| {
                !String::from_utf8_lossy(req.body().unwrap()).contains("cache_control")
            })
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("plain"))
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "qwen3-8b");
        let client = OxenClient::new(server.url(), "key", "qwen3-8b");
        let config = AgentConfig {
            model: "qwen3-8b".into(),
            system_prompt: None,
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        let out = agent.run_turn("hello", |_| {}).await.unwrap();
        assert_eq!(out, "plain");
    }

    #[tokio::test]
    async fn requested_max_tokens_is_clamped_to_the_model_output_ceiling() {
        let mut server = mockito::Server::new_async().await;
        // Only a request asking for the *clamped* reply size matches — the
        // model's reported 2000-token ceiling, not the 4096 default reserve.
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "max_tokens": 2000
            })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("capped"))
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            max_output_tokens: Some(2000),
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        let out = agent.run_turn("hello", |_| {}).await.unwrap();
        assert_eq!(out, "capped");
    }

    #[tokio::test]
    async fn the_request_log_classifies_prefixes_across_rounds() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_snap_call())
            .expect(1)
            .create_async()
            .await;
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("image attached below|snap".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("finished"))
            .create_async()
            .await;

        struct NoopSnap;
        #[derive(serde::Deserialize, schemars::JsonSchema)]
        struct NoopArgs {}
        #[async_trait::async_trait]
        impl harness_tools::TypedTool for NoopSnap {
            const NAME: &'static str = "snap";
            type Args = NoopArgs;
            fn description(&self) -> &str {
                "noop"
            }
            async fn run(&self, _: NoopArgs) -> Result<String, harness_tools::ToolError> {
                Ok("ok".into())
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests.jsonl");
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let mut tools = ToolRegistry::new();
        tools.register_typed(NoopSnap);
        let config = AgentConfig {
            system_prompt: None,
            request_log: Some(log.clone()),
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, tools, store, session, config).unwrap();

        let out = agent.run_turn("snap it", |_| {}).await.unwrap();
        assert_eq!(out, "finished");

        let body = std::fs::read_to_string(&log).unwrap();
        let entries: Vec<serde_json::Value> = body
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(entries.len(), 2, "one entry per model call: {body}");
        // First call has no predecessor; the tool round only appends to it —
        // the cache-friendly shape the diagnostics are there to confirm.
        assert_eq!(entries[0]["event"], "model_request");
        assert_eq!(entries[0]["prefix"], "first");
        assert_eq!(entries[1]["prefix"], "append_only");
        assert_eq!(entries[1]["tools_changed"], false);
        assert!(entries[1]["latency_ms"].as_u64().is_some());
    }
}
