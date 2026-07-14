//! Permission modes, the on-disk rules config, and rule matching.
//!
//! Two files feed a session's policy, both the versioned-JSON shape every
//! harness config uses (see `harness-config::io`):
//!
//! - `~/.oxen-harness/permissions.json` — the user's global default mode and
//!   allow/deny rules.
//! - `<workspace>/.oxen-harness/permissions.json` — per-project rules; this is
//!   where "always allow for this project" grants persist. A project `mode`
//!   overrides the global one.
//!
//! Rules are deliberately simple: `deny` and `allow` are word-boundary command
//! prefixes (`git push` matches `git push origin` but not `git pushx`);
//! `allow_exact` matches the whole command string verbatim. Deny always wins,
//! and prefix rules only apply to commands the parser could fully see through
//! — a command with substitution/indirection can only match an exact grant.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Schema version for `permissions.json` (global and per-project).
pub const SCHEMA_VERSION: u32 = 1;

/// How eagerly the gate asks before running tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// Only recognizably dangerous or unparseable shell commands prompt;
    /// everything else runs. The default.
    #[default]
    Relaxed,
    /// Only recognizably read-only commands run unprompted; file writes/edits
    /// and `git commit` prompt too.
    Cautious,
    /// Nothing prompts — except circuit breakers, which fire in every mode.
    Bypass,
}

impl PermissionMode {
    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::Relaxed => "relaxed",
            PermissionMode::Cautious => "cautious",
            PermissionMode::Bypass => "bypass",
        }
    }
}

/// One `permissions.json` payload (global or per-project).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionsConfig {
    /// Default mode; a project file's value overrides the global one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PermissionMode>,
    /// Word-boundary command prefixes that run without prompting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// Whole commands (verbatim, trimmed) that run without prompting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_exact: Vec<String>,
    /// Word-boundary command prefixes that are always refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

/// The merged, effective policy for one session.
#[derive(Debug, Clone, Default)]
pub struct PolicySet {
    pub mode: PermissionMode,
    pub allow: Vec<String>,
    pub allow_exact: Vec<String>,
    pub deny: Vec<String>,
}

/// Where a project's permissions file lives.
pub fn project_permissions_file(workspace: &Path) -> PathBuf {
    workspace.join(".oxen-harness").join("permissions.json")
}

impl PolicySet {
    /// Load and merge the global and per-project configs. Missing/unreadable
    /// files read as defaults — permissions config is never a hard failure.
    pub fn load(workspace: &Path) -> Self {
        let global: PermissionsConfig = harness_config::paths::permissions_file()
            .map(|p| harness_config::io::read_versioned::<PermissionsConfig>(&p).1)
            .unwrap_or_default();
        let project =
            harness_config::io::read_versioned::<PermissionsConfig>(&project_permissions_file(
                workspace,
            ))
            .1;
        Self::merge(global, project)
    }

    fn merge(global: PermissionsConfig, project: PermissionsConfig) -> Self {
        let mode = project.mode.or(global.mode).unwrap_or_default();
        let mut merged = Self {
            mode,
            allow: global.allow,
            allow_exact: global.allow_exact,
            deny: global.deny,
        };
        merged.allow.extend(project.allow);
        merged.allow_exact.extend(project.allow_exact);
        merged.deny.extend(project.deny);
        merged
    }

    /// Does any deny rule match this command line or one of its subcommands?
    pub fn denies(&self, command: &str, subcommand_renderings: &[String]) -> Option<&str> {
        self.deny
            .iter()
            .find(|rule| {
                prefix_matches(rule, command)
                    || subcommand_renderings.iter().any(|s| prefix_matches(rule, s))
            })
            .map(String::as_str)
    }

    /// Is the whole command covered by an exact allow?
    pub fn allows_exact(&self, command: &str) -> bool {
        let trimmed = command.trim();
        self.allow_exact.iter().any(|c| c.trim() == trimmed)
    }

