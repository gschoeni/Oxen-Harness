//! Shared helpers for this crate's unit tests (compiled only under
//! `cfg(test)`): session-creation boilerplate and canned SSE bodies for
//! `mockito`-backed model endpoints.

use harness_store::{HistoryStore, SessionMeta};

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
