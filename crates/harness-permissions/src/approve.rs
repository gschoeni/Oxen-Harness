//! The approval bridge: what the gate asks a host, and what comes back.
//!
//! Mirrors the `QuestionAsker` pattern in `harness-tools`: the host injects a
//! [`CommandApprover`] at build time, the gate blocks on it when a tool call
//! needs a decision, and `Ok(None)` means "no interactive user is available"
//! (a piped/non-TTY session) — the gate then declines the command rather than
//! hanging or silently running it.
//!
//! Subagents (fleet lanes, review side-agents) get [`AutoDenyApprover`]: N
//! lanes can't share the host's single interactive prompt (the same deadlock
//! reasoning that strips `ask_user_question` from subagents), so a gated
//! command in a lane is declined with instructions to report it back instead.

use async_trait::async_trait;

use crate::classify::Risk;

/// What kind of action is awaiting approval (drives prompt wording).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalKind {
    /// A `run_shell` command.
    Shell,
    /// A `write_file`/`edit_file` call (cautious mode).
    FileEdit,
    /// A `git` tool commit (cautious mode).
    GitCommit,
    /// A `kill_task` call terminating a background task's process group
    /// (cautious mode) — gated like the equivalent `run_shell` kill would be.
    TaskKill,
}

/// Everything a host needs to render one approval prompt.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub kind: ApprovalKind,
    /// The tool that wants to run (`run_shell`, `write_file`, …).
    pub tool: String,
    /// What to show: the shell command, the file path, or the commit summary.
    pub command: String,
    pub risk: Risk,
    /// Why this was flagged (empty for mode-driven prompts on unknown commands).
    pub reasons: Vec<String>,
    /// Human-readable description of what "always allow" would grant, e.g.
    /// "commands starting with `git push`" or "this exact command".
    pub grant_label: String,
    /// Whether "always for this project" is offered (persists to the project's
    /// permissions.json).
    pub offer_project_grant: bool,
    /// Whether "move to trash instead" is offered (plain `rm` only).
    pub offer_trash: bool,
}

/// The user's decision on one [`ApprovalRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Run it this once.
    AllowOnce,
    /// Run it, and don't ask again this session (grant recorded in memory).
    AllowSession,
    /// Run it, and persist the grant to the project's permissions file.
    AllowProject,
    /// Run it, and switch this session's gate to bypass mode — nothing asks
    /// again (circuit breakers still refuse). The "dangerously allow
    /// everything" escape hatch, deliberately session-scoped and not
    /// persisted: a new chat starts back at the configured default.
    AllowAllBypass,
    /// Don't delete — relocate the targets into the harness trash instead.
    MoveToTrash,
    /// Don't run it.
    Deny,
    /// Don't run it, and tell the model why (the user's own words).
    DenyWithMessage(String),
}

impl ApprovalDecision {
    /// Short label for the resolved-event line and the audit log.
    pub fn label(&self) -> &'static str {
        match self {
            ApprovalDecision::AllowOnce => "approved",
            ApprovalDecision::AllowSession => "approved for this session",
            ApprovalDecision::AllowProject => "approved for this project",
            ApprovalDecision::AllowAllBypass => {
                "approved — allowing everything this session (bypass)"
            }
            ApprovalDecision::MoveToTrash => "moved to trash instead",
            ApprovalDecision::Deny | ApprovalDecision::DenyWithMessage(_) => "denied",
        }
    }
}

/// A host that can put an [`ApprovalRequest`] in front of the user.
#[async_trait]
pub trait CommandApprover: Send + Sync {
    /// Present the request and collect a decision. `Ok(None)` means no
    /// interactive user is available; the gate treats that as a decline.
    async fn approve(&self, request: &ApprovalRequest) -> Result<Option<ApprovalDecision>, String>;

    /// Whether this approver actually prompts a human (drives whether the
    /// agent emits approval events for the host to hand the screen over).
    fn is_interactive(&self) -> bool {
        true
    }
}

/// The subagent approver: never prompts, always declines. The gate composes
/// the report-back message (see `PermissionGate::for_subagent`).
pub struct AutoDenyApprover;

#[async_trait]
impl CommandApprover for AutoDenyApprover {
    async fn approve(
        &self,
        _request: &ApprovalRequest,
    ) -> Result<Option<ApprovalDecision>, String> {
        Ok(Some(ApprovalDecision::Deny))
    }

    fn is_interactive(&self) -> bool {
        false
    }
}
