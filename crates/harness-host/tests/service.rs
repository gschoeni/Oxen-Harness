//! The session service driven exactly the way a host transport (Tauri IPC,
//! HTTP server) drives it: build against a scripted mock LLM endpoint, run
//! turns, and assert on the protocol events a client would receive.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use harness_config::features::FeatureFlags;
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
    service_with_features(server_url, sink, workspace, FeatureFlags::default())
}

fn service_with_features(
    server_url: String,
    sink: Arc<CollectingSink>,
    workspace: &std::path::Path,
    features: FeatureFlags,
) -> Arc<SessionService> {
    isolate_config_home();
    let url = server_url.clone();
    Arc::new(
        SessionService::builder(sink)
            .feature_flags(features)
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
    panic!(
        "condition not met within timeout; events seen: {:#?}",
        sink.events()
    );
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

/// The thread overview end-to-end: a turn leaves a titled, fresh entry;
/// finishing and reopening round-trip; the seen mark is recorded.
#[tokio::test]
async fn thread_snapshot_derives_entries_and_finish_round_trips() {
    let mut server = mockito::Server::new_async().await;
    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink, workspace.path());

    let mock = sse_mock(&mut server, FINAL_SSE);
    let info = service.new_session().await.unwrap();
    let session = info.session_id.clone();
    service
        .run_turn(&session, "what is 2 + 3?".to_string(), vec![])
        .await
        .unwrap();
    mock.assert_async().await;

    // A curation verdict surfaces on the entry.
    let store = service.store().unwrap();
    store.set_review_status(&session, "kept").unwrap();

    let snapshot = service.thread_snapshot().await.unwrap();
    assert!(snapshot.running.is_empty());
    let entry = snapshot
        .entries
        .iter()
        .find(|e| e.id == session)
        .expect("the session has an entry");
    assert_eq!(entry.title, "what is 2 + 3?");
    assert_eq!(entry.finished_at, 0);
    assert_eq!(entry.seen_at, 0, "never opened since the mark existed");
    assert_eq!(entry.review_status, "kept");
    // The reply landed: not mid-turn.
    assert!(!entry.mid_turn);

    // A thread with work in flight refuses to finish — a list's render gate
    // can race a turn start, so the contract lives server-side.
    service
        .cancels
        .lock()
        .await
        .insert(session.clone(), tokio_util::sync::CancellationToken::new());
    let err = service.finish_session(&session).await.unwrap_err();
    assert!(err.contains("mid-turn"), "got: {err}");
    service.cancels.lock().await.remove(&session);

    // Finish, see it in the snapshot, then reopen.
    let finished_at = service.finish_session(&session).await.unwrap();
    assert!(finished_at > 0);
    let snapshot = service.thread_snapshot().await.unwrap();
    let entry = snapshot.entries.iter().find(|e| e.id == session).unwrap();
    assert_eq!(entry.finished_at, finished_at);

    service.reopen_session(&session).unwrap();
    let snapshot = service.thread_snapshot().await.unwrap();
    let entry = snapshot.entries.iter().find(|e| e.id == session).unwrap();
    assert_eq!(entry.finished_at, 0);

    // Finishing a session that doesn't exist is a clean error.
    assert!(service.finish_session("nope").await.is_err());

    // Marking seen lands on the entry the next snapshot reports.
    let seen = service.mark_session_seen(&session).unwrap();
    assert!(seen > 0);
    let snapshot = service.thread_snapshot().await.unwrap();
    let entry = snapshot.entries.iter().find(|e| e.id == session).unwrap();
    assert_eq!(entry.seen_at, seen);
}

/// The other half of the finish/turn-start race: however the two interleave,
/// a running thread is never finished. `finish_session` holds the running-set
/// lock across its check-and-write (tested above); here, a run that starts
/// AFTER a finish landed clears the mark — running again means open again.
#[tokio::test]
async fn a_run_starting_on_a_finished_thread_reopens_it() {
    let mut server = mockito::Server::new_async().await;
    let _m = sse_mock(&mut server, FINAL_SSE);
    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink, workspace.path());

    let info = service.new_session().await.unwrap();
    service
        .run_turn(&info.session_id, "hello".into(), vec![])
        .await
        .unwrap();
    service.finish_session(&info.session_id).await.unwrap();

    service
        .run_turn(&info.session_id, "actually, keep going".into(), vec![])
        .await
        .unwrap();
    let snapshot = service.thread_snapshot().await.unwrap();
    let entry = snapshot
        .entries
        .iter()
        .find(|e| e.id == info.session_id)
        .unwrap();
    assert_eq!(
        entry.finished_at, 0,
        "a thread that ran again must not stay finished"
    );
}

