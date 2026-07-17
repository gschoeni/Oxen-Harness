//! The wire contract, pinned as exact JSON. These tests are the protocol's
//! spec: a change that breaks one of them breaks every client (the desktop
//! webview, the HTTP SSE stream, third-party UIs), so shapes only change here
//! deliberately.

use harness_protocol::{
    ApprovalAnswer, ApprovalKind, Choice, FleetActivityKind, FleetAgentPhase, FleetSource,
    LocalPhase, PreviewPhase, ProtocolEvent, Question, QuestionAnswer, SessionInfo, ToolPhase,
    TurnRequest,
};

fn json(event: &ProtocolEvent) -> serde_json::Value {
    serde_json::to_value(event).expect("event serializes")
}

/// Serialize → deserialize must return the identical event, for every variant
/// a client can receive. (The SSE stream and any recorded event log depend on
/// this round-trip.)
fn round_trips(event: ProtocolEvent) {
    let value = json(&event);
    let back: ProtocolEvent = serde_json::from_value(value).expect("event deserializes");
    assert_eq!(back, event);
}

#[test]
fn token_event_wire_shape() {
    let event = ProtocolEvent::Token {
        session: "s1".into(),
        token: "hello".into(),
    };
    assert_eq!(
        json(&event),
        serde_json::json!({"type": "agent.token", "session": "s1", "token": "hello"})
    );
    round_trips(event);
}

#[test]
fn tool_event_wire_shape() {
    let event = ProtocolEvent::Tool {
        session: "s1".into(),
        phase: ToolPhase::Start,
        name: "run_shell".into(),
        detail: "{\"command\":\"ls\"}".into(),
    };
    let value = json(&event);
    assert_eq!(value["type"], "agent.tool");
    assert_eq!(value["phase"], "start");
    round_trips(event);

    let end = ProtocolEvent::Tool {
        session: "s1".into(),
        phase: ToolPhase::End,
        name: "run_shell".into(),
        detail: "ok".into(),
    };
    assert_eq!(json(&end)["phase"], "end");
}

#[test]
fn tool_delta_and_usage_wire_shapes() {
    let delta = ProtocolEvent::ToolDelta {
        session: "s1".into(),
        name: "canvas".into(),
        delta: "chunk".into(),
    };
    assert_eq!(json(&delta)["type"], "agent.tool_delta");
    round_trips(delta);

    let usage = ProtocolEvent::Usage {
        session: "s1".into(),
        tokens_used: 105,
        context_tokens: 90,
        context_window: 8192,
        prompt_tokens_used: 100,
        completion_tokens_used: 5,
    };
    let value = json(&usage);
    assert_eq!(value["type"], "agent.usage");
    assert_eq!(value["tokens_used"], 105);
    assert_eq!(value["context_window"], 8192);
    round_trips(usage);
}

#[test]
fn turn_lifecycle_wire_shapes() {
    let started = ProtocolEvent::TurnStarted {
        session: "s1".into(),
    };
    assert_eq!(
        json(&started),
        serde_json::json!({"type": "turn.started", "session": "s1"})
    );
    round_trips(started);

    let completed = ProtocolEvent::TurnCompleted {
        session: "s1".into(),
        text: "done".into(),
    };
    assert_eq!(json(&completed)["type"], "turn.completed");
    round_trips(completed);

    let failed = ProtocolEvent::TurnFailed {
        session: "s1".into(),
        error: "401 unauthorized".into(),
    };
    assert_eq!(json(&failed)["type"], "turn.failed");
    round_trips(failed);
}

#[test]
fn question_event_carries_session_and_claude_code_shape() {
    let event = ProtocolEvent::Question {
        session: "s1".into(),
        id: "q0".into(),
        questions: vec![Question {
            question: "Which DB?".into(),
            header: "Storage".into(),
            options: vec![Choice {
                label: "SQLite".into(),
                description: "file-backed".into(),
            }],
            multi_select: true,
        }],
    };
    let value = json(&event);
    assert_eq!(value["type"], "agent.question");
    assert_eq!(value["session"], "s1");
    // The camelCase rename matches harness-tools' model-facing schema.
    assert_eq!(value["questions"][0]["multiSelect"], true);
    round_trips(event);
}

