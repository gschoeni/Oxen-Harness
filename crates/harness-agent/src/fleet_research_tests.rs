//! Research lane regression coverage, including real tool dispatch against a mock provider.

use super::*;
use crate::lane_profile::RESEARCH_TOOLS;
use crate::test_support::{sse_prose, sse_tool_call};

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("shared.txt"), "original\n").unwrap();
    dir
}

fn spawner(root: &std::path::Path, url: &str) -> FleetSpawner {
    FleetSpawner::new(
        OxenClient::new(url, "k", "m"),
        ToolRegistry::default_for_workspace(harness_tools::Workspace::new(root).unwrap()),
        AgentConfig::default(),
    )
    .with_workspace(root)
}

#[test]
fn research_profile_is_opt_in_and_rejects_unknown_values() {
    let args: FleetArgs = serde_json::from_value(serde_json::json!({
        "agents": [{"name": "legacy", "prompt": "investigate"}]
    }))
    .unwrap();
    assert_eq!(args.agents[0].profile, LaneProfile::Full);
    assert!(serde_json::from_value::<FleetArgs>(serde_json::json!({
        "agents": [{"name": "bad", "prompt": "investigate", "profile": "unknown"}]
    }))
    .is_err());
}

#[test]
fn research_profile_survives_cold_resume_and_fork() {
    let dir = project();
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let parent = store.create_session(&SessionMeta::default()).unwrap();
    let make = || {
        spawner(dir.path(), "http://127.0.0.1:1")
            .with_store(store.clone())
            .with_session(parent.clone())
    };
    let sp = make();
    *sp.fork_slot().lock().unwrap() = Some(Arc::new(vec![
        ChatMessage::system("parent"),
        ChatMessage::user("prior investigation"),
    ]));
    for fork in [false, true] {
        let lane = sp
            .build_agent_with(
                "research",
                "fleet",
                None,
                CancellationToken::new(),
                LaneOptions {
                    fork,
                    profile: LaneProfile::Research,
                },
            )
            .unwrap();
        let names: Vec<String> = lane
            .tool_definitions()
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap().to_string())
            .collect();
        for required in [
            "read_file",
            "find_files",
            "search_files",
            "web_search",
            "web_fetch",
            "retrieve_original",
        ] {
            assert!(names.iter().any(|n| n == required), "{required}: {names:?}");
        }
        assert!(
            names.iter().all(|n| RESEARCH_TOOLS.contains(&n.as_str())),
            "{names:?}"
        );
        assert!(lane
            .config()
            .system_prompt
            .as_ref()
            .unwrap()
            .contains("research profile"));
        if fork {
            assert!(lane
                .messages()
                .iter()
                .any(|m| m.content_text().as_deref() == Some("prior investigation")));
        }
        let id = lane.session_id().to_string();
        drop(lane);
        let resumed = make()
            .resume_lane(&id, "research", "followup", CancellationToken::new())
            .unwrap();
        assert_eq!(
            names,
            resumed
                .tool_definitions()
                .iter()
                .map(|d| d["function"]["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        );
        drop(resumed);
        store
            .save_session_state(&id, LANE_PROFILE_STATE, &"invalid-profile")
            .unwrap();
        assert!(make()
            .resume_lane(&id, "research", "followup", CancellationToken::new())
            .is_err());
    }
}

#[test]
fn research_profile_materially_reduces_request_overhead() {
    let dir = project();
    let sp = spawner(dir.path(), "http://127.0.0.1:1");
    let full = sp
        .build_agent("full", "fleet", None, CancellationToken::new())
        .unwrap();
    let research = sp
        .build_agent_with(
            "research",
            "fleet",
            None,
            CancellationToken::new(),
            LaneOptions {
                profile: LaneProfile::Research,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(full
        .tool_definitions()
        .iter()
        .any(|d| d["function"]["name"] == "write_file"));
    let full_tokens =
        crate::budget::estimate_prompt_tokens(full.messages(), &full.tool_definitions());
    let research_tokens =
        crate::budget::estimate_prompt_tokens(research.messages(), &research.tool_definitions());
    println!("estimated fixed request overhead: full={full_tokens}, research={research_tokens}");
    assert!(
        research_tokens * 100 < full_tokens * 70,
        "full={full_tokens}, research={research_tokens}"
    );
}

#[tokio::test]
async fn research_lane_reads_but_rejects_mutation_calls_including_followups() {
    let mut server = mockito::Server::new_async().await;
    let dir = project();
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let parent = store.create_session(&SessionMeta::default()).unwrap();
    let url = server.url();
    let make = || {
        Arc::new(
            spawner(dir.path(), &url)
                .with_store(store.clone())
                .with_session(parent.clone()),
        )
    };
    let sp = make();
    let read = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::Regex("inspect shared.txt".into()))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse_tool_call(
            "read",
            "read_file",
            serde_json::json!({"path": "shared.txt"}),
        ))
        .expect(1)
        .create_async()
        .await;
    let answer = server
        .mock("POST", "/chat/completions")
        .match_body(mockito::Matcher::Regex("original".into()))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse_prose("Found original content"))
        .expect(1)
        .create_async()
        .await;
    let tool = FleetTool::new(sp.clone(), Arc::new(QuietFleetSink));
    let result = tool.invoke(serde_json::json!({"agents": [{"name": "reader", "prompt": "inspect shared.txt", "profile": "research"}]})).await.unwrap();
    assert!(result.contains("Found original content"), "{result}");
    read.assert_async().await;
    answer.assert_async().await;
    read.remove_async().await;
    answer.remove_async().await;
    let id = store.lanes_of(&parent).unwrap()[0].id.clone();
    drop(tool);
    drop(sp);
    for name in [
        "write_file",
        "edit_file",
        "run_shell",
        "spawn_agents",
        "send_to_agent",
        "generate_image",
    ] {
        let attempt = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_tool_call(
                "blocked",
                name,
                serde_json::json!({"path": "shared.txt", "contents": "changed"}),
            ))
            .expect(1)
            .create_async()
            .await;
        let finish = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex(format!("[Uu]nknown tool.*{name}")))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("Blocked mutation"))
            .expect(1)
            .create_async()
            .await;
        let result = make()
            .follow_up(&id, "try the requested operation")
            .await
            .unwrap();
        assert!(result.contains("Blocked mutation"), "{result}");
        attempt.assert_async().await;
        finish.assert_async().await;
        attempt.remove_async().await;
        finish.remove_async().await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("shared.txt")).unwrap(),
            "original\n"
        );
    }
}

