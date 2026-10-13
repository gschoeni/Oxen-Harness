//! New chats — and the next launch — start on the model the user last used,
//! and a local model is only ever remembered by name: nothing loads until a
//! chat on it is sent to.
//!
//! Its own test binary: the scenario reads and writes `models.json` in a
//! config home no other test shares, and it runs as one sequence because
//! that home (like `LLAMA_SERVER`) is process-wide.

use std::sync::{Arc, Mutex};

use harness_host::{EventSink, SessionService};
use harness_llm::OxenClient;
use harness_protocol::ProtocolEvent;
use harness_store::HistoryStore;

#[derive(Default)]
struct NullSink(Mutex<Vec<ProtocolEvent>>);

impl EventSink for NullSink {
    fn emit(&self, event: ProtocolEvent) {
        self.0.lock().unwrap().push(event);
    }
}

const FINAL_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":1,\"total_tokens\":11}}\n\n",
    "data: [DONE]\n\n"
);

/// A service as a host builds it at launch: no model passed in, so it starts
/// on whatever `models.json` remembers.
fn launch(url: &str, store: Arc<HistoryStore>, workspace: &std::path::Path) -> SessionService {
    let url = url.to_string();
    SessionService::builder(Arc::new(NullSink::default()))
        .store(store)
        .active_project(workspace)
        .client_factory(move |model| Ok(OxenClient::new(url.clone(), "sk-test", model)))
        .build()
}

async fn serving(service: &SessionService) -> Option<String> {
    let guard = service.local_server.lock().await;
    guard.as_ref().map(|s| s.model_id().to_string())
}

#[tokio::test]
async fn new_chats_and_the_next_launch_start_on_the_last_model_used() {
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("OXEN_HARNESS_DIR", home.path());
    // A runtime and a downloaded model, as far as discovery can tell: the
    // runtime is never started here, which is the point.
    let runtime = home.path().join("llama-server");
    std::fs::write(&runtime, "").unwrap();
    std::env::set_var("LLAMA_SERVER", &runtime);
    let models = harness_config::paths::models_dir().unwrap();
    std::fs::create_dir_all(&models).unwrap();
    std::fs::write(models.join("tiny-local.gguf"), b"GGUF").unwrap();

    let mut server = mockito::Server::new_async().await;
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let service = launch(&server.url(), store.clone(), workspace.path());
    let chat = service.new_session().await.unwrap().session_id;

    // Sending a message is using the model: a chat recorded on another cloud
    // model makes that model the default once it is sent to.
    let other = store
        .create_session(&harness_store::SessionMeta {
            workspace: workspace.path().display().to_string(),
            model: "model-sent-to".into(),
            provider: "oxen".into(),
            base_url: server.url(),
            mode: harness_store::MODE_CLOUD.into(),
            ..Default::default()
        })
        .unwrap();
    let reply = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(FINAL_SSE)
        .create();
    service.resume_session(&other).await.unwrap();
    service.run_turn(&other, "hi".into(), vec![]).await.unwrap();
    reply.assert();
    assert_eq!(service.new_session().await.unwrap().model, "model-sent-to");
    assert_eq!(harness_runtime::models::selected(), "model-sent-to");

    // Picking a cloud model in a chat's picker does the same.
    service
        .set_session_model(&chat, "picked-cloud")
        .await
        .unwrap();
    assert_eq!(service.new_session().await.unwrap().model, "picked-cloud");

    // Picking a local model moves the chat and the default onto it — and
    // loads nothing.
    let moved = service
        .set_session_local_model(&chat, "tiny-local")
        .await
        .unwrap();
    assert_eq!(moved.model, "tiny-local");
    assert_eq!(serving(&service).await, None);
    let fresh = service.new_session().await.unwrap();
    assert_eq!(fresh.model, "tiny-local");
    assert_eq!(
        serving(&service).await,
        None,
        "opening a chat loads no model"
    );
    assert_eq!(
        harness_runtime::models::active_local().as_deref(),
        Some("tiny-local")
    );

    // Relaunch: the first chat is on the local model, still not loaded.
    let relaunched = launch(
        &server.url(),
        Arc::new(HistoryStore::open_in_memory().unwrap()),
        workspace.path(),
    );
    let first = relaunched.new_session().await.unwrap();
    assert_eq!(first.model, "tiny-local");
    assert_eq!(serving(&relaunched).await, None, "launching loads no model");

    // Back to cloud: the local model is forgotten, here and on disk.
    relaunched
        .set_session_model(&first.session_id, "back-to-cloud")
        .await
        .unwrap();
    assert_eq!(
        relaunched.new_session().await.unwrap().model,
        "back-to-cloud"
    );
    assert_eq!(harness_runtime::models::active_local(), None);
    let again = launch(
        &server.url(),
        Arc::new(HistoryStore::open_in_memory().unwrap()),
        workspace.path(),
    );
    assert_eq!(again.new_session().await.unwrap().model, "back-to-cloud");

    // A remembered local model deleted since is dropped for the cloud one.
    harness_runtime::models::set_active_local("tiny-local").unwrap();
    std::fs::remove_file(models.join("tiny-local.gguf")).unwrap();
    let after_delete = launch(
        &server.url(),
        Arc::new(HistoryStore::open_in_memory().unwrap()),
        workspace.path(),
    );
    assert_eq!(
        after_delete.new_session().await.unwrap().model,
        "back-to-cloud"
    );
}
