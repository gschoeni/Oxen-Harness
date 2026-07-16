//! The approval bridge in isolation: a gated tool call parks on the pending
//! map, the sink carries the request to the client, and the client's answer
//! resolves it into a `harness_permissions` decision.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness_host::{EventSink, HostApprover, PendingApprovals};
use harness_permissions::{ApprovalDecision, ApprovalKind, ApprovalRequest, CommandApprover, Risk};
use harness_protocol::{ApprovalAnswer, ProtocolEvent};

#[derive(Default)]
struct CollectingSink(Mutex<Vec<ProtocolEvent>>);

impl EventSink for CollectingSink {
    fn emit(&self, event: ProtocolEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn request() -> ApprovalRequest {
    ApprovalRequest {
        kind: ApprovalKind::Shell,
        tool: "run_shell".into(),
        command: "rm -rf build".into(),
        risk: Risk::Dangerous,
        reasons: vec!["deletes files".into()],
        grant_label: "commands starting with `rm`".into(),
        offer_project_grant: true,
        offer_trash: true,
    }
}

async fn wait_for_request(sink: &CollectingSink) -> String {
    for _ in 0..200 {
        let found = sink.0.lock().unwrap().iter().find_map(|e| match e {
            ProtocolEvent::ApprovalRequest { id, .. } => Some(id.clone()),
            _ => None,
        });
        if let Some(id) = found {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no approval request emitted");
}

#[tokio::test]
async fn approval_round_trip_resolves_the_decision() {
    let sink = Arc::new(CollectingSink::default());
    let pending = PendingApprovals::default();
    let approver = HostApprover {
        sink: sink.clone(),
        session: "s1".into(),
        pending: pending.clone(),
    };

    let task = tokio::spawn(async move { approver.approve(&request()).await });

    let id = wait_for_request(&sink).await;
    // The emitted request carries everything the approval card renders.
    let events = sink.0.lock().unwrap().clone();
    match &events[0] {
        ProtocolEvent::ApprovalRequest {
            session,
            kind,
            tool,
            command,
            risk,
            offer_trash,
            ..
        } => {
            assert_eq!(session, "s1");
            assert_eq!(*kind, harness_protocol::ApprovalKind::Shell);
            assert_eq!(tool, "run_shell");
            assert_eq!(command, "rm -rf build");
            assert_eq!(risk, "dangerous");
            assert!(*offer_trash);
        }
        other => panic!("unexpected event {other:?}"),
    }

    assert!(pending.deliver(
        &id,
        ApprovalAnswer {
            decision: "once".into(),
            message: None,
        },
    ));
    let decision = task.await.unwrap().expect("approver ran");
    assert_eq!(decision, Some(ApprovalDecision::AllowOnce));
}

#[tokio::test]
async fn deny_with_message_reaches_the_gate() {
    let sink = Arc::new(CollectingSink::default());
    let pending = PendingApprovals::default();
    let approver = HostApprover {
        sink: sink.clone(),
        session: "s1".into(),
        pending: pending.clone(),
    };

    let task = tokio::spawn(async move { approver.approve(&request()).await });
    let id = wait_for_request(&sink).await;
    pending.deliver(
        &id,
        ApprovalAnswer {
            decision: "deny".into(),
            message: Some("not on main".into()),
        },
    );
    let decision = task.await.unwrap().unwrap();
    assert_eq!(
        decision,
        Some(ApprovalDecision::DenyWithMessage("not on main".into()))
    );
}

#[tokio::test]
async fn dropped_answer_reads_as_no_interactive_user() {
    let sink = Arc::new(CollectingSink::default());
    let pending = PendingApprovals::default();
    let approver = HostApprover {
        sink: sink.clone(),
        session: "s1".into(),
        pending: pending.clone(),
    };

    let task = tokio::spawn(async move { approver.approve(&request()).await });
    let id = wait_for_request(&sink).await;
    // Simulate the chat being evicted / the client going away.
    pending.forget(&id);
    let decision = task.await.unwrap().unwrap();
    assert_eq!(decision, None);
}

/// Unknown ids are ignored — the request may have been cancelled already.
#[tokio::test]
async fn delivering_to_an_unknown_id_returns_false() {
    let pending = PendingApprovals::default();
    assert!(!pending.deliver(
        "missing",
        ApprovalAnswer {
            decision: "once".into(),
            message: None,
        },
    ));
}