/// Deleting a chat with a run in flight stops the run FIRST and waits for it
/// to deregister — history must never vanish under an agent still executing
/// tools. The fake run mirrors the real turn's contract: it removes itself
/// from the running set only after its post-cancel cleanup.
#[tokio::test]
async fn deleting_a_running_chat_cancels_and_awaits_the_run() {
    let mut server = mockito::Server::new_async().await;
    let _m = sse_mock(&mut server, FINAL_SSE);
    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink, workspace.path());

    let info = service.new_session().await.unwrap();
    service
        .run_turn(&info.session_id, "hello".into(), vec![])
        .await
        .unwrap();

    let token = tokio_util::sync::CancellationToken::new();
    service
        .cancels
        .lock()
        .await
        .insert(info.session_id.clone(), token.clone());
    let fake_turn = {
        let service = service.clone();
        let id = info.session_id.clone();
        let token = token.clone();
        tokio::spawn(async move {
            token.cancelled().await;
            // "final persistence" before deregistering, like a real turn.
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            service.cancels.lock().await.remove(&id);
        })
    };

    service.delete_session(&info.session_id).await.unwrap();
    assert!(token.is_cancelled(), "delete must stop the run");
    assert!(service.list_sessions().unwrap().is_empty());
    fake_turn.await.unwrap();
}

#[tokio::test]
async fn agent_patch_requires_ownership_and_exact_review_and_applies_in_subdirectory() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Test"]);
    git(&["config", "user.email", "test@example.test"]);
    std::fs::create_dir(root.join("app")).unwrap();
    std::fs::write(root.join("app/file"), "before\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "initial"]);
    let service = service_for(
        "http://127.0.0.1:1".into(),
        Arc::default(),
        &root.join("app"),
    );
    let store = service.store().unwrap();
    let session = store
        .create_session(&harness_store::SessionMeta {
            workspace: root.join("app").display().to_string(),
            ..Default::default()
        })
        .unwrap();
    let lane = store
        .create_session(&harness_store::SessionMeta {
            parent_session: session.clone(),
            ..Default::default()
        })
        .unwrap();
    let stranger = store
        .create_session(&harness_store::SessionMeta::default())
        .unwrap();
    let worktrees = harness_agent::worktree::create(&root.join("app"), "host", 1).unwrap();
    std::fs::write(worktrees[0].path().join("file"), "after\n").unwrap();
    let snapshot =
        harness_agent::worktree::WorktreeSnapshot::capture(&worktrees[0], &lane).unwrap();
    store
        .save_session_state(&lane, harness_agent::LANE_WORKSPACE_STATE, &snapshot)
        .unwrap();
    drop(worktrees);
    assert!(service.agent_patch(&stranger, &lane).is_err());
    assert!(service
        .apply_agent_patch(&session, &lane, "old patch")
        .await
        .unwrap_err()
        .contains("Review"));
    service
        .apply_agent_patch(&session, &lane, &snapshot.patch)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("app/file")).unwrap(),
        "after\n"
    );
    assert!(service
        .apply_agent_patch(&session, &lane, &snapshot.patch)
        .await
        .unwrap_err()
        .contains("not applied"));
    assert_eq!(
        std::fs::read_to_string(root.join("app/file")).unwrap(),
        "after\n"
    );
}