/// `harness_tools::Question` and the protocol's `Question` must stay
/// serde-compatible: the host converts by construction, but a recorded JSON
/// payload from either side must parse as the other.
#[test]
fn question_matches_harness_tools_shape() {
    let tools_question = harness_tools::Question {
        question: "Which DB?".into(),
        header: "Storage".into(),
        options: vec![harness_tools::Choice {
            label: "SQLite".into(),
            description: "file-backed".into(),
        }],
        multi_select: true,
    };
    let value = serde_json::to_value(&tools_question).unwrap();
    let protocol: Question = serde_json::from_value(value.clone()).expect("shapes match");
    assert_eq!(serde_json::to_value(&protocol).unwrap(), value);

    let answer = QuestionAnswer {
        header: "Storage".into(),
        question: "Which DB?".into(),
        selected: vec!["SQLite".into()],
    };
    let tools_answer: harness_tools::QuestionAnswer =
        serde_json::from_value(serde_json::to_value(&answer).unwrap()).expect("shapes match");
    assert_eq!(tools_answer.selected, vec!["SQLite".to_string()]);
}

#[test]
fn approval_request_wire_shape() {
    let event = ProtocolEvent::ApprovalRequest {
        session: "s1".into(),
        id: "a0".into(),
        kind: ApprovalKind::Shell,
        tool: "run_shell".into(),
        command: "rm -rf build".into(),
        risk: "destructive".into(),
        reasons: vec!["deletes files".into()],
        grant_label: "commands starting with `rm`".into(),
        offer_project_grant: true,
        offer_trash: true,
    };
    let value = json(&event);
    assert_eq!(value["type"], "agent.approval_request");
    assert_eq!(value["kind"], "shell");
    assert_eq!(value["offer_trash"], true);
    round_trips(event);

    // The other gate kinds keep their snake_case labels.
    assert_eq!(
        serde_json::to_value(ApprovalKind::FileEdit).unwrap(),
        "file_edit"
    );
    assert_eq!(
        serde_json::to_value(ApprovalKind::GitCommit).unwrap(),
        "git_commit"
    );
}

#[test]
fn approval_answer_decision_keywords() {
    // The decision keywords the desktop already sends; the HTTP server reuses
    // them verbatim so one client vocabulary drives both hosts.
    let answer: ApprovalAnswer =
        serde_json::from_value(serde_json::json!({"decision": "once"})).unwrap();
    assert_eq!(answer.decision, "once");
    assert_eq!(answer.message, None);

    let deny: ApprovalAnswer =
        serde_json::from_value(serde_json::json!({"decision": "deny", "message": "not on main"}))
            .unwrap();
    assert_eq!(deny.message.as_deref(), Some("not on main"));
}

#[test]
fn fleet_event_wire_shapes() {
    let started = ProtocolEvent::FleetStarted {
        session: "s1".into(),
        agents: vec!["lane a".into(), "lane b".into()],
        source: FleetSource::Turn,
    };
    let value = json(&started);
    assert_eq!(value["type"], "fleet.started");
    assert_eq!(value["source"], "turn");
    round_trips(started);

    let agent = ProtocolEvent::FleetAgent {
        session: "s1".into(),
        agent: 1,
        name: "lane b".into(),
        phase: FleetAgentPhase::Done,
        tokens: 1200,
        summary: "finished".into(),
    };
    assert_eq!(json(&agent)["phase"], "done");
    round_trips(agent);

    let activity = ProtocolEvent::FleetActivity {
        session: "s1".into(),
        agent: 0,
        kind: FleetActivityKind::Token,
        text: "thinking".into(),
        tokens: None,
    };
    assert_eq!(json(&activity)["kind"], "token");
    round_trips(activity);

    round_trips(ProtocolEvent::FleetCompleted {
        session: "s1".into(),
    });
}