#[tokio::test]
async fn research_filter_preserves_parent_preferences_and_metadata() {
    let dir = project();
    let mut tools =
        ToolRegistry::default_for_workspace(harness_tools::Workspace::new(dir.path()).unwrap());
    tools.remove("web_search");
    tools.set_description_override("read_file", "project-specific reader");
    let parent_defs = tools.definitions();
    let mut research = LaneProfile::Research.filter(tools.clone());
    assert!(research.get("web_search").is_none());
    assert!(research.get("write_file").is_none());
    assert_eq!(
        research.description_override("read_file"),
        Some("project-specific reader")
    );
    assert!(Arc::ptr_eq(
        research.files().unwrap(),
        tools.files().unwrap()
    ));
    assert!(Arc::ptr_eq(
        research.overflow_store().unwrap(),
        tools.overflow_store().unwrap()
    ));
    assert_eq!(
        research.workspace().unwrap().root(),
        tools.workspace().unwrap().root()
    );
    assert!(matches!(
        research
            .invoke("write_file", serde_json::json!({}), &CallContext::default())
            .await,
        Err(ToolError::UnknownTool(_))
    ));
    research.remove("read_file");
    assert!(tools.get("read_file").is_some());
    assert!(tools.get("write_file").is_some());
    assert_eq!(parent_defs, tools.definitions());
}

