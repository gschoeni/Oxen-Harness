use std::sync::{Arc, Mutex};

use harness_agent::fleet::{run_fleet, FleetEvent, FleetLimits, FleetSink, LaneStop, SubagentTask};
use harness_agent::{Agent, AgentConfig, FleetSpawner, FleetTool};
use harness_llm::OxenClient;
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::{ToolRegistry, TypedTool};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn individually_cancelled_lane_is_partial() {
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let outcomes = run_fleet(
        |_: usize, cancel: CancellationToken| {
            cancel.cancel();
            let session = store.create_session(&SessionMeta::default()).unwrap();
            Agent::new(
                OxenClient::new("http://127.0.0.1:1", "k", "m"),
                ToolRegistry::new(),
                store.clone(),
                session,
                AgentConfig::default(),
            )
        },
        vec![SubagentTask::new("stopped", "go")],
        FleetLimits::with_concurrency(1),
        CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(outcomes[0].stopped, Some(LaneStop::Cancelled));
    assert!(!outcomes[0].ok());
}

fn git(root: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn committed_lane_edits_are_returned() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    git(repo.path(), &["config", "user.name", "Review"]);
    git(
        repo.path(),
        &["config", "user.email", "review@example.test"],
    );
    std::fs::write(repo.path().join("sample.txt"), "before\n").unwrap();
    git(repo.path(), &["add", "sample.txt"]);
    git(repo.path(), &["commit", "-qm", "initial"]);
    let lanes = harness_agent::worktree::create(repo.path(), "review-probe", 1).unwrap();
    std::fs::write(lanes[0].path().join("sample.txt"), "after\n").unwrap();
    git(lanes[0].path(), &["add", "sample.txt"]);
    git(lanes[0].path(), &["commit", "-qm", "implement task"]);
    assert!(harness_agent::worktree::changes(&lanes[0])
        .unwrap()
        .is_some());
}

#[derive(Default)]
struct QuietSink;
impl FleetSink for QuietSink {
    fn started(&self, _: &str, _: &[String], _: CancellationToken) {}
    fn event(&self, _: &str, _: &FleetEvent) {}
    fn finished(&self, _: &str) {}
}

#[tokio::test]
async fn construction_failure_cleans_live_registry() {
    let spawner = Arc::new(FleetSpawner::new(
        OxenClient::new("http://127.0.0.1:1", "k", "m"),
        ToolRegistry::new(),
        AgentConfig::default(),
    ));
    let tool = FleetTool::new(spawner.clone(), Arc::new(QuietSink));
    let result = tool
        .invoke(serde_json::json!({"agents": [
            {"name": "built", "prompt": "go"},
            {"name": "invalid-fork", "prompt": "go", "fork": true}
        ]}))
        .await;
    assert!(result.is_err());
    assert!(spawner.tree().live().is_empty());
}

struct CompletionSink {
    store: Arc<HistoryStore>,
    parent: String,
    observed: Mutex<Option<(usize, bool)>>,
    spawner: Arc<FleetSpawner>,
}
impl FleetSink for CompletionSink {
    fn started(&self, _: &str, _: &[String], _: CancellationToken) {}
    fn event(&self, _: &str, _: &FleetEvent) {}
    fn finished(&self, _: &str) {
        let lanes = self.store.lanes_of(&self.parent).unwrap();
        *self.observed.lock().unwrap() = Some((
            lanes.iter().filter(|lane| lane.record.is_some()).count(),
            self.spawner.tree().live().is_empty(),
        ));
    }
}

#[tokio::test]
async fn completion_is_emitted_after_records_are_ready() {
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let parent = store.create_session(&SessionMeta::default()).unwrap();
    let spawner = Arc::new(
        FleetSpawner::new(
            OxenClient::new("http://127.0.0.1:1", "k", "m"),
            ToolRegistry::new(),
            AgentConfig::default(),
        )
        .with_store(store.clone())
        .with_session(parent.clone()),
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    spawner.set_cancel(cancel);
    let sink = Arc::new(CompletionSink {
        store,
        parent,
        spawner: spawner.clone(),
        observed: Mutex::new(None),
    });
    let tool = FleetTool::new(spawner, sink.clone());
    tool.invoke(serde_json::json!({"agents": [{"name": "a", "prompt": "go"}]}))
        .await
        .unwrap();
    assert_eq!(*sink.observed.lock().unwrap(), Some((1, true)));
}

#[test]
fn isolated_snapshot_restores_committed_and_uncommitted_work_after_checkout_cleanup() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    git(repo.path(), &["config", "user.name", "Test"]);
    git(repo.path(), &["config", "user.email", "test@example.test"]);
    std::fs::create_dir(repo.path().join("app")).unwrap();
    std::fs::write(repo.path().join("app/base"), "initial\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-qm", "initial"]);
    std::fs::write(repo.path().join("app/base"), "user edit\n").unwrap();
    let lanes = harness_agent::worktree::create(&repo.path().join("app"), "snapshot", 1).unwrap();
    let checkout = lanes[0].path().to_path_buf();
    std::fs::write(checkout.join("base"), "committed agent edit\n").unwrap();
    git(&checkout, &["add", "."]);
    git(&checkout, &["commit", "-qm", "agent"]);
    std::fs::write(checkout.join("new file\nwith newline"), "unstaged\n").unwrap();
    let snapshot =
        harness_agent::worktree::WorktreeSnapshot::capture(&lanes[0], "durable").unwrap();
    assert!(snapshot.patch.contains("-user edit"));
    assert!(snapshot.patch.contains("+committed agent edit"));
    drop(lanes);
    assert!(!checkout.exists());
    git(repo.path(), &["reflog", "expire", "--expire=now", "--all"]);
    git(repo.path(), &["gc", "--prune=now"]);
    let restored = snapshot.restore().unwrap();
    assert_eq!(
        std::fs::read_to_string(restored.path().join("base")).unwrap(),
        "committed agent edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(restored.path().join("new file\nwith newline")).unwrap(),
        "unstaged\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("app/base")).unwrap(),
        "user edit\n"
    );
    assert_eq!(
        harness_agent::worktree::changes(&restored)
            .unwrap()
            .unwrap()
            .patch,
        snapshot.patch
    );
}

#[test]
fn request_admission_is_atomic_across_parallel_lanes() {
    let budget = Arc::new(harness_agent::TreeBudget::new(harness_agent::TreeLimits {
        max_requests: 3,
        max_tokens: 1000,
        max_spawns: 24,
    }));
    let barrier = Arc::new(std::sync::Barrier::new(16));
    let tasks: Vec<_> = (0..16)
        .map(|_| {
            let budget = budget.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                budget.reserve_request().is_ok()
            })
        })
        .collect();
    let admitted = tasks
        .into_iter()
        .map(|t| usize::from(t.join().unwrap()))
        .sum::<usize>();
    assert_eq!(admitted, 3);
    assert_eq!(budget.usage().requests, 3);
}
