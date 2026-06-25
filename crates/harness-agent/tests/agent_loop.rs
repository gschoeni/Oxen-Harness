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
