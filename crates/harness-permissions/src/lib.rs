//! The permission gate consulted before every agent tool call.
//!
//! Layered defense for destructive actions, designed to stay out of the way:
//!
//! 1. **Classification** ([`classify`]) — shell commands are parsed with
//!    tree-sitter-bash and classified per simple command; anything the parser
//!    can't see through requires approval. Never a regex over the raw string.
//! 2. **Policy** ([`policy`]) — three modes (relaxed/cautious/bypass) plus
//!    allow/deny rules from `permissions.json` (global + per-project) and
//!    in-memory session grants. Deny wins; prefix rules only match cleanly
//!    parsed commands.
//! 3. **Approval** ([`approve`]) — when policy says *ask*, a host-injected
//!    [`CommandApprover`] puts the decision in front of the user (run once /
//!    always this session / always this project / move to trash / deny).
//! 4. **Circuit breakers** — `rm -rf /`, `rm -rf ~`, and writes to `.git`,
//!    shell rc files, or the harness's own config refuse to run in *every*
//!    mode, allow rules and bypass included.
//! 5. **Audit** ([`audit`]) — every decision and its source is one JSON line
//!    in `~/.oxen-harness/permissions.jsonl`.
//!
//! The gate hooks `Agent::run_tool` (one choke point covers the main agent,
//! fleet lanes, and review side-agents). Subagents get [`for_subagent`]: same
//! policy, but gated actions auto-decline with report-back instructions —
//! a lane must never block on a modal (see `harness-agent`'s `subagent_tools`).
//!
//! [`for_subagent`]: PermissionGate::for_subagent

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

pub mod approve;
pub mod audit;
pub mod classify;
pub mod policy;
mod snapshot;
mod trash;

pub use approve::{
    ApprovalDecision, ApprovalKind, ApprovalRequest, AutoDenyApprover, CommandApprover,
};
pub use classify::{classify, Analysis, Risk};
pub use policy::{PermissionMode, PermissionsConfig, PolicySet, SCHEMA_VERSION};

/// Grants accumulated during one session ("always allow this session").
/// Deliberately *not* shared with subagents: an in-chat approval authorizes
/// the conversation the user is watching, not headless lanes.
#[derive(Debug, Default)]
struct SessionGrants {
    exact: Vec<String>,
    prefixes: Vec<String>,
    /// Cautious mode: file writes/edits approved for the session.
    edits: bool,
    /// Cautious mode: git commits approved for the session.
    commits: bool,
}

/// The gate's verdict before any user interaction.
#[derive(Debug)]
pub enum GateReview {
    /// Run it.
    Allow,
    /// Refuse it; `message` is returned to the model as the tool result.
    Deny { message: String },
    /// Put it in front of the user (or the auto-deny approver).
    Ask(Box<ApprovalRequest>),
}

/// The final outcome after any approval flow.
#[derive(Debug)]
pub enum GateOutcome {
    Allow,
    /// Run a *rewritten* call instead (move-to-trash): new `run_shell`
    /// arguments, plus a note prepended to the tool result so the model knows
    /// the deletion became a relocation.
    AllowRewritten {
        args: serde_json::Value,
        note: String,
    },
    Deny { message: String },
}

/// Shell rc files a tool must never write: they execute on the user's next
/// shell start, making them a self-privilege-escalation vector.
const PROTECTED_BASENAMES: &[&str] = &[
    ".bashrc",
    ".zshrc",
    ".zprofile",
    ".profile",
    ".bash_profile",
    ".zshenv",
];

/// The permission gate for one session. Construct with [`PermissionGate::new`]
/// at agent build time; attach via `AgentConfig::permissions`.
pub struct PermissionGate {
    workspace: PathBuf,
    policy: Arc<RwLock<PolicySet>>,
    grants: RwLock<SessionGrants>,
    approver: Arc<dyn CommandApprover>,
    audit_path: Option<PathBuf>,
    /// True for the interactive session agent; false for subagent gates.
    subagent: bool,
}

impl std::fmt::Debug for PermissionGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionGate")
            .field("mode", &self.mode().label())
            .field("subagent", &self.subagent)
            .finish_non_exhaustive()
    }
}