    /// Are *all* subcommands of a cleanly-parsed line covered by allow-prefix
    /// rules (config rules plus `extra` session grants)?
    pub fn allows_by_prefix(&self, subcommand_renderings: &[String], extra: &[String]) -> bool {
        !subcommand_renderings.is_empty()
            && subcommand_renderings.iter().all(|s| {
                self.allow.iter().chain(extra.iter()).any(|rule| prefix_matches(rule, s))
            })
    }
}

/// Word-boundary prefix match: every whitespace token of `rule` must equal the
/// corresponding leading token of `command`.
pub fn prefix_matches(rule: &str, command: &str) -> bool {
    let rule_tokens: Vec<&str> = rule.split_whitespace().collect();
    if rule_tokens.is_empty() {
        return false;
    }
    let cmd_tokens: Vec<&str> = command.split_whitespace().collect();
    cmd_tokens.len() >= rule_tokens.len()
        && rule_tokens.iter().zip(&cmd_tokens).all(|(r, c)| r == c)
}

/// Persist a grant into the *project* permissions file (the "always allow for
/// this project" decision) and return the updated merged policy.
pub fn persist_project_grant(
    workspace: &Path,
    exact: Option<&str>,
    prefixes: &[String],
) -> Result<(), harness_config::ConfigError> {
    let path = project_permissions_file(workspace);
    let mut config = harness_config::io::read_versioned::<PermissionsConfig>(&path).1;
    if let Some(exact) = exact {
        let exact = exact.trim().to_string();
        if !config.allow_exact.contains(&exact) {
            config.allow_exact.push(exact);
        }
    }
    for p in prefixes {
        if !config.allow.contains(p) {
            config.allow.push(p.clone());
        }
    }
    harness_config::io::write_versioned(&path, SCHEMA_VERSION, &config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_rules_match_on_word_boundaries() {
        assert!(prefix_matches("git push", "git push origin main"));
        assert!(prefix_matches("git push", "git push"));
        assert!(!prefix_matches("git push", "git pushx origin"));
        assert!(!prefix_matches("git push", "git pull"));
        assert!(!prefix_matches("ls", "lsof -i"));
        assert!(!prefix_matches("", "anything"));
    }

    #[test]
    fn deny_wins_and_project_overrides_mode() {
        let merged = PolicySet::merge(
            PermissionsConfig {
                mode: Some(PermissionMode::Relaxed),
                deny: vec!["curl".into()],
                ..Default::default()
            },
            PermissionsConfig {
                mode: Some(PermissionMode::Cautious),
                allow: vec!["curl -s".into()], // deny still wins over allow
                ..Default::default()
            },
        );
        assert_eq!(merged.mode, PermissionMode::Cautious);
        assert!(merged.denies("curl -s https://x.dev", &[]).is_some());
    }

    #[test]
    fn all_subcommands_must_be_allowed() {
        let policy = PolicySet {
            allow: vec!["git push".into()],
            ..Default::default()
        };
        assert!(policy.allows_by_prefix(&["git push origin".into()], &[]));
        assert!(!policy.allows_by_prefix(
            &["git push origin".into(), "curl https://x.dev".into()],
            &[]
        ));
        // Session grants extend the config rules.
        assert!(policy.allows_by_prefix(
            &["git push origin".into(), "curl https://x.dev".into()],
            &["curl".into()]
        ));
        // An empty rendering list (indirection) never matches prefix rules.
        assert!(!policy.allows_by_prefix(&[], &["anything".into()]));
    }

    #[test]
    fn project_grants_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        persist_project_grant(dir.path(), Some("rm -rf ./build"), &["cargo test".into()])
            .unwrap();
        persist_project_grant(dir.path(), Some("rm -rf ./build"), &[]).unwrap(); // dedupe
        let config = harness_config::io::read_versioned::<PermissionsConfig>(
            &project_permissions_file(dir.path()),
        )
        .1;
        assert_eq!(config.allow_exact, vec!["rm -rf ./build"]);
        assert_eq!(config.allow, vec!["cargo test"]);
    }
}