#[tokio::test]
async fn workflow_uses_oxen_rewrite_records_outputs_and_rejects_stale_runs() {
    use serde_json::json;
    let workspace = tempfile::tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let mock=server.mock("POST","/chat/completions").with_header("content-type","application/json").with_body(json!({"choices":[{"message":{"role":"assistant","content":"A luminous cabin above the clouds."}}],"usage":{"prompt_tokens":10,"completion_tokens":8}}).to_string()).expect(1).create_async().await;
    let service = service_for(
        server.url(),
        Arc::new(CollectingSink::default()),
        workspace.path(),
    );
    let session = service.new_session().await.unwrap().session_id;
    let graph = json!({"version":1,"title":"Rewrite","nodes":[{"id":"p","kind":"prompt","position":{"x":0,"y":0},"config":{"text":"cabin in clouds"}},{"id":"r","kind":"rewrite","position":{"x":1,"y":0}},{"id":"o","kind":"output","position":{"x":2,"y":0}}],"edges":[{"id":"a","source":"p","target":"r","target_port":"prompt"},{"id":"b","source":"r","target":"o","target_port":"input"}]});
    let doc = service
        .save_document(&session, "test.graph.json", &graph.to_string(), None)
        .await
        .unwrap();
    let engine = service.workbench(&session).await.unwrap();
    assert!(engine
        .start(&doc.path, Some("stale"))
        .await
        .unwrap_err()
        .contains("changed on disk"));
    let run = engine.start(&doc.path, Some(&doc.revision)).await.unwrap();
    let done = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let run = engine.status(&run.id).unwrap();
            if run.status != "queued" && run.status != "running" {
                break run;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(done.status, "succeeded", "{:?}", done.error);
    assert_eq!(
        done.nodes["o"].outputs[0].value,
        "A luminous cabin above the clouds."
    );
    assert_eq!(
        service
            .store()
            .unwrap()
            .usage_for_session(&session)
            .unwrap()
            .completion_tokens,
        8
    );
    mock.assert_async().await;
    assert!(service
        .read_document("unknown-session", "test.graph.json")
        .unwrap_err()
        .contains("unknown work context"));
    engine.gate.set_plan_mode(true);
    assert!(engine
        .start(&doc.path, None)
        .await
        .unwrap_err()
        .contains("plan"));
}

#[tokio::test]
async fn bundled_view_descriptors_are_discoverable_and_openable_without_backend_registration() {
    use serde_json::json;
    let workspace = tempfile::tempdir().unwrap();
    let sink = Arc::new(CollectingSink::default());
    let service = service_for("http://127.0.0.1:1".into(), sink, workspace.path());
    let session = service.new_session().await.unwrap().session_id;
    service
        .save_document(&session, "notes/a.notes.json", "{}", None)
        .await
        .unwrap();
    service
        .workbench_request(
            &session,
            "register_views",
            json!({"views":[{
                "id":"community.notes","title":"Notes","description":"Project notes",
                "file_patterns":["*.notes.json"],"requires_file":true,"priority":20,
                "document_schema":{"type":"object","properties":{"text":{"type":"string"}}}
            }]}),
        )
        .await
        .unwrap();
    let available = service
        .workbench_request(&session, "list", json!({}))
        .await
        .unwrap();
    let notes = available["views"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "community.notes")
        .unwrap();
    assert_eq!(
        notes["document_schema"]["properties"]["text"]["type"],
        "string"
    );
    let opened = service
        .workbench_request(&session, "open", json!({"path":"notes/a.notes.json"}))
        .await
        .unwrap();
    assert_eq!(opened["view"], "community.notes");
    assert!(service.workbench_request(&session,"register_views",json!({"views":[{
        "id":"package:spoofed","title":"Spoof","description":"","file_patterns":[],"requires_file":false
    }]})).await.unwrap_err().contains("invalid bundled view id"));
}

#[tokio::test]
async fn release_flags_disable_authoring_but_keep_file_views_and_workflows() {
    use serde_json::json;
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(
        "http://127.0.0.1:1".into(),
        Arc::new(CollectingSink::default()),
        workspace.path(),
    );
    let session = service.new_session().await.unwrap().session_id;
    let agent = service.agent_or_build(&session).await.unwrap();
    let tools = agent.lock().await.tool_definitions();
    assert!(!tools
        .iter()
        .any(|tool| tool["function"]["name"] == "develop_view"));
    assert!(tools
        .iter()
        .any(|tool| tool["function"]["name"] == "open_view"));
    for action in ["scaffold", "preview", "check", "status", "install"] {
        let error = service
            .workbench_request(
                &session,
                "develop",
                json!({
                    "action": action, "source": "views/demo", "id": "demo.view", "title": "Demo"
                }),
            )
            .await
            .unwrap_err();
        assert!(error.contains("customization is disabled"), "{error}");
    }
    assert!(!workspace.path().join("views").exists());
    assert!(service
        .workbench(&session)
        .await
        .unwrap()
        .preview_lease()
        .await
        .is_err());
    for view in ["view-studio", "view-manager"] {
        assert!(service.workbench_request(&session, "register_views", json!({"views":[{
            "id": view, "title": "Authoring", "description": "", "file_patterns": [], "requires_file": false
        }]})).await.unwrap_err().contains("customization is disabled"));
        assert!(service
            .workbench_request(&session, "open", json!({"view":view}))
            .await
            .is_err());
    }
    for (path, expected) in [
        ("notes.txt", "editor"),
        ("studio.graph.json", "workflow"),
        ("report.canvas.json", "canvas"),
    ] {
        service
            .save_document(&session, path, "{}", None)
            .await
            .unwrap();
        assert_eq!(service.read_document(&session, path).unwrap().content, "{}");
        let opened = service
            .workbench_request(&session, "open", json!({"path":path}))
            .await
            .unwrap();
        assert_eq!(opened["view"], expected);
    }
}

#[tokio::test]
async fn view_authoring_is_workspace_scoped_and_keeps_drafts_per_conversation() {
    use serde_json::json;
    let workspace = tempfile::tempdir().unwrap();
    let sink = Arc::new(CollectingSink::default());
    let service = service_with_features(
        "http://127.0.0.1:1".into(),
        sink,
        workspace.path(),
        FeatureFlags {
            workbench_customization: true,
        },
    );
    let first = service.new_session().await.unwrap().session_id;
    let second = service.new_session().await.unwrap().session_id;
    let agent = service.agent_or_build(&first).await.unwrap();
    assert!(agent
        .lock()
        .await
        .tool_definitions()
        .iter()
        .any(|tool| tool["function"]["name"] == "develop_view"));
    let result = service
        .workbench_request(
            &first,
            "develop",
            json!({"action":"scaffold","source":"views/demo","id":"demo.view","title":"Demo"}),
        )
        .await
        .unwrap();
    assert!(result["candidate"]["digest"].is_string());
    assert!(workspace.path().join("views/demo/oxen-view.d.ts").is_file());
    service
        .workbench_request(
            &first,
            "develop",
            json!({
                "action":"install", "source":"views/demo", "digest":result["candidate"]["digest"]
            }),
        )
        .await
        .unwrap();
    let listed = service
        .workbench_request(&first, "list", json!({}))
        .await
        .unwrap();
    assert!(listed["views"]
        .as_array()
        .unwrap()
        .iter()
        .any(|view| view["id"] == "package:demo.view"));

    let release = service_for(
        "http://127.0.0.1:1".into(),
        Arc::new(CollectingSink::default()),
        workspace.path(),
    );
    let release_session = release.new_session().await.unwrap().session_id;
    let listed = release
        .workbench_request(&release_session, "list", json!({}))
        .await
        .unwrap();
    assert!(!listed["views"]
        .as_array()
        .unwrap()
        .iter()
        .any(|view| view["id"] == "package:demo.view"));
    assert!(release
        .workbench_request(
            &release_session,
            "open",
            json!({"view":"package:demo.view"})
        )
        .await
        .is_err());
    assert!(harness_runtime::view_packages::installed()
        .unwrap()
        .iter()
        .any(|package| package.manifest.id == "demo.view"));
    assert!(service
        .workbench_request(
            &first,
            "develop",
            json!({"action":"check","source":"../outside"})
        )
        .await
        .unwrap_err()
        .contains("invalid"));
    let a = service.workbench(&first).await.unwrap();
    let b = service.workbench(&second).await.unwrap();
    a.view_state("demo.view", Some(json!({"draft":"keep this"})))
        .await
        .unwrap();
    assert_eq!(
        a.view_state("demo.view", None).await.unwrap()["draft"],
        "keep this"
    );
    assert!(a.view_state("another.view", None).await.unwrap().is_null());
    assert!(b.view_state("demo.view", None).await.unwrap().is_null());
    a.gate.set_plan_mode(true);
    assert!(service
        .workbench_request(
            &first,
            "develop",
            json!({"action":"scaffold","source":"views/blocked","id":"blocked","title":"Blocked"})
        )
        .await
        .unwrap_err()
        .contains("plan"));
    assert!(!workspace.path().join("views/blocked").exists());
}

/// A scripted `run_shell` call that starts a background task.
const BACKGROUND_SHELL_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_bg\",\"type\":\"function\",\"function\":{\"name\":\"run_shell\",\"arguments\":\"{\\\"command\\\":\\\"sleep 1; echo render-finished\\\",\\\"is_background\\\":true}\"}}]}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":5,\"total_tokens\":105}}\n\n",
    "data: [DONE]\n\n"
);