impl PermissionGate {
    /// Build the session gate: loads global + project rules, resolves the
    /// audit log path, and prunes expired trash entries.
    pub fn new(workspace: impl Into<PathBuf>, approver: Arc<dyn CommandApprover>) -> Self {
        let workspace = workspace.into();
        trash::prune_expired();
        Self {
            policy: Arc::new(RwLock::new(PolicySet::load(&workspace))),
            grants: RwLock::new(SessionGrants::default()),
            approver,
            audit_path: harness_config::paths::permissions_log().ok(),
            subagent: false,
            workspace,
        }
    }

    /// The gate a detached subagent runs behind: same policy rules (shared, so
    /// a mid-session project grant reaches future lanes), fresh session grants,
    /// and an approver that declines instead of prompting — N lanes can't
    /// share one modal.
    pub fn for_subagent(&self) -> Self {
        Self {
            workspace: self.workspace.clone(),
            policy: self.policy.clone(),
            grants: RwLock::new(SessionGrants::default()),
            approver: Arc::new(AutoDenyApprover),
            audit_path: self.audit_path.clone(),
            subagent: true,
        }
    }

    /// The active permission mode.
    pub fn mode(&self) -> PermissionMode {
        self.policy.read().expect("policy poisoned").mode
    }

    /// The workspace this gate scopes (where project grants persist).
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// A snapshot of the merged rules currently in force (for `/permissions`).
    pub fn policy_snapshot(&self) -> PolicySet {
        self.policy.read().expect("policy poisoned").clone()
    }

    /// Whether approval prompts reach a human (drives host screen hand-off).
    pub fn is_interactive(&self) -> bool {
        self.approver.is_interactive()
    }

    /// Switch the live mode (mid-session `/permissions` toggle) and persist it
    /// as the global default. The policy is shared with this session's
    /// subagent gates, so future lanes see the new mode too.
    pub fn set_mode(&self, mode: PermissionMode) {
        self.policy.write().expect("policy poisoned").mode = mode;
        if let Err(e) = policy::persist_global_mode(mode) {
            tracing::warn!("could not persist permission mode: {e}");
        }
    }

    /// Reload allow/deny rules and mode from disk (after Settings edits), for
    /// the live session. Keeps in-memory session grants.
    pub fn reload_policy(&self) {
        let mut policy = self.policy.write().expect("policy poisoned");
        *policy = PolicySet::load(&self.workspace);
    }

    /// First stage: classify + apply policy, no user interaction. `args` are
    /// the tool call's parsed JSON arguments.
    pub fn review(&self, tool: &str, args: &serde_json::Value) -> GateReview {
        match tool {
            "run_shell" => self.review_shell(args),
            "write_file" | "edit_file" => self.review_file_edit(tool, args),
            "git" => self.review_git(args),
            _ => GateReview::Allow,
        }
    }

