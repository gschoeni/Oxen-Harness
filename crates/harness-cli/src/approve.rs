//! Terminal approver for the permission gate.
//!
//! Bridges [`harness_permissions::CommandApprover`] to the interactive
//! [`crate::picker`], exactly the way [`crate::ask`] bridges the question
//! tool: the blocking picker runs on a `spawn_blocking` thread, a non-TTY
//! session returns `None` (the gate then declines rather than hanging), and
//! typing free text into the picker denies with the user's words sent to the
//! model — "no, because…" costs one line.

use std::io::IsTerminal;

use async_trait::async_trait;
use harness_permissions::{ApprovalDecision, ApprovalKind, ApprovalRequest, CommandApprover};

use crate::picker::{self, Choice};
use crate::theme::Ui;

const RUN_ONCE: &str = "Run once";
const ALLOW_SESSION: &str = "Allow for this session";
const ALLOW_PROJECT: &str = "Allow for this project";
const MOVE_TO_TRASH: &str = "Move to trash instead";
const DENY: &str = "Deny";

/// Asks for command approval through the terminal picker.
pub struct CliApprover {
    ui: Ui,
}

impl CliApprover {
    pub fn new(ui: Ui) -> Self {
        Self { ui }
    }
}

#[async_trait]
impl CommandApprover for CliApprover {
    async fn approve(
        &self,
        request: &ApprovalRequest,
    ) -> Result<Option<ApprovalDecision>, String> {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return Ok(None);
        }
        let ui = self.ui.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || prompt(&ui, &request))
            .await
            .map_err(|e| format!("approval prompt panicked: {e}"))?
    }
}

fn prompt(ui: &Ui, request: &ApprovalRequest) -> Result<Option<ApprovalDecision>, String> {
    let question = question_text(request);
    let mut options = vec![
        Choice::new(RUN_ONCE, "Execute this one time; ask again next time"),
        Choice::new(
            ALLOW_SESSION,
            format!("Don't ask again this session for {}", request.grant_label),
        ),
    ];
    if request.offer_project_grant {
        options.push(Choice::new(
            ALLOW_PROJECT,
            format!(
                "Don't ask again in this project for {} (saved to .oxen-harness/permissions.json)",
                request.grant_label
            ),
        ));
    }
    if request.offer_trash {
        options.push(Choice::new(
            MOVE_TO_TRASH,
            "Relocate the files into ~/.oxen-harness/trash (kept 7 days) instead of deleting",
        ));
    }
    options.push(Choice::new(
        DENY,
        "Don't run it — or type your own reason to send back to the model",
    ));

    let selected = picker::select(ui, "approval", &question, &options, false)
        .map_err(|e| format!("terminal error: {e}"))?;
    let Some(selected) = selected else {
        // Esc / Ctrl-C on the prompt: the safe reading is "no".
        return Ok(Some(ApprovalDecision::Deny));
    };
    let answer = selected.into_iter().next().unwrap_or_default();
    Ok(Some(match answer.as_str() {
        RUN_ONCE => ApprovalDecision::AllowOnce,
        ALLOW_SESSION => ApprovalDecision::AllowSession,
        ALLOW_PROJECT => ApprovalDecision::AllowProject,
        MOVE_TO_TRASH => ApprovalDecision::MoveToTrash,
        DENY => ApprovalDecision::Deny,
        // Anything else is the free-text row: a denial in the user's words.
        other => ApprovalDecision::DenyWithMessage(other.to_string()),
    }))
}

/// The prompt body: what wants to run, and why it was flagged.
fn question_text(request: &ApprovalRequest) -> String {
    let action = match request.kind {
        ApprovalKind::Shell => "The agent wants to run:",
        ApprovalKind::FileEdit => "The agent wants to write:",
        ApprovalKind::GitCommit => "The agent wants to commit:",
    };
    let mut text = format!("{action}\n\n    {}\n", request.command);
    if !request.reasons.is_empty() {
        text.push_str(&format!("\nFlagged: {}.", request.reasons.join("; ")));
    }
    text.push_str("\nAllow it?");
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_permissions::Risk;

    fn request(kind: ApprovalKind) -> ApprovalRequest {
        ApprovalRequest {
            kind,
            tool: "run_shell".into(),
            command: "rm -rf ./build".into(),
            risk: Risk::Dangerous,
            reasons: vec!["deletes files (rm)".into()],
            grant_label: "this exact command".into(),
            offer_project_grant: true,
            offer_trash: true,
        }
    }

    #[test]
    fn question_names_the_command_and_the_reasons() {
        let text = question_text(&request(ApprovalKind::Shell));
        assert!(text.contains("rm -rf ./build"));
        assert!(text.contains("deletes files"));
        let edit = question_text(&request(ApprovalKind::FileEdit));
        assert!(edit.contains("wants to write"));
    }
}