const QUEUED_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Started — I'll show it the moment it lands.\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":200,\"completion_tokens\":10,\"total_tokens\":210}}\n\n",
    "data: [DONE]\n\n"
);

#[tokio::test]
async fn background_work_that_finishes_while_idle_is_announced_and_delivered() {
    let mut server = mockito::Server::new_async().await;
    let started = sse_mock(&mut server, BACKGROUND_SHELL_SSE);
    let queued = sse_mock(&mut server, QUEUED_SSE);

    let sink = Arc::new(CollectingSink::default());
    let workspace = tempfile::tempdir().unwrap();
    let service = service_for(server.url(), sink.clone(), workspace.path());
    let session = service.new_session().await.expect("new session").session_id;

    // Nothing pending yet: no turn runs.
    assert_eq!(service.deliver_pending(&session).await, Ok(None));

    let text = service
        .run_turn(&session, "render it in the background".into(), vec![])
        .await
        .expect("turn runs");
    assert!(text.contains("the moment it lands"), "{text}");
    started.assert_async().await;
    queued.assert_async().await;

    // The task outlives the turn, so nothing delivered it yet; its end is
    // announced so the idle client can act on it.
    assert!(!sink
        .events()
        .iter()
        .any(|e| { matches!(e, ProtocolEvent::Notice { kind, .. } if kind == "background_task") }));
    wait_for(&sink, || {
        sink.events()
            .iter()
            .any(|e| matches!(e, ProtocolEvent::DeliveryReady { session: s } if *s == session))
            .then_some(())
    })
    .await;

    let delivered = sse_mock(&mut server, FINAL_SSE);
    let reply = service
        .deliver_pending(&session)
        .await
        .expect("delivery turn");
    assert_eq!(reply.as_deref(), Some("The sum is 5."));
    delivered.assert_async().await;

    let transcript = service.session_messages(&session).unwrap();
    assert!(
        transcript
            .iter()
            .any(|m| m.to_string().contains("render-finished")),
        "the task's output reaches the model"
    );
    // Delivered once: a second call has nothing to do.
    assert_eq!(service.deliver_pending(&session).await, Ok(None));
}
