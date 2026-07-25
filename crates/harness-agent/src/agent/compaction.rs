//! Keeping the transcript inside the context window.
//!
//! Two stages, and one speculation. [`Agent::fit_context`] is the gate every
//! model call passes through: if the transcript still fits, it returns; if it
//! doesn't, [`Agent::compact_to_fit`] prunes stale tool output and then
//! summarizes the oldest turns. Because that summary is itself a model call
//! made at the worst possible moment — mid-turn, with the user waiting — one
//! is started speculatively as the budget nears ([`Agent::maybe_prefire`]) and
//! spliced in when the line is actually crossed ([`Agent::consume_prefire`]).
//!
//! Only the in-memory transcript is touched; the history store keeps the full
//! record, so nothing the user said is ever lost to compaction.

use harness_llm::stream::AssembledMessage;
use harness_llm::types::ChatMessage;
use harness_llm::{ChatRequest, OxenClient};
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::{budget, compact};

use super::{Agent, CallOutcome, PrefireSummary};

/// Compaction keeps the latest few tool outputs verbatim.
const KEEP_RECENT_TOOLS: usize = 2;
/// …and the last few whole turns.
const KEEP_RECENT_TURNS: usize = 3;

/// At this percentage of the prompt budget, a compaction summary starts
/// cooking speculatively in the background (see [`Agent::maybe_prefire`]) so
/// it's warm when the hard 100% line forces a splice.
const PREFIRE_THRESHOLD_PERCENT: usize = 85;

