//! Outbound context compression (see [`harness_compress`]).
//!
//! Compression only ever shapes the *request*: stale tool output in the
//! outbound copy of the transcript is replaced with a compact digest and a
//! `retrieve_original` marker, while the in-memory transcript and the history
//! store keep every original byte. `Audit` runs the identical pipeline to
//! measure would-be savings without changing what's sent.

use std::sync::Arc;

use harness_compress::{CcrStore, CompressionMode};
use harness_llm::types::{ChatMessage, MessageContent};
use harness_tools::ToolRegistry;

use crate::config::AgentConfig;

use super::Agent;

impl Agent {
    /// The compression mode this agent was built with (a UI showing "armed"
    /// state needs the agent's actual mode, not the current global preference —
    /// they differ for agents built before the preference changed).
    pub fn compression_mode(&self) -> CompressionMode {
        self.config.compression
    }

    /// Switch compression for subsequent model calls on this live conversation
    /// (e.g. from a meter toggle), registering or removing the
    /// `retrieve_original` tool to match. Turning `On` off is always safe: the
    /// transcript keeps every original, compression only ever shapes what's
    /// sent. Markers from a previous `On` period stop being resolvable (their
    /// store is dropped), which the retrieve tool reports gracefully.
    pub fn set_compression_mode(&mut self, mode: CompressionMode) {
        if mode == self.config.compression {
            return;
        }
        self.config.compression = mode;
        self.compression_cache.clear();
        match mode {
            CompressionMode::On => {
                self.ccr = setup_compression(&self.config, &mut self.tools);
            }
            CompressionMode::Audit | CompressionMode::Off => {
                self.tools.remove(harness_tools::RETRIEVE_ORIGINAL_TOOL);
                self.ccr = None;
            }
        }
    }

    /// Cumulative estimated tokens compression saved (`on`) or would have
    /// saved (`audit`) this run. Always 0 with compression off.
    pub fn tokens_saved(&self) -> usize {
        self.tokens_saved
    }

    /// Build the transcript to send, applying context compression per the
    /// configured mode (see [`harness_compress`]).
    ///
    /// Only stale `tool` messages are candidates — never the most recent few
    /// (the model is still working with them), never `retrieve_original`
    /// results (re-compressing them would loop), and the compressor itself
    /// protects errors, small output, and anything already compressed. In
    /// `Audit` mode the report is computed but the original messages are
    /// returned; in `Off` this is exactly [`Agent::outbound_messages`].
    ///
    /// [`Agent::outbound_messages`]: Agent#method.outbound_messages
    pub(super) fn prepare_outbound(&mut self) -> (Vec<ChatMessage>, CompressionReport) {
        let mut messages = self.outbound_messages();
        let mut report = CompressionReport::default();
        if self.config.compression == CompressionMode::Off {
            return (messages, report);
        }

        // Results of `retrieve_original` calls are exempt: they exist because
        // the model asked for the full data back.
        let retrieve_ids: std::collections::HashSet<String> = messages
            .iter()
            .flat_map(|m| m.tool_calls.iter().flatten())
            .filter(|c| c.function.name == harness_tools::RETRIEVE_ORIGINAL_TOOL)
            .map(|c| c.id.clone())
            .collect();

        let tool_indices: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == "tool")
            .map(|(i, _)| i)
            .collect();
        let protect_from = tool_indices
            .len()
            .saturating_sub(self.compress_cfg.keep_recent_tools);

        let apply = self.config.compression == CompressionMode::On;
        // Audit passes no store: the identical pipeline runs, nothing is kept.
        let store = if apply { self.ccr.as_deref() } else { None };

        for &i in &tool_indices[..protect_from] {
            if messages[i]
                .tool_call_id
                .as_ref()
                .is_some_and(|id| retrieve_ids.contains(id))
            {
                continue;
            }
            let Some(MessageContent::Text(text)) = &messages[i].content else {
                continue;
            };
            let key = harness_compress::ccr::hash_content(text);
            let cached = self.compression_cache.get(&key).cloned();
            let compressed = match cached {
                Some(value) => value,
                None => {
                    let value =
                        harness_compress::compress_tool_result(text, &self.compress_cfg, store)
                            .map(|compressed| compressed.text);
                    if self.compression_cache.len() >= 256 {
                        self.compression_cache.clear();
                    }
                    self.compression_cache.insert(key, value.clone());
                    value
                }
            };
            if let Some(compressed) = compressed {
                report.saved_chars += text.len().saturating_sub(compressed.len());
                report.results_compressed += 1;
                if apply {
                    messages[i].content = Some(MessageContent::Text(compressed));
                }
            }
        }
        (messages, report)
    }
}

/// What one [`Agent::prepare_outbound`] pass did (or, in audit, would do).
///
/// [`Agent::prepare_outbound`]: Agent#method.prepare_outbound
#[derive(Debug, Default)]
pub(super) struct CompressionReport {
    pub(super) saved_chars: usize,
    pub(super) results_compressed: usize,
}

