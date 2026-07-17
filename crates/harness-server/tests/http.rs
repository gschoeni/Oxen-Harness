//! The HTTP protocol end to end: a real server on an ephemeral port, a real
//! (mock-LLM-backed) session service behind it, and a plain HTTP client — the
//! exact way a third-party UI consumes the harness.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use harness_llm::OxenClient;
use harness_server::{serve_on_ephemeral_port, ServerHandle};
use harness_store::HistoryStore;
use serde_json::{json, Value};

const TOKEN: &str = "test-token-123";

fn isolate_config_home() {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    let path = HOME.get_or_init(|| tempfile::tempdir().expect("config home").keep());
    std::env::set_var("OXEN_HARNESS_DIR", path);
}

const FINAL_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"The sum is 5.\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":200,\"completion_tokens\":10,\"total_tokens\":210}}\n\n",
    "data: [DONE]\n\n"
);

const ASK_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"ask_user_question\",\"arguments\":\"{\\\"questions\\\":[{\\\"question\\\":\\\"Which DB?\\\",\\\"header\\\":\\\"Storage\\\",\\\"options\\\":[{\\\"label\\\":\\\"SQLite\\\",\\\"description\\\":\\\"file\\\"},{\\\"label\\\":\\\"Postgres\\\",\\\"description\\\":\\\"server\\\"}],\\\"multiSelect\\\":false}]}\"}}]}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":5,\"total_tokens\":105}}\n\n",
    "data: [DONE]\n\n"
);