#[test]
fn remaining_variants_round_trip() {
    for event in [
        ProtocolEvent::Compacted {
            session: "s1".into(),
            detail: "trimmed 12 messages".into(),
        },
        ProtocolEvent::Retry {
            session: "s1".into(),
            attempt: 1,
            max_attempts: 3,
            delay_ms: 500,
            error: "connection reset".into(),
        },
        ProtocolEvent::Compression {
            session: "s1".into(),
            mode: "on".into(),
            saved_tokens: 100,
            total_saved_tokens: 400,
            results_compressed: 2,
        },
        ProtocolEvent::Approval {
            session: "s1".into(),
            phase: harness_protocol::ApprovalPhase::Resolved,
            name: "run_shell".into(),
            command: "ls".into(),
            decision: "allow once".into(),
        },
        ProtocolEvent::Canvas {
            session: "s1".into(),
            id: "c1".into(),
            title: "Doc".into(),
            format: "markdown".into(),
            language: None,
            content: "# hi".into(),
        },
        ProtocolEvent::CanvasWriting {
            session: "s1".into(),
        },
        ProtocolEvent::OpenFile {
            session: "s1".into(),
            paths: vec!["src/main.rs".into()],
        },
        ProtocolEvent::ReviewProgress {
            session: "s1".into(),
            step: "find".into(),
            index: 0,
            total: 3,
            agents: vec!["bugs".into()],
        },
        ProtocolEvent::ReviewToken {
            session: "s1".into(),
            token: "looking".into(),
        },
        ProtocolEvent::ReviewTool {
            session: "s1".into(),
            name: "read_file".into(),
        },
        ProtocolEvent::PreviewStatus {
            session: "s1".into(),
            phase: PreviewPhase::Ready,
            name: "dev".into(),
            command: "npm run dev".into(),
            url: Some("http://localhost:5173".into()),
            port: Some(5173),
            message: None,
        },
        ProtocolEvent::PreviewConsole {
            session: "s1".into(),
            text: "TypeError: x is undefined".into(),
        },
        ProtocolEvent::LocalStatus {
            model: "qwen".into(),
            phase: LocalPhase::Loading,
        },
        ProtocolEvent::DownloadProgress {
            id: "qwen".into(),
            downloaded: 10,
            total: Some(100),
            fraction: Some(0.1),
        },
    ] {
        round_trips(event);
    }
}

/// Every event maps onto the channel name the desktop webview already listens
/// on (`app/src/lib/agentEvents.ts`), so the Tauri adapter can emit protocol
/// events without the frontend changing.
#[test]
fn legacy_channel_names() {
    let cases: Vec<(ProtocolEvent, &str)> = vec![
        (
            ProtocolEvent::Token {
                session: "s".into(),
                token: "t".into(),
            },
            "agent://token",
        ),
        (
            ProtocolEvent::Tool {
                session: "s".into(),
                phase: ToolPhase::Start,
                name: "n".into(),
                detail: "d".into(),
            },
            "agent://tool",
        ),
        (
            ProtocolEvent::ToolDelta {
                session: "s".into(),
                name: "n".into(),
                delta: "d".into(),
            },
            "agent://tool-delta",
        ),
        (
            ProtocolEvent::Usage {
                session: "s".into(),
                tokens_used: 0,
                context_tokens: 0,
                context_window: 0,
                prompt_tokens_used: 0,
                completion_tokens_used: 0,
            },
            "agent://usage",
        ),
        (
            ProtocolEvent::Compacted {
                session: "s".into(),
                detail: "d".into(),
            },
            "agent://compacted",
        ),
        (
            ProtocolEvent::Retry {
                session: "s".into(),
                attempt: 0,
                max_attempts: 0,
                delay_ms: 0,
                error: "e".into(),
            },
            "agent://retry",
        ),
        (
            ProtocolEvent::Compression {
                session: "s".into(),
                mode: "on".into(),
                saved_tokens: 0,
                total_saved_tokens: 0,
                results_compressed: 0,
            },
            "agent://compression",
        ),
        (
            ProtocolEvent::Question {
                session: "s".into(),
                id: "q".into(),
                questions: vec![],
            },
            "agent://question",
        ),
        (
            ProtocolEvent::Canvas {
                session: "s".into(),
                id: "c".into(),
                title: "t".into(),
                format: "markdown".into(),
                language: None,
                content: "".into(),
            },
            "agent://canvas",
        ),
        (
            ProtocolEvent::CanvasWriting {
                session: "s".into(),
            },
            "agent://canvas-writing",
        ),
        (
            ProtocolEvent::OpenFile {
                session: "s".into(),
                paths: vec![],
            },
            "agent://open-file",
        ),
        (
            ProtocolEvent::ApprovalRequest {
                session: "s".into(),
                id: "a".into(),
                kind: ApprovalKind::Shell,
                tool: "t".into(),
                command: "c".into(),
                risk: "r".into(),
                reasons: vec![],
                grant_label: "g".into(),
                offer_project_grant: false,
                offer_trash: false,
            },
            "agent://approval-request",
        ),
        (
            ProtocolEvent::Approval {
                session: "s".into(),
                phase: harness_protocol::ApprovalPhase::Pending,
                name: "n".into(),
                command: "c".into(),
                decision: "".into(),
            },
            "agent://approval",
        ),
        (
            ProtocolEvent::TurnStarted {
                session: "s".into(),
            },
            "turn://started",
        ),
        (
            ProtocolEvent::TurnCompleted {
                session: "s".into(),
                text: "t".into(),
            },
            "turn://completed",
        ),
        (
            ProtocolEvent::TurnFailed {
                session: "s".into(),
                error: "e".into(),
            },
            "turn://failed",
        ),
        (
            ProtocolEvent::FleetStarted {
                session: "s".into(),
                agents: vec![],
                source: FleetSource::Review,
            },
            "fleet://started",
        ),
        (
            ProtocolEvent::FleetAgent {
                session: "s".into(),
                agent: 0,
                name: "n".into(),
                phase: FleetAgentPhase::Started,
                tokens: 0,
                summary: "".into(),
            },
            "fleet://agent",
        ),
        (
            ProtocolEvent::FleetActivity {
                session: "s".into(),
                agent: 0,
                kind: FleetActivityKind::Tool,
                text: "".into(),
                tokens: None,
            },
            "fleet://agent-activity",
        ),
        (
            ProtocolEvent::FleetCompleted {
                session: "s".into(),
            },
            "fleet://completed",
        ),
        (
            ProtocolEvent::ReviewProgress {
                session: "s".into(),
                step: "s".into(),
                index: 0,
                total: 0,
                agents: vec![],
            },
            "review://progress",
        ),
        (
            ProtocolEvent::ReviewToken {
                session: "s".into(),
                token: "t".into(),
            },
            "review://token",
        ),
        (
            ProtocolEvent::ReviewTool {
                session: "s".into(),
                name: "n".into(),
            },
            "review://tool",
        ),
        (
            ProtocolEvent::PreviewStatus {
                session: "s".into(),
                phase: PreviewPhase::Starting,
                name: "n".into(),
                command: "c".into(),
                url: None,
                port: None,
                message: None,
            },
            "preview://status",
        ),
        (
            ProtocolEvent::PreviewConsole {
                session: "s".into(),
                text: "t".into(),
            },
            "preview://console",
        ),
        (
            ProtocolEvent::LocalStatus {
                model: "m".into(),
                phase: LocalPhase::Ready,
            },
            "local://status",
        ),
        (
            ProtocolEvent::DownloadProgress {
                id: "m".into(),
                downloaded: 0,
                total: None,
                fraction: None,
            },
            "models://progress",
        ),
    ];
    for (event, channel) in cases {
        assert_eq!(event.channel(), channel, "channel for {event:?}");
    }
}