/// Set up compression for a new agent: `On` gets a CCR store and the
/// `retrieve_original` tool registered; `Audit`/`Off` need neither (audit
/// sends unmodified requests, so there are no markers to resolve).
pub(super) fn setup_compression(
    config: &AgentConfig,
    tools: &mut ToolRegistry,
) -> Option<Arc<CcrStore>> {
    match config.compression {
        CompressionMode::On => {
            let store = Arc::new(match &config.attachment_root {
                Some(root) => CcrStore::disk_backed(root.join(".oxen-harness/ccr")),
                None => CcrStore::default(),
            });
            tools.register_typed(harness_tools::RetrieveOriginalTool::new(store.clone()));
            Some(store)
        }
        CompressionMode::Audit | CompressionMode::Off => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use harness_llm::OxenClient;
    use harness_store::{HistoryStore, SessionMeta};
    use harness_tools::ToolRegistry;

    use crate::test_support::{sse_prose, test_session};
    use crate::{Agent, AgentConfig, AgentEvent};

    use super::*;

    /// A big, repetitive JSON tool result the crusher provably shrinks.
    fn repetitive_json_rows(n: usize) -> String {
        let rows: Vec<serde_json::Value> = (0..n)
            .map(|i| serde_json::json!({"id": i, "level": "info", "message": "heartbeat ok"}))
            .collect();
        serde_json::Value::Array(rows).to_string()
    }

    /// Seed a session with three big JSON tool results (the oldest is fair
    /// game for compression; the last two are protected as "recent").
    fn seed_big_tool_results(store: &HistoryStore) -> String {
        let session = store
            .create_session(&SessionMeta {
                workspace: "/tmp/proj".into(),
                model: "claude-opus-4-8".into(),
                ..Default::default()
            })
            .unwrap();
        for i in 0..3 {
            store
                .append_message(&session, &ChatMessage::user(format!("q{i}")))
                .unwrap();
            store
                .append_message(
                    &session,
                    &ChatMessage::tool_result(format!("t{i}"), repetitive_json_rows(200)),
                )
                .unwrap();
        }
        session
    }

    #[tokio::test]
    async fn compression_on_shrinks_the_request_but_never_the_transcript() {
        let mut server = mockito::Server::new_async().await;
        // The mock only matches a request whose body carries a CCR sentinel —
        // an uncompressed request gets no response and the turn errors.
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("_ccr_dropped".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("all done"))
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = seed_big_tool_results(&store);
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            compression: CompressionMode::On,
            ..AgentConfig::default()
        };
        let mut agent =
            Agent::resume_from_store(client, ToolRegistry::new(), store, session, config).unwrap();

        // The retrieve tool rides along whenever compression is on.
        let defs = agent.tool_definitions();
        assert!(
            defs.iter()
                .any(|d| d["function"]["name"] == "retrieve_original"),
            "retrieve_original should be registered with compression on"
        );

        let mut compression_events = Vec::new();
        let out = agent
            .run_turn("continue", |e| {
                if let AgentEvent::Compression { .. } = e {
                    compression_events.push(e.clone());
                }
            })
            .await
            .expect("turn should succeed with a compressed request");
        assert_eq!(out, "all done");

        let AgentEvent::Compression {
            mode,
            saved_tokens,
            results_compressed,
            ..
        } = &compression_events[0]
        else {
            panic!("expected a compression event");
        };
        assert_eq!(mode, "on");
        assert!(*saved_tokens > 0);
        // Only the stale tool result is compressed; the recent two are protected.
        assert_eq!(*results_compressed, 1);
        assert_eq!(agent.tokens_saved(), *saved_tokens);

        // The transcript (memory + store) still holds every original byte.
        let originals = agent
            .messages()
            .iter()
            .filter(|m| m.role == "tool")
            .filter(|m| m.content_text().is_some_and(|t| t.contains("heartbeat ok")))
            .count();
        assert_eq!(originals, 3, "in-memory transcript must stay uncompressed");
    }

    #[test]
    fn live_mode_switch_registers_and_removes_the_retrieve_tool() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new("http://localhost/api/ai", "key", "claude-opus-4-8");
        let mut agent = Agent::new(
            client,
            ToolRegistry::new(),
            store,
            session,
            AgentConfig::default(),
        )
        .unwrap();
        let has_retrieve = |agent: &Agent| {
            agent
                .tool_definitions()
                .iter()
                .any(|d| d["function"]["name"] == "retrieve_original")
        };

        assert_eq!(agent.compression_mode(), CompressionMode::Off);
        assert!(!has_retrieve(&agent));

        agent.set_compression_mode(CompressionMode::On);
        assert_eq!(agent.compression_mode(), CompressionMode::On);
        assert!(has_retrieve(&agent), "On registers the retrieve tool");

        agent.set_compression_mode(CompressionMode::Audit);
        assert_eq!(agent.compression_mode(), CompressionMode::Audit);
        assert!(!has_retrieve(&agent), "leaving On removes it");
    }

    #[tokio::test]
    async fn audit_mode_measures_savings_but_sends_the_original_request() {
        let mut server = mockito::Server::new_async().await;
        // Match only an *uncompressed* request: the oldest tool result's rows
        // all present (row 150 only survives if nothing was sampled away) and
        // no CCR sentinel anywhere.
        server
            .mock("POST", "/chat/completions")
            .match_request(|req| {
                let body = String::from_utf8_lossy(req.body().unwrap()).to_string();
                body.contains("\\\"id\\\":150") && !body.contains("_ccr_dropped")
            })
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("all done"))
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = seed_big_tool_results(&store);
        let client = OxenClient::new(server.url(), "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            compression: CompressionMode::Audit,
            ..AgentConfig::default()
        };
        let mut agent =
            Agent::resume_from_store(client, ToolRegistry::new(), store, session, config).unwrap();

        // No markers are sent in audit mode, so no retrieve tool either.
        assert!(agent.tool_definitions().is_empty());

        let mut audit_saved = 0usize;
        let out = agent
            .run_turn("continue", |e| {
                if let AgentEvent::Compression {
                    mode, saved_tokens, ..
                } = e
                {
                    assert_eq!(mode, "audit");
                    audit_saved += saved_tokens;
                }
            })
            .await
            .expect("audit turn must send the untouched request");
        assert_eq!(out, "all done");
        assert!(audit_saved > 0, "audit should report would-be savings");
    }
}