impl Agent {
    /// Keep the next request within the context window, returning the raw
    /// (uncalibrated) prompt-token estimate for the transcript that will be sent.
    ///
    /// The estimate is calibrated by the latest real usage before the check, so
    /// it tracks reality (the raw code under-counts at ~4 chars/token). On
    /// overflow it compacts — pruning stale tool output, then summarizing old
    /// turns — rather than hard-stopping, and only errors if even a compacted
    /// transcript still can't fit.
    pub(super) async fn fit_context<F>(
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
        let mut raw = budget::estimate_prompt_tokens(&self.messages, tool_defs);
        if self.calibrated(raw) > resident_budget && resident_budget < budget {
            // Best effort: a single recent turn may legitimately exceed the soft
            // resident target. The real provider window remains authoritative.
            let _ = self
                .compact_to_fit(resident_budget, tool_defs, on_event)
                .await?;
            // Only a compaction changes the transcript; re-estimate just then.
            raw = budget::estimate_prompt_tokens(&self.messages, tool_defs);
        }
        if self.calibrated(raw) <= budget {
            // Nearing the line: start preparing the summary in the background
            // now, so when compaction actually triggers the summary is
            // already warm and the turn doesn't stall on a model call it
            // could have made minutes earlier.
            if self.calibrated(raw).saturating_mul(100)
                >= budget.saturating_mul(PREFIRE_THRESHOLD_PERCENT)
            {
                self.maybe_prefire();
            }
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

    /// Free context so the next request fits `budget`, in two stages (see
    /// [`compact`]): prune stale tool output, then summarize the oldest turns.
    /// Emits an [`AgentEvent::Compacted`] for each stage that does work and
    /// returns whether the transcript now fits. Mutates only the in-memory
    /// transcript — the history store keeps the full record.
    pub(super) async fn compact_to_fit<F>(
        &mut self,
        budget: usize,
        tool_defs: &[serde_json::Value],
        on_event: &mut F,
    ) -> Result<bool, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
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

        // Stage 2a: a summary prepared speculatively in the background (see
        // [`Agent::maybe_prefire`]) is spliced in first — usually making the
        // synchronous model call below unnecessary, so compaction costs no
        // wall-clock the user can feel.
        if self.consume_prefire(on_event).await && self.fits_budget(budget, tool_defs) {
            return Ok(true);
        }

        // Stage 2: summarize the oldest turns into a single message. The cut is
        // on a user-turn boundary, so no tool result is orphaned from its call.
        // When stage 2a spliced but didn't free enough, this pass re-summarizes
        // the remainder — the fresh summary message participates via the
        // prompt's carry-forward instruction, so nothing it held is lost.
        let Some(cut) = compact::summary_cut_index(&self.messages, KEEP_RECENT_TURNS) else {
            return Ok(self.fits_budget(budget, tool_defs));
        };
        let start = self.transcript_start();
        let rendered = compact::render_for_summary(&self.messages[start..cut]);
        // Summarization runs on the (possibly cheaper) summary model: it
        // re-reads the whole elided span, which at frontier rates would make
        // every compaction cost a frontier-sized prompt.
        let prompt_estimate =
            budget::estimate_tokens_for_chars(compact::SUMMARY_PROMPT.len() + rendered.len());
        let started = std::time::Instant::now();
        let assembled = summarize(&self.client, self.summary_model(), rendered).await?;
        let (prompt, completion) = budget::split_oneshot_usage(&assembled, prompt_estimate);
        self.record_usage_event(
            self.summary_model(),
            prompt,
            completion,
            "summary",
            &CallOutcome {
                latency_ms: Some(started.elapsed().as_millis() as u64),
                ..CallOutcome::default()
            },
        );
        self.apply_summary(
            cut,
            &assembled.content,
            "summarized earlier conversation",
            on_event,
        );
        Ok(self.fits_budget(budget, tool_defs))
    }

    /// Record a summarization call's spend against the summary model under the
    /// `"summary"` kind — the one policy for every prefire accounting path.
    pub(super) fn record_summary_usage(&self, prompt: usize, completion: usize) {
        self.record_usage_event(
            self.summary_model(),
            prompt,
            completion,
            "summary",
            &CallOutcome::default(),
        );
    }

    /// Where the summarizable transcript begins: past the leading system
    /// prompt when there is one.
    pub(super) fn transcript_start(&self) -> usize {
        usize::from(self.messages.first().is_some_and(|m| m.role == "system"))
    }

    /// Replace `[transcript_start()..cut)` with one summary message, then do
    /// everything a splice obligates: drop any in-flight prefire (its prefix
    /// is now stale), persist the compact snapshot, and tell the host. The
    /// one place splice bookkeeping lives, so the prefire and synchronous
    /// paths can't drift on it.
    pub(super) fn apply_summary<F>(
        &mut self,
        cut: usize,
        summary: &str,
        detail: &str,
        on_event: &mut F,
    ) where
        F: FnMut(&AgentEvent),
    {
        let start = self.transcript_start();
        let note = ChatMessage::user(format!("{}\n{}", compact::SUMMARY_MARKER, summary));
        self.messages.splice(start..cut, std::iter::once(note));
        self.invalidate_prefire();
        self.save_context_snapshot();
        on_event(&AgentEvent::Compacted {
            detail: detail.to_string(),
        });
    }

    /// Kick off a background summarization of the oldest turns if none is in
    /// flight. Called when the transcript nears (but hasn't hit) the budget;
    /// the result is consumed by [`Agent::consume_prefire`] when compaction
    /// actually triggers. Costs one speculative model call; if compaction
    /// never triggers the result is simply dropped.
    pub(super) fn maybe_prefire(&mut self) {
        if self.prefire.is_some() {
            return;
        }
        let Some(cut) = compact::summary_cut_index(&self.messages, KEEP_RECENT_TURNS) else {
            return;
        };
        let start = self.transcript_start();
        if cut <= start {
            return;
        }
        let rendered = compact::render_for_summary(&self.messages[start..cut]);
        // The call's approximate input cost, kept so its spend is accounted
        // even when the provider reports no usage (or the summary is later
        // discarded) — the request hits the provider either way.
        let prompt_estimate =
            budget::estimate_tokens_for_chars(compact::SUMMARY_PROMPT.len() + rendered.len());
        let client = self.client.clone();
        let model = self.summary_model().to_string();
        let handle = tokio::spawn(async move { summarize(&client, &model, rendered).await });
        self.prefire = Some(PrefireSummary {
            cut,
            prompt_estimate,
            handle,
        });
    }

    /// Splice in the prefire summary if one is ready and still valid,
    /// returning whether the transcript changed. Validity holds because the
    /// transcript is append-only between splices, and every splice calls
    /// [`Agent::invalidate_prefire`] (see [`Agent::apply_summary`]) — a
    /// prefire that survives to this point covers an intact prefix. A
    /// failed or empty summary is discarded (its estimated spend still
    /// accounted) and the caller falls back to the synchronous path.
    pub(super) async fn consume_prefire<F>(&mut self, on_event: &mut F) -> bool
    where
        F: FnMut(&AgentEvent),
    {
        let Some(pre) = self.prefire.take() else {
            return false;
        };
        let start = self.transcript_start();
        if pre.cut <= start || pre.cut > self.messages.len() {
            // Defensive: shouldn't happen while every splice invalidates.
            self.record_summary_usage(pre.prompt_estimate, 0);
            pre.handle.abort();
            return false;
        }
        let prompt_estimate = pre.prompt_estimate;
        // Usually already finished; if the hard threshold arrived first, this
        // waits out the remainder — no worse than the synchronous call.
        let Ok(Ok(assembled)) = pre.handle.await else {
            self.record_summary_usage(prompt_estimate, 0);
            return false;
        };
        // Account the speculative call's spend (provider-reported when
        // available, estimated otherwise) — same policy as [`Agent::complete`].
        let (prompt, completion) = budget::split_oneshot_usage(&assembled, prompt_estimate);
        self.record_summary_usage(prompt, completion);
        if assembled.content.is_empty() {
            return false;
        }
        self.apply_summary(
            pre.cut,
            &assembled.content,
            "summarized earlier conversation (prepared in background)",
            on_event,
        );
        true
    }
}

/// One-shot summarization call used by the background prefire task — free of
/// `&self` so it can run on a spawned task while the agent keeps working.
/// Usage is accounted by the consumer (see [`Agent::consume_prefire`]).
async fn summarize(
    client: &OxenClient,
    model: &str,
    rendered: String,
) -> Result<AssembledMessage, harness_llm::LlmError> {
    let messages = vec![
        ChatMessage::system(compact::SUMMARY_PROMPT.to_string()),
        ChatMessage::user(rendered),
    ];
    let request = ChatRequest::new(model, messages).streaming(true);
    client
        .stream_chat(&request, &CancellationToken::new(), |_| {})
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use harness_llm::OxenClient;
    use harness_store::HistoryStore;
    use harness_tools::ToolRegistry;

    use crate::test_support::{sse_prose, test_session};
    use crate::{Agent, AgentConfig, AgentError, AgentEvent};

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
    async fn a_prefired_summary_is_consumed_when_compaction_triggers() {
        let mut server = mockito::Server::new_async().await;
        // Defined first so the background summarization request (the only one
        // whose body carries the structured summary prompt) matches it and
        // ordinary turn requests fall through to the mocks below. expect(1)
        // also proves the synchronous summarize path never ran.
        let summary = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("Structure the summary".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("PREFIRED SUMMARY"))
            .expect(1)
            .create_async()
            .await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("ok"))
            .expect(1)
            .create_async()
            .await;
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("done"))
            .expect(1)
            .create_async()
            .await;