    fn review_shell(&self, args: &serde_json::Value) -> GateReview {
        let Some(command) = args.get("command").and_then(|c| c.as_str()) else {
            // Malformed arguments: let the tool's own parsing produce the error.
            return GateReview::Allow;
        };
        let analysis = classify(command, dirs::home_dir().as_deref());
        let policy = self.policy.read().expect("policy poisoned").clone();

        // Circuit breaker: refused in every mode, allow rules included.
        if let Some(reason) = &analysis.breaker {
            self.audit("run_shell", command, "deny", "circuit_breaker", &policy);
            return GateReview::Deny {
                message: format!(
                    "tool error: command refused ({reason}). This is a hard safety limit of the \
                     harness and cannot be approved or bypassed. Do not attempt to work around \
                     it; if the user truly wants this, they must run it themselves."
                ),
            };
        }
        // Deny rules from config (global + project).
        if let Some(rule) = policy.denies(command, &analysis.commands) {
            self.audit("run_shell", command, "deny", &format!("deny_rule:{rule}"), &policy);
            return GateReview::Deny {
                message: format!(
                    "tool error: command refused by the user's deny rule `{rule}`. Choose a \
                     different approach or ask the user."
                ),
            };
        }
        // Exact grants (session + config) cover any command verbatim.
        let session_exact = {
            let grants = self.grants.read().expect("grants poisoned");
            grants.exact.iter().any(|c| c.trim() == command.trim())
        };
        if session_exact || policy.allows_exact(command) {
            self.audit("run_shell", command, "allow", "exact_grant", &policy);
            return GateReview::Allow;
        }
        // Prefix rules only match cleanly parsed commands.
        let session_prefixes = {
            self.grants
                .read()
                .expect("grants poisoned")
                .prefixes
                .clone()
        };
        if policy.allows_by_prefix(&analysis.commands, &session_prefixes) {
            self.audit("run_shell", command, "allow", "prefix_rule", &policy);
            return GateReview::Allow;
        }

        // Mode defaults.
        let must_ask = match policy.mode {
            PermissionMode::Bypass => false,
            PermissionMode::Relaxed => analysis.risk >= Risk::Indirect,
            PermissionMode::Cautious => analysis.risk >= Risk::Unknown,
        };
        if !must_ask {
            if analysis.risk >= Risk::Unknown {
                self.audit(
                    "run_shell",
                    command,
                    "allow",
                    &format!("mode_{}_{}", policy.mode.label(), analysis.risk.label()),
                    &policy,
                );
            }
            return GateReview::Allow;
        }

        let grant_label = if analysis.grant_prefixes.is_empty() {
            "this exact command".to_string()
        } else {
            format!(
                "commands starting with {}",
                analysis
                    .grant_prefixes
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        GateReview::Ask(Box::new(ApprovalRequest {
            kind: ApprovalKind::Shell,
            tool: "run_shell".to_string(),
            command: command.to_string(),
            risk: analysis.risk,
            reasons: analysis.reasons.clone(),
            grant_label,
            offer_project_grant: true,
            offer_trash: analysis.trash_plan.is_some(),
        }))
    }

    fn review_file_edit(&self, tool: &str, args: &serde_json::Value) -> GateReview {
        let Some(path) = args.get("path").and_then(|p| p.as_str()) else {
            return GateReview::Allow;
        };
        // Circuit breaker: never let a tool rewrite git internals, shell rc
        // files, or the harness's own permission rules — in any mode.
        if let Some(reason) = protected_path_reason(path) {
            let policy = self.policy.read().expect("policy poisoned").clone();
            self.audit(tool, path, "deny", "protected_path", &policy);
            return GateReview::Deny {
                message: format!(
                    "tool error: refusing to write {path}: {reason}. This is a hard safety limit; \
                     ask the user to change this file themselves if it's truly needed."
                ),
            };
        }
        let policy = self.policy.read().expect("policy poisoned").clone();
        let approved = self.grants.read().expect("grants poisoned").edits;
        if policy.mode != PermissionMode::Cautious || approved {
            return GateReview::Allow;
        }
        GateReview::Ask(Box::new(ApprovalRequest {
            kind: ApprovalKind::FileEdit,
            tool: tool.to_string(),
            command: path.to_string(),
            risk: Risk::Unknown,
            reasons: Vec::new(),
            grant_label: "all file writes and edits".to_string(),
            offer_project_grant: false,
            offer_trash: false,
        }))
    }

    fn review_git(&self, args: &serde_json::Value) -> GateReview {
        let is_commit = args.get("operation").and_then(|o| o.as_str()) == Some("commit");
        let policy = self.policy.read().expect("policy poisoned").clone();
        let approved = self.grants.read().expect("grants poisoned").commits;
        if !is_commit || policy.mode != PermissionMode::Cautious || approved {
            return GateReview::Allow;
        }
        let message = args
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        GateReview::Ask(Box::new(ApprovalRequest {
            kind: ApprovalKind::GitCommit,
            tool: "git".to_string(),
            command: format!("git commit — {message}"),
            risk: Risk::Unknown,
            reasons: Vec::new(),
            grant_label: "all git commits".to_string(),
            offer_project_grant: false,
            offer_trash: false,
        }))
    }

    /// Second stage: put an [`ApprovalRequest`] in front of the approver and
    /// act on the decision (record grants, persist project rules, snapshot
    /// before approved-dangerous, rewrite to trash).
    pub async fn resolve(&self, request: ApprovalRequest) -> (GateOutcome, ApprovalDecision) {
        let policy = self.policy.read().expect("policy poisoned").clone();
        let decision = match self.approver.approve(&request).await {
            Err(e) => {
                let message = format!("tool error: the approval prompt failed ({e}); not running.");
                self.audit(&request.tool, &request.command, "deny", "approver_error", &policy);
                return (GateOutcome::Deny { message }, ApprovalDecision::Deny);
            }
            Ok(None) => {
                self.audit(&request.tool, &request.command, "deny", "no_interactive_user", &policy);
                return (
                    GateOutcome::Deny {
                        message: "tool error: this action requires the user's approval, but no \
                                  interactive user is available in this session. Proceed another \
                                  way, or state clearly what you wanted to run and why so the \
                                  user can do it or approve it later."
                            .to_string(),
                    },
                    ApprovalDecision::Deny,
                );
            }
            Ok(Some(decision)) => decision,
        };

        let outcome = match &decision {
            ApprovalDecision::Deny | ApprovalDecision::DenyWithMessage(_) => {
                let source = if self.subagent { "subagent_auto_deny" } else { "user_denied" };
                self.audit(&request.tool, &request.command, "deny", source, &policy);
                GateOutcome::Deny {
                    message: self.denial_message(&decision),
                }
            }
            ApprovalDecision::AllowOnce => {
                self.audit(&request.tool, &request.command, "allow", "user_once", &policy);
                self.snapshot_if_dangerous(&request).await;
                GateOutcome::Allow
            }
            ApprovalDecision::AllowSession => {
                self.record_session_grant(&request);
                self.audit(&request.tool, &request.command, "allow", "user_session", &policy);
                self.snapshot_if_dangerous(&request).await;
                GateOutcome::Allow
            }
            ApprovalDecision::AllowProject => {
                self.persist_project_grant(&request);
                self.audit(&request.tool, &request.command, "allow", "user_project", &policy);
                self.snapshot_if_dangerous(&request).await;
                GateOutcome::Allow
            }
            ApprovalDecision::AllowAllBypass => {
                // Live-only: the shared policy flips to bypass (future fleet
                // lanes included) but nothing persists — a new chat starts
                // back at the configured default, and circuit breakers keep
                // refusing regardless.
                self.policy.write().expect("policy poisoned").mode = PermissionMode::Bypass;
                self.audit(&request.tool, &request.command, "allow", "user_bypass_session", &policy);
                self.snapshot_if_dangerous(&request).await;
                GateOutcome::Allow
            }
            ApprovalDecision::MoveToTrash => {
                // Re-derive the plan from the command (the request only carries
                // the flag); classification is deterministic.
                let plan = classify(&request.command, dirs::home_dir().as_deref()).trash_plan;
                match plan.as_ref().and_then(trash::rewrite) {
                    Some((command, note)) => {
                        self.audit(&request.tool, &request.command, "allow", "user_trash", &policy);
                        GateOutcome::AllowRewritten {
                            args: serde_json::json!({ "command": command }),
                            note,
                        }
                    }
                    None => {
                        self.audit(&request.tool, &request.command, "deny", "trash_unavailable", &policy);
                        GateOutcome::Deny {
                            message: "tool error: could not build the move-to-trash command; \
                                      the deletion was not run."
                                .to_string(),
                        }
                    }
                }
            }
        };
        (outcome, decision)
    }

    /// The message the model reads when a gated action was declined.
    fn denial_message(&self, decision: &ApprovalDecision) -> String {
        if self.subagent {
            return "tool error: this command needs user approval, which subagents cannot \
                    request. Do not retry it or work around it; instead, note the exact command \
                    and why it's needed in your final summary so the orchestrator or user can \
                    run it."
                .to_string();
        }
        let note = match decision {
            ApprovalDecision::DenyWithMessage(msg) if !msg.trim().is_empty() => {
                format!(" The user said: {}", msg.trim())
            }
            _ => String::new(),
        };
        format!(
            "tool error: the user declined to run this command.{note} Do not retry it verbatim \
             or attempt the same effect another way; adjust your approach or ask the user what \
             they'd prefer."
        )
    }

    fn record_session_grant(&self, request: &ApprovalRequest) {
        let mut grants = self.grants.write().expect("grants poisoned");
        match request.kind {
            ApprovalKind::Shell => {
                let analysis = classify(&request.command, dirs::home_dir().as_deref());
                if analysis.grant_prefixes.is_empty() {
                    grants.exact.push(request.command.trim().to_string());
                } else {
                    grants.prefixes.extend(analysis.grant_prefixes);
                }
            }
            ApprovalKind::FileEdit => grants.edits = true,
            ApprovalKind::GitCommit => grants.commits = true,
        }
    }

    fn persist_project_grant(&self, request: &ApprovalRequest) {
        let analysis = classify(&request.command, dirs::home_dir().as_deref());
        let (exact, prefixes) = if analysis.grant_prefixes.is_empty() {
            (Some(request.command.as_str()), Vec::new())
        } else {
            (None, analysis.grant_prefixes.clone())
        };
        if let Err(e) = policy::persist_project_grant(&self.workspace, exact, &prefixes) {
            tracing::warn!("could not persist project permission grant: {e}");
        }
        // Reload so the live policy (shared with subagent gates) reflects it.
        let mut policy = self.policy.write().expect("policy poisoned");
        *policy = PolicySet::load(&self.workspace);
    }

    /// Snapshot the workspace before an approved dangerous shell command, so a
    /// wrong approval is recoverable. Best-effort and audited either way.
    async fn snapshot_if_dangerous(&self, request: &ApprovalRequest) {
        if request.kind != ApprovalKind::Shell || request.risk < Risk::Dangerous {
            return;
        }
        let workspace = self.workspace.clone();
        let result = tokio::task::spawn_blocking(move || snapshot::take(&workspace))
            .await
            .unwrap_or_else(|e| Err(format!("snapshot task panicked: {e}")));
        match result {
            Ok(hash) => audit::record(
                self.audit_path.as_deref(),
                serde_json::json!({
                    "event": "snapshot",
                    "workspace": self.workspace.display().to_string(),
                    "commit": hash,
                    "command": request.command,
                }),
            ),
            Err(e) => tracing::warn!("pre-command snapshot failed: {e}"),
        }
    }

    fn audit(&self, tool: &str, command: &str, decision: &str, source: &str, policy: &PolicySet) {
        audit::record(
            self.audit_path.as_deref(),
            serde_json::json!({
                "event": "gate_decision",
                "tool": tool,
                "command": command,
                "decision": decision,
                "source": source,
                "mode": policy.mode.label(),
                "subagent": self.subagent,
                "workspace": self.workspace.display().to_string(),
            }),
        );
    }
}

/// Why a written path is protected, if it is. Checked lexically on the
/// tool-supplied (workspace-relative or absolute) path.
fn protected_path_reason(path: &str) -> Option<&'static str> {
    let p = Path::new(path);
    if p.components().any(|c| c.as_os_str() == ".git") {
        return Some("it is inside a .git directory (repository internals)");
    }
    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
        if PROTECTED_BASENAMES.contains(&name) {
            return Some("shell startup files execute code on the user's next shell");
        }
    }
    if path.contains(".oxen-harness") && path.ends_with("permissions.json") {
        return Some("the agent must not edit its own permission rules");
    }
    None
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Tests that touch `OXEN_HARNESS_DIR` must not interleave.
    pub(crate) fn env_guard() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// An approver scripted to always return one decision.
    struct Scripted(Option<ApprovalDecision>);