/// Boot a server wired to `llm_url`'s mock endpoint, returning its handle.
async fn boot(llm_url: String, workspace: &std::path::Path) -> ServerHandle {
    isolate_config_home();
    let url = llm_url.clone();
    let workspace = workspace.to_path_buf();
    serve_on_ephemeral_port(harness_server::ServerConfig {
        token: TOKEN.to_string(),
        configure: Some(Box::new(move |builder| {
            builder
                .cloud_model("claude-opus-4-8")
                .active_project(workspace)
                .store(Arc::new(HistoryStore::open_in_memory().unwrap()))
                .client_factory(move |model| Ok(OxenClient::new(url.clone(), "sk-test", model)))
        })),
    })
    .await
    .expect("server boots")
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

fn sse_mock(server: &mut mockito::ServerGuard, body: &'static str) -> mockito::Mock {
    server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(body)
        .expect(1)
        .create()
}

/// Read SSE `data:` payloads from a live response stream until `pred` matches
/// (returning the matching event) or the timeout lapses.
async fn next_matching(response: &mut reqwest::Response, pred: impl Fn(&Value) -> bool) -> Value {
    let mut buffer = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let chunk = tokio::time::timeout_at(deadline, response.chunk())
            .await
            .expect("timed out waiting for SSE event")
            .expect("stream read")
            .expect("stream ended before the expected event");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buffer.find("\n\n") {
            let frame: String = buffer.drain(..pos + 2).collect();
            for line in frame.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(value) = serde_json::from_str::<Value>(data) {
                        if pred(&value) {
                            return value;
                        }
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn rejects_requests_without_the_bearer_token() {
    let llm = mockito::Server::new_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let server = boot(llm.url(), workspace.path()).await;

    // No token → 401.
    let response = client()
        .get(format!("{}/v1/sessions", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);

    // Wrong token → 401.
    let response = client()
        .get(format!("{}/v1/sessions", server.base_url()))
        .bearer_auth("wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);

    // Right token → 200.
    let response = client()
        .get(format!("{}/v1/sessions", server.base_url()))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // The SSE endpoint also accepts the token as a query parameter (browser
    // EventSource can't set headers).
    let response = client()
        .get(format!("{}/v1/events?token={TOKEN}", server.base_url()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn runs_a_turn_and_streams_protocol_events_over_sse() {
    let mut llm = mockito::Server::new_async().await;
    let m1 = sse_mock(&mut llm, FINAL_SSE);
    let workspace = tempfile::tempdir().unwrap();
    let server = boot(llm.url(), workspace.path()).await;
    let base = server.base_url();

    // Create a session.
    let info: Value = client()
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = info["session_id"].as_str().unwrap().to_string();
    assert_eq!(info["model"], "claude-opus-4-8");

    // Open the event stream BEFORE the turn so we see it from the start.
    let mut events = client()
        .get(format!("{base}/v1/events?token={TOKEN}"))
        .send()
        .await
        .unwrap();

    // Run the turn.
    let turn: Value = client()
        .post(format!("{base}/v1/sessions/{session}/turns"))
        .bearer_auth(TOKEN)
        .json(&json!({"prompt": "say something"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(turn["text"], "The sum is 5.");
    m1.assert_async().await;

    // The stream carried the turn lifecycle and the streamed text.
    let started = next_matching(&mut events, |e| e["type"] == "turn.started").await;
    assert_eq!(started["session"], session.as_str());
    let token_event = next_matching(&mut events, |e| e["type"] == "agent.token").await;
    assert_eq!(token_event["token"], "The sum is 5.");
    let completed = next_matching(&mut events, |e| e["type"] == "turn.completed").await;
    assert_eq!(completed["text"], "The sum is 5.");
}

#[tokio::test]
async fn question_round_trip_over_http() {
    let mut llm = mockito::Server::new_async().await;
    let _m1 = sse_mock(&mut llm, ASK_SSE);
    let _m2 = sse_mock(&mut llm, FINAL_SSE);
    let workspace = tempfile::tempdir().unwrap();
    let server = boot(llm.url(), workspace.path()).await;
    let base = server.base_url();

    let info: Value = client()
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = info["session_id"].as_str().unwrap().to_string();

    let mut events = client()
        .get(format!("{base}/v1/events?token={TOKEN}"))
        .send()
        .await
        .unwrap();

    // Fire the turn without waiting for it (it parks on the question).
    let turn_base = base.clone();
    let turn_session = session.clone();
    let turn = tokio::spawn(async move {
        client()
            .post(format!("{turn_base}/v1/sessions/{turn_session}/turns"))
            .bearer_auth(TOKEN)
            .json(&json!({"prompt": "ask me"}))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });

    // The question arrives on the stream; answer it over REST.
    let question = next_matching(&mut events, |e| e["type"] == "agent.question").await;
    assert_eq!(question["session"], session.as_str());
    assert_eq!(question["questions"][0]["header"], "Storage");
    let id = question["id"].as_str().unwrap();

    let answered = client()
        .post(format!("{base}/v1/questions/{id}/answer"))
        .bearer_auth(TOKEN)
        .json(&json!({"answers": [{
            "header": "Storage",
            "question": "Which DB?",
            "selected": ["SQLite"],
        }]}))
        .send()
        .await
        .unwrap();
    assert_eq!(answered.status(), 200);

    let turn = turn.await.unwrap();
    assert_eq!(turn["text"], "The sum is 5.");
}

#[tokio::test]
async fn session_lifecycle_and_replay() {
    let mut llm = mockito::Server::new_async().await;
    let _m1 = sse_mock(&mut llm, FINAL_SSE);
    let workspace = tempfile::tempdir().unwrap();
    let server = boot(llm.url(), workspace.path()).await;
    let base = server.base_url();

    let info: Value = client()
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = info["session_id"].as_str().unwrap().to_string();

    client()
        .post(format!("{base}/v1/sessions/{session}/turns"))
        .bearer_auth(TOKEN)
        .json(&json!({"prompt": "hello"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // The session lists.
    let sessions: Value = client()
        .get(format!("{base}/v1/sessions"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sessions.as_array().unwrap().len(), 1);

    // Resuming returns the transcript.
    let view: Value = client()
        .get(format!("{base}/v1/sessions/{session}"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(view["running"], false);
    assert!(!view["messages"].as_array().unwrap().is_empty());

    // Raw messages endpoint works too.
    let raw: Value = client()
        .get(format!("{base}/v1/sessions/{session}/messages"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!raw.as_array().unwrap().is_empty());

    // A late subscriber with Last-Event-ID 0 replays the backlog: the turn's
    // events are still there.
    let mut events = client()
        .get(format!("{base}/v1/events?token={TOKEN}"))
        .header("Last-Event-ID", "0")
        .send()
        .await
        .unwrap();
    let replayed = next_matching(&mut events, |e| e["type"] == "turn.completed").await;
    assert_eq!(replayed["session"], session.as_str());

    // Cancel on an idle session is a 200 no-op.
    let response = client()
        .post(format!("{base}/v1/sessions/{session}/cancel"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // Delete removes it.
    let response = client()
        .delete(format!("{base}/v1/sessions/{session}"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let sessions: Value = client()
        .get(format!("{base}/v1/sessions"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(sessions.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn uploads_an_attachment_and_reports_health() {
    let llm = mockito::Server::new_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let server = boot(llm.url(), workspace.path()).await;
    let base = server.base_url();

    let health: Value = client()
        .get(format!("{base}/v1/health"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["status"], "ok");

    let uploaded: Value = client()
        .post(format!("{base}/v1/attachments?filename=note.txt"))
        .bearer_auth(TOKEN)
        .body("hello attachment")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let path = uploaded["path"].as_str().unwrap();
    assert!(path.ends_with("note.txt"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "hello attachment");
}

#[tokio::test]
async fn filters_events_by_session() {
    let mut llm = mockito::Server::new_async().await;
    let _m1 = sse_mock(&mut llm, FINAL_SSE);
    let workspace = tempfile::tempdir().unwrap();
    let server = boot(llm.url(), workspace.path()).await;
    let base = server.base_url();

    let a: Value = client()
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b: Value = client()
        .post(format!("{base}/v1/sessions"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_a = a["session_id"].as_str().unwrap().to_string();
    let session_b = b["session_id"].as_str().unwrap().to_string();

    // Subscribe to session B only, then run a turn in session A.
    let mut events_b = client()
        .get(format!(
            "{base}/v1/events?token={TOKEN}&session={session_b}"
        ))
        .send()
        .await
        .unwrap();

    client()
        .post(format!("{base}/v1/sessions/{session_a}/turns"))
        .bearer_auth(TOKEN)
        .json(&json!({"prompt": "hello"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // B's stream must not carry A's events. Give the broadcast a beat, then
    // read whatever arrived with a short deadline: only keepalives allowed.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let leaked = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            match events_b.chunk().await {
                Ok(Some(chunk)) => {
                    let text = String::from_utf8_lossy(&chunk).to_string();
                    if text.contains(&format!("\"session\":\"{session_a}\"")) {
                        return true;
                    }
                }
                _ => return false,
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(
        !leaked,
        "session A's events leaked into B's filtered stream"
    );
}