        // Seed three turns of big assistant prose — nothing for the
        // tool-prune stage to reclaim, so only summarization can free room.
        // ~24k chars ≈ 6k tokens: above 85% of the 7000 budget, below 100%.
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "qwen3-8b");
        let big = "x".repeat(8000);
        for i in 0..3 {
            store
                .append_message(&session, &ChatMessage::user(format!("q{i}")))
                .unwrap();
            store
                .append_message(&session, &ChatMessage::assistant(big.clone()))
                .unwrap();
        }
        let client = OxenClient::new(server.url(), "key", "qwen3-8b");
        let config = AgentConfig {
            model: "qwen3-8b".into(),
            system_prompt: None,
            context_window: Some(7000),
            response_reserve: 0,
            ..AgentConfig::default()
        };
        let mut agent =
            Agent::resume_from_store(client, ToolRegistry::new(), store, session, config).unwrap();

        // Turn 1 fits but crosses the 85% prefire line: the background
        // summarization is spawned; the turn itself completes normally.
        let out = agent.run_turn("continue", |_| {}).await.unwrap();
        assert_eq!(out, "ok");
        assert!(agent.prefire.is_some(), "the prefire should be in flight");

        // Turn 2 pushes the transcript over budget: compaction consumes the
        // warm summary instead of making a synchronous model call.
        let mut details = Vec::new();
        let out = agent
            .run_turn("y".repeat(8000), |e| {
                if let AgentEvent::Compacted { detail } = e {
                    details.push(detail.clone());
                }
            })
            .await
            .unwrap();
        assert_eq!(out, "done");
        assert!(
            details.iter().any(|d| d.contains("prepared in background")),
            "compaction should have used the prefired summary: {details:?}"
        );
        let note = agent
            .messages()
            .iter()
            .find(|m| {
                m.content_text()
                    .is_some_and(|t| t.contains("PREFIRED SUMMARY"))
            })
            .expect("the prefired summary should be spliced into the transcript");
        assert_eq!(note.role, "user");
        summary.assert_async().await;
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