    #[async_trait]
    impl CommandApprover for Scripted {
        async fn approve(
            &self,
            _request: &ApprovalRequest,
        ) -> Result<Option<ApprovalDecision>, String> {
            Ok(self.0.clone())
        }
    }

    /// Build a gate against fresh temp home + workspace dirs. Sets
    /// `OXEN_HARNESS_DIR` so audit/config writes never touch the real home —
    /// callers must hold [`testutil::env_guard`] first.
    fn gate(
        decision: Option<ApprovalDecision>,
    ) -> (tempfile::TempDir, tempfile::TempDir, PermissionGate) {
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("OXEN_HARNESS_DIR", home.path());
        let ws = tempfile::tempdir().unwrap();
        let gate = PermissionGate::new(ws.path(), Arc::new(Scripted(decision)));
        (home, ws, gate)
    }

    fn shell_args(command: &str) -> serde_json::Value {
        serde_json::json!({ "command": command })
    }

    #[test]
    fn safe_commands_pass_without_asking() {
        let _env = testutil::env_guard();
        let (_home, _ws, gate) = gate(None);
        assert!(matches!(
            gate.review("run_shell", &shell_args("git status")),
            GateReview::Allow
        ));
        assert!(matches!(
            gate.review("run_shell", &shell_args("cargo build")),
            GateReview::Allow
        ));
        // Non-shell tools pass through in relaxed mode.
        assert!(matches!(
            gate.review("write_file", &serde_json::json!({"path": "src/main.rs"})),
            GateReview::Allow
        ));
    }