#[test]
fn research_profile_is_enforced_on_direct_restore_and_session_switch() {
    let dir = project();
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let parent = store.create_session(&SessionMeta::default()).unwrap();
    let sp = spawner(dir.path(), "http://127.0.0.1:1")
        .with_store(store.clone())
        .with_session(parent.clone());
    let lane = sp
        .build_agent_with(
            "reader",
            "fleet",
            None,
            CancellationToken::new(),
            LaneOptions {
                profile: LaneProfile::Research,
                ..Default::default()
            },
        )
        .unwrap();
    let id = lane.session_id().to_string();
    drop(lane);
    let tools =
        || ToolRegistry::default_for_workspace(harness_tools::Workspace::new(dir.path()).unwrap());
    let client = || OxenClient::new("http://127.0.0.1:1", "k", "m");
    let mut restored = Agent::resume_from_store(
        client(),
        tools(),
        store.clone(),
        id.clone(),
        AgentConfig::default(),
    )
    .unwrap();
    assert!(restored
        .tool_definitions()
        .iter()
        .all(|d| RESEARCH_TOOLS.contains(&d["function"]["name"].as_str().unwrap())));
    restored.set_compression_mode(harness_compress::CompressionMode::On);
    assert!(restored
        .tool_definitions()
        .iter()
        .all(|d| RESEARCH_TOOLS.contains(&d["function"]["name"].as_str().unwrap())));
    let mut full = Agent::resume_from_store(
        client(),
        tools(),
        store.clone(),
        parent.clone(),
        AgentConfig::default(),
    )
    .unwrap();
    full.load_session(id.clone()).unwrap();
    assert!(full
        .tool_definitions()
        .iter()
        .all(|d| RESEARCH_TOOLS.contains(&d["function"]["name"].as_str().unwrap())));
    full.set_compression_mode(harness_compress::CompressionMode::On);
    full.load_session(parent).unwrap();
    assert!(full
        .tool_definitions()
        .iter()
        .any(|d| d["function"]["name"] == "write_file"));
    assert!(full
        .tool_definitions()
        .iter()
        .any(|d| d["function"]["name"] == "retrieve_original"));
    full.load_session(id).unwrap();
    full.start_new_session(&SessionMeta::default()).unwrap();
    assert!(full
        .tool_definitions()
        .iter()
        .any(|d| d["function"]["name"] == "write_file"));
}

#[test]
fn research_prompt_strips_delegation_and_is_idempotent_on_resume() {
    let dir = project();
    let store = Arc::new(HistoryStore::open_in_memory().unwrap());
    let parent = store.create_session(&SessionMeta::default()).unwrap();
    let make = || {
        FleetSpawner::new(
            OxenClient::new("http://127.0.0.1:1", "k", "m"),
            ToolRegistry::default_for_workspace(harness_tools::Workspace::new(dir.path()).unwrap()),
            AgentConfig {
                system_prompt: Some(crate::prompt::system_prompt_with(
                    crate::prompt::OptionalTools {
                        agents: true,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        )
        .with_store(store.clone())
        .with_session(parent.clone())
    };
    let sp = make();
    let lane = sp
        .build_agent_with(
            "reader",
            "fleet",
            None,
            CancellationToken::new(),
            LaneOptions {
                profile: LaneProfile::Research,
                ..Default::default()
            },
        )
        .unwrap();
    let check = |lane: &Agent| {
        let prompt = lane.config().system_prompt.as_ref().unwrap();
        assert!(!prompt.contains("`spawn_agents` / `map_agents` / `ask_model`"));
        assert!(!prompt.contains(crate::prompt::DELEGATION_GUIDELINE));
        assert_eq!(prompt.matches("## Research profile").count(), 1);
        let mut reapplied = Some(prompt.clone());
        LaneProfile::Research.apply_prompt(&mut reapplied);
        assert_eq!(reapplied.as_ref(), Some(prompt));
    };
    check(&lane);
    let id = lane.session_id().to_string();
    drop(lane);
    let resumed = make()
        .resume_lane(&id, "reader", "followup", CancellationToken::new())
        .unwrap();
    check(&resumed);
    assert_eq!(
        resumed.messages()[0].content_text().as_ref(),
        resumed.config().system_prompt.as_ref()
    );
}
