//! The session service driven exactly the way a host transport (Tauri IPC,
//! HTTP server) drives it: build against a scripted mock LLM endpoint, run
//! turns, and assert on the protocol events a client would receive.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use harness_host::{EventSink, SessionService};
use harness_llm::OxenClient;
use harness_protocol::{ProtocolEvent, QuestionAnswer, ToolPhase};
use harness_store::HistoryStore;

/// Collects every emitted protocol event for assertions.
#[derive(Default)]
struct CollectingSink(Mutex<Vec<ProtocolEvent>>);

impl EventSink for CollectingSink {
    fn emit(&self, event: ProtocolEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl CollectingSink {
    fn events(&self) -> Vec<ProtocolEvent> {
        self.0.lock().unwrap().clone()
    }
}

/// Isolate every test in this binary from the user's real `~/.oxen-harness`
/// (tool prefs, permissions, skills would otherwise leak into agent builds).
/// One shared config home for the whole binary: set once, kept alive forever.
fn isolate_config_home() {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    let path = HOME.get_or_init(|| {
        let dir = tempfile::tempdir().expect("config home");
        dir.keep()
    });
    std::env::set_var("OXEN_HARNESS_DIR", path);
}

const FINAL_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"The sum is 5.\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":200,\"completion_tokens\":10,\"total_tokens\":210}}\n\n",
    "data: [DONE]\n\n"
);

/// A scripted `ask_user_question` call, so one turn exercises tool events and
/// the question round-trip without any real tool side effects.
const ASK_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"ask_user_question\",\"arguments\":\"{\\\"questions\\\":[{\\\"question\\\":\\\"Which DB?\\\",\\\"header\\\":\\\"Storage\\\",\\\"options\\\":[{\\\"label\\\":\\\"SQLite\\\",\\\"description\\\":\\\"file\\\"},{\\\"label\\\":\\\"Postgres\\\",\\\"description\\\":\\\"server\\\"}],\\\"multiSelect\\\":false}]}\"}}]}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":5,\"total_tokens\":105}}\n\n",
    "data: [DONE]\n\n"
);