    #[test]
    fn dangerous_commands_ask_and_breakers_deny() {
        let _env = testutil::env_guard();
        let (_home, _ws, gate) = gate(None);
        assert!(matches!(
            gate.review("run_shell", &shell_args("rm -rf ./build")),
            GateReview::Ask(_)
        ));
        let review = gate.review("run_shell", &shell_args("rm -rf ~"));
        match review {
            GateReview::Deny { message } => assert!(message.contains("hard safety limit")),
            other => panic!("expected breaker deny, got {other:?}"),
        }
    }

    #[test]
    fn protected_paths_deny_in_every_mode() {
        let _env = testutil::env_guard();
        let (_home, _ws, gate) = gate(None);
        for path in [
            ".git/hooks/pre-commit",
            "../.bashrc",
            ".oxen-harness/permissions.json",
        ] {
            let review = gate.review("write_file", &serde_json::json!({"path": path}));
            assert!(
                matches!(review, GateReview::Deny { .. }),
                "expected protected-path deny for {path}"
            );
        }
    }

    #[tokio::test]
    async fn session_grant_stops_repeat_prompts() {
        let _env = testutil::env_guard();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("OXEN_HARNESS_DIR", home.path());
        let (_home, _ws, gate) = gate(Some(ApprovalDecision::AllowSession));

        let GateReview::Ask(request) = gate.review("run_shell", &shell_args("rm -rf ./build"))
        else {
            panic!("expected ask");
        };
        let (outcome, decision) = gate.resolve(*request).await;
        assert!(matches!(outcome, GateOutcome::Allow));
        assert_eq!(decision, ApprovalDecision::AllowSession);
        // Dangerous grant is exact: the same command passes, a different rm asks.
        assert!(matches!(
            gate.review("run_shell", &shell_args("rm -rf ./build")),
            GateReview::Allow
        ));
        assert!(matches!(
            gate.review("run_shell", &shell_args("rm -rf ./dist")),
            GateReview::Ask(_)
        ));
        std::env::remove_var("OXEN_HARNESS_DIR");
    }

