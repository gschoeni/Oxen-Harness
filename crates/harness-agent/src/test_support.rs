//! Shared helpers for this crate's unit tests (compiled only under
//! `cfg(test)`): session-creation boilerplate and canned SSE bodies for
//! `mockito`-backed model endpoints.

use std::sync::Arc;

use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::ToolRegistry;

use crate::{Agent, AgentConfig, RetryPolicy};

/// Create a throwaway session in `store` with the standard test workspace.
pub(crate) fn test_session(store: &HistoryStore, model: &str) -> String {
    store
        .create_session(&SessionMeta {
            workspace: "/tmp/proj".into(),
            model: model.into(),
            ..Default::default()
        })
        .unwrap()
}

/// SSE body for a plain prose reply (no tool calls) that ends the turn.
pub(crate) fn sse_prose(text: &str) -> String {
    let chunk = serde_json::json!({
        "choices": [{
            "index": 0,
            "delta": { "content": text },
            "finish_reason": "stop"
        }]
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

/// A retry policy with near-zero waits so backoff tests run instantly.
pub(crate) fn fast_retry(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        base_delay: std::time::Duration::from_millis(1),
        fallback_models: Vec::new(),
    }
}

pub(crate) fn retry_test_agent(url: String, retry: RetryPolicy) -> Agent {
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

/// SSE for a reply that calls a tool named `snap` with empty args.
pub(crate) fn sse_snap_call() -> String {
    let chunk = serde_json::json!({
        "choices": [{
            "index": 0,
            "delta": {
                "content": "",
                "tool_calls": [{
                    "index": 0,
                    "id": "call_snap",
                    "function": { "name": "snap", "arguments": "{}" }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}