/// `session()` exposes the routing key for every session-scoped event, and
/// `None` for the app-wide ones (local model status, download progress).
#[test]
fn session_accessor() {
    let scoped = ProtocolEvent::Token {
        session: "s9".into(),
        token: "t".into(),
    };
    assert_eq!(scoped.session(), Some("s9"));

    let global = ProtocolEvent::LocalStatus {
        model: "m".into(),
        phase: LocalPhase::Starting,
    };
    assert_eq!(global.session(), None);
}

#[test]
fn dto_wire_shapes() {
    let info = SessionInfo {
        model: "claude-opus-4-8".into(),
        workspace: "/tmp/proj".into(),
        session_id: "s1".into(),
        tokens_used: 100,
        context_tokens: 50,
        context_window: 200_000,
        compression_mode: "off".into(),
    };
    let value = serde_json::to_value(&info).unwrap();
    assert_eq!(value["session_id"], "s1");
    let back: SessionInfo = serde_json::from_value(value).unwrap();
    assert_eq!(back, info);

    let request: TurnRequest = serde_json::from_value(serde_json::json!({
        "prompt": "hello",
    }))
    .unwrap();
    assert_eq!(request.prompt, "hello");
    assert!(request.attachments.is_empty());
}

#[test]
fn review_and_loop_result_wire_shapes() {
    let review: harness_protocol::ReviewResult = serde_json::from_value(serde_json::json!({
        "status": "ok",
        "user": "review my diff",
        "assistant": "Found 2 issues…",
        "findings": 2,
        "tokens_used": 1200,
    }))
    .unwrap();
    assert_eq!(review.status, "ok");
    assert_eq!(review.findings, 2);

    let outcome: harness_protocol::LoopResult = serde_json::from_value(serde_json::json!({
        "succeeded": true,
        "iterations": 3,
        "summary": "Loop complete: all gates passed after 3 iteration(s).",
    }))
    .unwrap();
    assert!(outcome.succeeded);
    assert_eq!(outcome.iterations, 3);
}

/// The protocol self-describes: JSON Schema generation must work for the event
/// enum and DTOs (this is what the TS type generation consumes).
#[test]
fn json_schema_generates() {
    let schema = schemars::schema_for!(ProtocolEvent);
    let text = serde_json::to_string(&schema).unwrap();
    assert!(text.contains("agent.token"));
    assert!(schemars::schema_for!(SessionInfo).as_value().is_object());
}