    #[tokio::test]
    async fn project_grant_persists_and_reloads() {
        let _env = testutil::env_guard();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("OXEN_HARNESS_DIR", home.path());
        let (_home, ws, gate) = gate(Some(ApprovalDecision::AllowProject));

        let GateReview::Ask(request) = gate.review("run_shell", &shell_args("rm -rf ./build"))
        else {
            panic!("expected ask");
        };
        let (outcome, _) = gate.resolve(*request).await;
        assert!(matches!(outcome, GateOutcome::Allow));
        // Persisted: a *fresh* gate for the same workspace allows it.
        let fresh = PermissionGate::new(ws.path(), Arc::new(Scripted(None)));
        assert!(matches!(
            fresh.review("run_shell", &shell_args("rm -rf ./build")),
            GateReview::Allow
        ));
        std::env::remove_var("OXEN_HARNESS_DIR");
    }

    #[tokio::test]
    async fn subagent_gate_auto_denies_with_report_back() {
        let _env = testutil::env_guard();
        let (_home, _ws, gate) = gate(Some(ApprovalDecision::AllowOnce));
        let sub = gate.for_subagent();
        assert!(!sub.is_interactive());
        let GateReview::Ask(request) = sub.review("run_shell", &shell_args("kill -9 42")) else {
            panic!("expected ask");
        };
        let (outcome, _) = sub.resolve(*request).await;
        match outcome {
            GateOutcome::Deny { message } => {
                assert!(message.contains("final summary"), "got: {message}")
            }
            other => panic!("expected deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn trash_decision_rewrites_the_command() {
        let _env = testutil::env_guard();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("OXEN_HARNESS_DIR", home.path());
        let (_home, _ws, gate) = gate(Some(ApprovalDecision::MoveToTrash));
        let GateReview::Ask(request) = gate.review("run_shell", &shell_args("rm -rf build")) else {
            panic!("expected ask");
        };
        assert!(request.offer_trash);
        let (outcome, _) = gate.resolve(*request).await;
        match outcome {
            GateOutcome::AllowRewritten { args, note } => {
                let cmd = args["command"].as_str().unwrap();
                assert!(cmd.contains("mv 'build'"), "got: {cmd}");
                assert!(note.contains("recoverable"));
            }
            other => panic!("expected rewrite, got {other:?}"),
        }
        std::env::remove_var("OXEN_HARNESS_DIR");
    }

    #[tokio::test]
    async fn allow_all_bypass_silences_the_session_but_not_the_breakers() {
        let _env = testutil::env_guard();
        let (_home, _ws, gate) = gate(Some(ApprovalDecision::AllowAllBypass));

        let GateReview::Ask(request) = gate.review("run_shell", &shell_args("rm -rf ./build"))
        else {
            panic!("expected ask");
        };
        let (outcome, decision) = gate.resolve(*request).await;
        assert!(matches!(outcome, GateOutcome::Allow));
        assert_eq!(decision, ApprovalDecision::AllowAllBypass);

        // The live session (and its future subagent gates) is now in bypass:
        // dangerous commands run without asking…
        assert_eq!(gate.mode(), PermissionMode::Bypass);
        assert!(matches!(
            gate.review("run_shell", &shell_args("git push --force")),
            GateReview::Allow
        ));
        assert_eq!(gate.for_subagent().mode(), PermissionMode::Bypass);
        // …but circuit breakers still refuse, and nothing was persisted (a
        // fresh gate for the same workspace starts back at the default).
        assert!(matches!(
            gate.review("run_shell", &shell_args("rm -rf ~")),
            GateReview::Deny { .. }
        ));
        assert_eq!(policy::load_global().mode, None);
    }

    #[test]
    fn bypass_mode_still_trips_breakers() {
        let _env = testutil::env_guard();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("OXEN_HARNESS_DIR", home.path());
        let ws = tempfile::tempdir().unwrap();
        // Write a project config selecting bypass mode.
        harness_config::io::write_versioned(
            &policy::project_permissions_file(ws.path()),
            SCHEMA_VERSION,
            &PermissionsConfig {
                mode: Some(PermissionMode::Bypass),
                ..Default::default()
            },
        )
        .unwrap();
        let gate = PermissionGate::new(ws.path(), Arc::new(Scripted(None)));
        assert_eq!(gate.mode(), PermissionMode::Bypass);
        assert!(matches!(
            gate.review("run_shell", &shell_args("rm -rf ./build")),
            GateReview::Allow
        ));
        assert!(matches!(
            gate.review("run_shell", &shell_args("sudo rm -rf /")),
            GateReview::Deny { .. }
        ));
        std::env::remove_var("OXEN_HARNESS_DIR");
    }

    #[test]
    fn cautious_mode_gates_unknown_commands_and_edits() {
        let _env = testutil::env_guard();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("OXEN_HARNESS_DIR", home.path());
        let ws = tempfile::tempdir().unwrap();
        harness_config::io::write_versioned(
            &policy::project_permissions_file(ws.path()),
            SCHEMA_VERSION,
            &PermissionsConfig {
                mode: Some(PermissionMode::Cautious),
                ..Default::default()
            },
        )
        .unwrap();
        let gate = PermissionGate::new(ws.path(), Arc::new(Scripted(None)));
        // Safe still flows; unknown now asks; edits and commits ask.
        assert!(matches!(
            gate.review("run_shell", &shell_args("git status")),
            GateReview::Allow
        ));
        assert!(matches!(
            gate.review("run_shell", &shell_args("cargo build")),
            GateReview::Ask(_)
        ));
        assert!(matches!(
            gate.review("write_file", &serde_json::json!({"path": "src/x.rs"})),
            GateReview::Ask(_)
        ));
        assert!(matches!(
            gate.review(
                "git",
                &serde_json::json!({"operation": "commit", "message": "wip"})
            ),
            GateReview::Ask(_)
        ));
        std::env::remove_var("OXEN_HARNESS_DIR");
    }
}