/// A service wired to `server_url`'s mock endpoint, an in-memory store, an
/// isolated workspace, and the collecting sink.
fn service_for(
    server_url: String,
    sink: Arc<CollectingSink>,
    workspace: &std::path::Path,
) -> Arc<SessionService> {
    isolate_config_home();
    let url = server_url.clone();
    Arc::new(
        SessionService::builder(sink)
            .cloud_model("claude-opus-4-8")
            .store(Arc::new(HistoryStore::open_in_memory().unwrap()))
            .active_project(workspace)
            .client_factory(move |model| Ok(OxenClient::new(url.clone(), "sk-test", model)))
            .build(),
    )
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

/// Poll `f` until it returns Some or the timeout lapses; dump the events the
/// sink actually saw on failure so a broken stream is diagnosable.
async fn wait_for<T>(sink: &CollectingSink, mut f: impl FnMut() -> Option<T>) -> T {
    for _ in 0..200 {
        if let Some(v) = f() {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition not met within timeout; events seen: {:#?}", sink.events());
}

#[tokio::test]
async fn turn_streams_protocol_events_and_returns_text() {
    let mut server = mockito::Server::new_async().await;
    let m1 = sse_mock(&mut server, FINAL_SSE);

    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink.clone(), workspace.path());

    let info = service.new_session().await.expect("new session");
    assert!(!info.session_id.is_empty());
    assert_eq!(info.model, "claude-opus-4-8");

    let text = service
        .run_turn(&info.session_id, "say something".into(), vec![])
        .await
        .expect("turn runs");
    assert_eq!(text, "The sum is 5.");
    m1.assert_async().await;

    let events = sink.events();
    let session = info.session_id.as_str();

    // Turn lifecycle brackets the stream.
    assert!(matches!(
        events.first(),
        Some(ProtocolEvent::TurnStarted { session: s }) if s == session
    ));
    assert!(matches!(
        events.last(),
        Some(ProtocolEvent::TurnCompleted { session: s, text }) if s == session && text == "The sum is 5."
    ));

    // The streamed tokens concatenate to the final text, session-tagged.
    let streamed: String = events
        .iter()
        .filter_map(|e| match e {
            ProtocolEvent::Token { session: s, token } if s == session => Some(token.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(streamed, "The sum is 5.");

    // Usage reached the sink with the context window filled in.
    assert!(events.iter().any(|e| matches!(
        e,
        ProtocolEvent::Usage { session: s, tokens_used, .. } if s == session && *tokens_used > 0
    )));
}

#[tokio::test]
async fn question_round_trip_over_the_protocol() {
    let mut server = mockito::Server::new_async().await;
    let m1 = sse_mock(&mut server, ASK_SSE);
    let m2 = sse_mock(&mut server, FINAL_SSE);

    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink.clone(), workspace.path());

    let info = service.new_session().await.unwrap();
    let session = info.session_id.clone();

    // Drive the turn concurrently: it parks on the question until we answer.
    let turn = tokio::spawn({
        let service = service.clone();
        let session = session.clone();
        async move { service.run_turn(&session, "ask me".into(), vec![]).await }
    });

    // The question event arrives, session-tagged and carrying the payload.
    let (id, questions) = wait_for(&sink, || {
        sink.events().into_iter().find_map(|e| match e {
            ProtocolEvent::Question {
                session: s,
                id,
                questions,
            } if s == session => Some((id, questions)),
            _ => None,
        })
    })
    .await;
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].header, "Storage");
    assert_eq!(questions[0].options[0].label, "SQLite");

    // Answer it; the turn resumes and completes.
    service.answer_question(
        &id,
        vec![QuestionAnswer {
            header: "Storage".into(),
            question: "Which DB?".into(),
            selected: vec!["SQLite".into()],
        }],
    );
    let text = turn.await.unwrap().expect("turn completes");
    assert_eq!(text, "The sum is 5.");
    m1.assert_async().await;
    m2.assert_async().await;

    // The ask tool's start/end bracketed the question on the stream.
    let events = sink.events();
    assert!(events.iter().any(|e| matches!(
        e,
        ProtocolEvent::Tool { phase: ToolPhase::Start, name, .. } if name == "ask_user_question"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        ProtocolEvent::Tool { phase: ToolPhase::End, name, .. } if name == "ask_user_question"
    )));
}

#[tokio::test]
async fn session_lifecycle_new_resume_list_delete() {
    let mut server = mockito::Server::new_async().await;
    let _m1 = sse_mock(&mut server, FINAL_SSE);

    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink.clone(), workspace.path());

    let info = service.new_session().await.unwrap();
    service
        .run_turn(&info.session_id, "hello".into(), vec![])
        .await
        .unwrap();

    // The session lists (it has a user message now).
    let sessions = service.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, info.session_id);

    // Resume returns the transcript: user turn + assistant reply.
    let view = service.resume_session(&info.session_id).await.unwrap();
    assert!(!view.running);
    assert_eq!(view.info.session_id, info.session_id);
    let roles: Vec<&str> = view
        .messages
        .iter()
        .filter_map(|m| m["role"].as_str())
        .collect();
    assert!(roles.contains(&"user"));
    assert!(roles.contains(&"assistant"));

    // Raw persisted messages are readable without touching the live agent.
    let raw = service.session_messages(&info.session_id).unwrap();
    assert!(!raw.is_empty());

    // Delete removes it from history.
    service.delete_session(&info.session_id).await.unwrap();
    assert!(service.list_sessions().unwrap().is_empty());
}

#[tokio::test]
async fn set_model_swaps_the_live_agent() {
    let server = mockito::Server::new_async().await;
    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink.clone(), workspace.path());

    let info = service.new_session().await.unwrap();
    assert_eq!(info.model, "claude-opus-4-8");

    let info = service.set_model("some-other-model").await.unwrap();
    assert_eq!(info.model, "some-other-model");

    // The current session reports the swapped model too.
    let current = service.session_info().await.unwrap();
    assert_eq!(current.model, "some-other-model");
}

#[tokio::test]
async fn cancel_turn_without_a_running_turn_is_a_noop() {
    let server = mockito::Server::new_async().await;
    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink, workspace.path());

    let info = service.new_session().await.unwrap();
    service.cancel_turn(&info.session_id).await; // must not panic or error
}

#[tokio::test]
async fn answering_an_unknown_question_id_is_ignored() {
    let server = mockito::Server::new_async().await;
    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink, workspace.path());

    service.answer_question("nope", vec![]); // silently ignored
    service.answer_approval(
        "nope",
        harness_protocol::ApprovalAnswer {
            decision: "once".into(),
            message: None,
        },
    );
}
