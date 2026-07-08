//! End-to-end pipeline test: two steps against a mock model endpoint, proving
//! each step runs on an isolated side agent, step 1's reply reaches step 2 via
//! `{{previous}}`, the session transcript stays untouched, and the final reply
//! parses into findings.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use harness_agent::{Agent, AgentConfig};
use harness_llm::OxenClient;
use harness_review::{
    ReviewConfig, ReviewEvent, ReviewRunner, ReviewStep, ReviewTarget, StepAgent,
};
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::ToolRegistry;

/// SSE body for a plain prose reply.
fn sse_prose(text: &str) -> String {
    let chunk = serde_json::json!({
        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": "stop" }]
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

/// A committed git repo with one pending edit; skips the test when git is
/// unavailable in the environment.
fn dirty_repo() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|_| ())
    };
    run(&["init", "-q"])?;
    run(&["config", "user.email", "t@t"])?;
    run(&["config", "user.name", "t"])?;
    std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
    run(&["add", "."])?;
    run(&["commit", "-q", "-m", "init"])?;
    std::fs::write(root.join("a.rs"), "fn main() { edited(); }\n").unwrap();
    Some((dir, root))
}

fn agent(url: &str, workspace: &Path) -> (Agent, Arc<HistoryStore>, String) {
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let session = store
        .create_session(&SessionMeta {
            workspace: workspace.display().to_string(),
            model: "claude-opus-4-8".into(),
            ..Default::default()
        })
        .unwrap();
    let client = OxenClient::new(url, "key", "claude-opus-4-8");
    let agent = Agent::new(
        client,
        ToolRegistry::new(),
        store.clone(),
        session.clone(),
        AgentConfig::default(),
    )
    .unwrap();
    (agent, store, session)
}

#[tokio::test]
async fn two_steps_chain_previous_output_and_yield_parsed_findings() {
    let Some((_dir, root)) = dirty_repo() else {
        return;
    };
    let mut server = mockito::Server::new_async().await;

    // Step 2's request embeds step 1's reply via {{previous}} — match on that
    // marker so the mocks can't be served out of order.
    let report_json = r#"{"findings":[{"title":"Fix edited()","file":"a.rs","line":1,"priority":1,"verdict":"CONFIRMED","body":"edited() is undefined.","failure_scenario":"any run → compile error"}],"overall_correctness":"incorrect","overall_explanation":"One confirmed bug."}"#;
    let step2 = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::Regex("CANDIDATE-MARKER-7".into()))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse_prose(report_json))
        .expect(1)
        .create_async()
        .await;
    let step1 = server
        .mock("POST", "/chat/completions")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse_prose("CANDIDATE-MARKER-7: a.rs:1 looks wrong"))
        .expect(1)
        .create_async()
        .await;

    let config = ReviewConfig {
        steps: vec![
            ReviewStep {
                name: "find".into(),
                prompt: "Find problems in:\n{{diff}}".into(),
                agents: Vec::new(),
            },
            ReviewStep {
                name: "report".into(),
                prompt: "Report on these candidates:\n{{previous}}".into(),
                agents: Vec::new(),
            },
        ],
        max_findings: 5,
        max_parallel: 2,
    };

    let (agent, store, session) = agent(&server.url(), &root);
    let before = store.messages(&session).unwrap().len();

    let mut step_names = Vec::new();
    let report = ReviewRunner::new(config, ReviewTarget::Uncommitted, &root)
        .run(&agent, |event| {
            if let ReviewEvent::StepStarted { name, .. } = event {
                step_names.push(name.clone());
            }
        })
        .await
        .unwrap();

    step1.assert_async().await;
    step2.assert_async().await;
    assert_eq!(step_names, ["find", "report"]);
    assert!(report.parsed);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].location(), "a.rs:1");

    // The pipeline ran entirely on side agents: the session is untouched.
    assert_eq!(store.messages(&session).unwrap().len(), before);
}

#[tokio::test]
async fn a_fan_out_step_runs_agents_in_parallel_and_combines_their_outputs() {
    let Some((_dir, root)) = dirty_repo() else {
        return;
    };
    let mut server = mockito::Server::new_async().await;

    // Two parallel finders, disambiguated by markers in their prompts; the
    // report step's request must carry BOTH lanes' replies via {{previous}}.
    let left = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::Regex("LANE-LEFT".into()))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse_prose("LEFT-CANDIDATE at a.rs:1"))
        .expect(1)
        .create_async()
        .await;
    let right = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::Regex("LANE-RIGHT".into()))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse_prose("RIGHT-CANDIDATE at a.rs:1"))
        .expect(1)
        .create_async()
        .await;
    let report_mock = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::AllOf(vec![
            mockito::Matcher::Regex("LEFT-CANDIDATE".into()),
            mockito::Matcher::Regex("RIGHT-CANDIDATE".into()),
            // Lanes arrive labeled, in agent order, under ### headings.
            mockito::Matcher::Regex("### left[\\s\\S]*### right".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse_prose(
            r#"{"findings":[],"overall_correctness":"correct"}"#,
        ))
        .expect(1)
        .create_async()
        .await;

    let config = ReviewConfig {
        steps: vec![
            ReviewStep {
                name: "find".into(),
                prompt: String::new(),
                agents: vec![
                    StepAgent {
                        name: "left".into(),
                        prompt: "LANE-LEFT: inspect\n{{diff}}".into(),
                    },
                    StepAgent {
                        name: "right".into(),
                        prompt: "LANE-RIGHT: inspect\n{{diff}}".into(),
                    },
                ],
            },
            ReviewStep {
                name: "report".into(),
                prompt: "Combine:\n{{previous}}".into(),
                agents: Vec::new(),
            },
        ],
        max_findings: 5,
        max_parallel: 2,
    };

    let (agent, _store, _session) = agent(&server.url(), &root);
    let mut lanes_started = Vec::new();
    let mut lanes_completed = Vec::new();
    let mut fan_out_step_agents = Vec::new();
    let mut total_tokens = 0usize;
    let report = ReviewRunner::new(config, ReviewTarget::Uncommitted, &root)
        .run(&agent, |event| match event {
            ReviewEvent::StepStarted { agents, name, .. } if name == "find" => {
                fan_out_step_agents = agents.clone();
            }
            ReviewEvent::SubagentStarted { name, .. } => lanes_started.push(name.clone()),
            ReviewEvent::SubagentCompleted { name, ok, .. } => {
                lanes_completed.push((name.clone(), *ok))
            }
            ReviewEvent::Completed { tokens_used, .. } => total_tokens = *tokens_used,
            _ => {}
        })
        .await
        .unwrap();

    left.assert_async().await;
    right.assert_async().await;
    report_mock.assert_async().await;

    assert_eq!(fan_out_step_agents, ["left", "right"]);
    assert_eq!(lanes_started.len(), 2);
    assert_eq!(lanes_completed.len(), 2);
    assert!(lanes_completed.iter().all(|(_, ok)| *ok));
    assert!(report.parsed);
    assert!(total_tokens > 0, "fleet tokens roll into the review total");
}
