//! Integration test for the agent (Ralph) loop.
//!
//! Uses a scripted mock Oxen endpoint: the first model response streams a tool
//! call, the second streams a final answer. A custom `add` tool runs in between.
//! This exercises the full loop end to end, including streaming, tool dispatch,
//! and verbatim history persistence.

use std::sync::Arc;

use async_trait::async_trait;
use harness_agent::{Agent, AgentConfig, AgentEvent};
use harness_llm::OxenClient;
use harness_store::HistoryStore;
use harness_tools::{Tool, ToolError, ToolRegistry};

struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &str {
        "add"
    }
    fn description(&self) -> &str {
        "Add two integers a and b."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "a": {"type": "integer"}, "b": {"type": "integer"} },
            "required": ["a", "b"]
        })
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let a = args["a"]
            .as_i64()
            .ok_or_else(|| ToolError::InvalidArguments("a".into()))?;
        let b = args["b"]
            .as_i64()
            .ok_or_else(|| ToolError::InvalidArguments("b".into()))?;
        Ok((a + b).to_string())
    }
}

const TOOL_CALL_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"add\",\"arguments\":\"{\\\"a\\\":2,\\\"b\\\":3}\"}}]}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: [DONE]\n\n"
);

const FINAL_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"The sum is 5.\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n"
);

// A text-only reply that *announces* an action without performing it — the
// "announce the plan, then stop" shape the nudge is meant to catch.
const INTENT_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Sure! I'll add those two numbers for you.\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n"
);

/// A text-only response (no tool calls) ends the turn immediately — the loop's
/// stop condition. This is exactly the behavior behind the "announced the plan
/// then stopped" case: when the model returns prose with no tool call, the loop
/// makes a single model call and hands control back rather than continuing. The
/// fix lives in the system prompt (see the lib unit test); the loop semantics
/// here are intentional and pinned by this regression guard.
#[tokio::test]
async fn text_only_response_ends_turn_after_one_call() {
    let mut server = mockito::Server::new_async().await;

    let m1 = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(FINAL_SSE)
        // Exactly one model call: no tool call means no second round-trip.
        .expect(1)
        .create_async()
        .await;

    let client = OxenClient::new(server.url(), "sk-test", "claude-opus-4-8");
    let tools = ToolRegistry::new().with(Arc::new(AddTool));
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let session = store
        .create_session(&harness_store::SessionMeta {
            workspace: "/tmp/proj".into(),
            model: "claude-opus-4-8".into(),
            ..Default::default()
        })
        .unwrap();

    let config = AgentConfig {
        model: "claude-opus-4-8".into(),
        system_prompt: None,
        ..AgentConfig::default()
    };
    let mut agent = Agent::new(client, tools, store.clone(), session.clone(), config).unwrap();

    let mut events = Vec::new();
    let final_text = agent
        .run_turn("just say something", |e| events.push(e.clone()))
        .await
        .unwrap();

    m1.assert_async().await;

    assert_eq!(final_text, "The sum is 5.");
    // No tool ran — the turn stopped on the text-only reply.
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolStart { .. })));

    // Only the user message and the single assistant reply were persisted.
    let stored = store.messages(&session).unwrap();
    let roles: Vec<&str> = stored.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, vec!["user", "assistant"]);
}

/// When the model returns a text-only reply that reads as an announced-but-
/// unperformed action ("I'll add those numbers…"), the loop nudges it once and
/// runs another round, so an intent-to-act doesn't silently end the turn. The
/// nudge is ephemeral — it never lands in the persisted transcript.
#[tokio::test]
async fn unfulfilled_intent_reply_nudges_once_then_continues() {
    let mut server = mockito::Server::new_async().await;

    // Scripted in order: intent-only prose -> (nudge) -> tool call -> final.
    let m1 = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(INTENT_SSE)
        .expect(1)
        .create_async()
        .await;
    let m2 = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(TOOL_CALL_SSE)
        .expect(1)
        .create_async()
        .await;
    let m3 = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(FINAL_SSE)
        .expect(1)
        .create_async()
        .await;

    let client = OxenClient::new(server.url(), "sk-test", "claude-opus-4-8");
    let tools = ToolRegistry::new().with(Arc::new(AddTool));
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let session = store
        .create_session(&harness_store::SessionMeta {
            workspace: "/tmp/proj".into(),
            model: "claude-opus-4-8".into(),
            ..Default::default()
        })
        .unwrap();

    let config = AgentConfig {
        model: "claude-opus-4-8".into(),
        system_prompt: None,
        ..AgentConfig::default()
    };
    let mut agent = Agent::new(client, tools, store.clone(), session.clone(), config).unwrap();

    let mut events = Vec::new();
    let final_text = agent
        .run_turn("please add 2 and 3", |e| events.push(e.clone()))
        .await
        .unwrap();

    m1.assert_async().await;
    m2.assert_async().await;
    m3.assert_async().await;

    assert_eq!(final_text, "The sum is 5.");
    // The tool ultimately ran — the nudge got the model off the plan and acting.
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolStart { name, .. } if name == "add"
    )));

    // The ephemeral nudge is NOT persisted: the transcript shows the preamble
    // assistant message, then the tool-call assistant message, with no synthetic
    // user turn between them.
    let stored = store.messages(&session).unwrap();
    let roles: Vec<&str> = stored.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "assistant", "tool", "assistant"]
    );
}

#[tokio::test]
async fn loop_calls_tool_then_returns_final_answer() {
    let mut server = mockito::Server::new_async().await;

    // Scripted in order: first request -> tool call, second -> final answer.
    let m1 = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(TOOL_CALL_SSE)
        .expect(1)
        .create_async()
        .await;
    let m2 = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(FINAL_SSE)
        .expect(1)
        .create_async()
        .await;

    let client = OxenClient::new(server.url(), "sk-test", "claude-opus-4-8");
    let tools = ToolRegistry::new().with(Arc::new(AddTool));
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let session = store
        .create_session(&harness_store::SessionMeta {
            workspace: "/tmp/proj".into(),
            model: "claude-opus-4-8".into(),
            ..Default::default()
        })
        .unwrap();

    let config = AgentConfig {
        model: "claude-opus-4-8".into(),
        system_prompt: None,
        ..AgentConfig::default()
    };
    let mut agent = Agent::new(client, tools, store.clone(), session.clone(), config).unwrap();

    let mut events = Vec::new();
    let final_text = agent
        .run_turn("please add 2 and 3", |e| events.push(e.clone()))
        .await
        .unwrap();

    m1.assert_async().await;
    m2.assert_async().await;

    assert_eq!(final_text, "The sum is 5.");

    // Tool activity surfaced to the caller.
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolStart { name, .. } if name == "add"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolEnd { result, .. } if result == "5"
    )));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::Token(t) if t.contains("sum"))));

    // History persisted verbatim: user, assistant(tool_calls), tool, assistant(final).
    let stored = store.messages(&session).unwrap();
    let roles: Vec<&str> = stored.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, vec!["user", "assistant", "tool", "assistant"]);
    assert_eq!(stored[1]["tool_calls"][0]["function"]["name"], "add");
    assert_eq!(stored[2]["content"], "5");
}
