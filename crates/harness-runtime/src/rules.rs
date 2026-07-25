//! Where stream rules come from.
//!
//! Two files, both optional and both plain JSON: `~/.oxen-harness/rules.json`
//! for the ones that follow you between projects, and
//! `<workspace>/.oxen-harness/rules.json` for the ones that belong to a
//! repository and travel with it in version control.
//!
//! ```json
//! {
//!   "rules": [
//!     {
//!       "name": "no-unwrap",
//!       "when": "\\.unwrap\\(\\)",
//!       "scope": ["tool"],
//!       "message": "This repo forbids `.unwrap()` outside tests — return a Result.",
//!       "interrupt": true,
//!       "repeat": "once"
//!     }
//!   ]
//! }
//! ```
//!
//! A rule that fails to compile is skipped rather than fatal: one bad regex in
//! a shared repo file must not stop everyone else's sessions from starting.
//! The engine (matching, repeat policy, injection) is `harness_agent::rules`;
//! this module only decides what exists.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Schema version for `rules.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// One rule as written on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSpec {
    /// Stable identity — named in the injected reminder, and what the
    /// once-per-session bookkeeping keys on.
    pub name: String,
    /// The regular expression to watch for.
    #[serde(rename = "when")]
    pub pattern: String,
    /// `text` (the reply's prose), `tool` (a tool call's arguments), or both.
    /// Empty means both.
    #[serde(default)]
    pub scope: Vec<String>,
    /// What to tell the model when it matches.
    pub message: String,
    /// Whether matching abandons the in-flight reply. Defaults to true — the
    /// point is to correct before the output lands.
    #[serde(default = "yes")]
    pub interrupt: bool,
    /// `"once"` (default) or `"after:<n>"` rounds.
    #[serde(default)]
    pub repeat: Option<String>,
    /// Set false to keep a rule in the file without it firing.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl RuleSpec {
    /// The rule in the shape the agent's compiler takes. Kept here so a host
    /// doesn't have to know the field order — it just forwards.
    pub fn parts(&self) -> (&str, &str, &[String], &str, bool, Option<&str>) {
        (
            &self.name,
            &self.pattern,
            &self.scope,
            &self.message,
            self.interrupt,
            self.repeat.as_deref(),
        )
    }
}

/// The file's shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rules {
    #[serde(default)]
    pub rules: Vec<RuleSpec>,
}

/// The user's own rules, which follow them between projects.
pub fn user_rules() -> Rules {
    crate::config::load_or_default(harness_config::paths::rules_file())
}

/// The rules committed to this repository, which travel with it.
pub fn project_rules(workspace: &Path) -> Rules {
    harness_config::read_versioned::<Rules>(&workspace.join(PROJECT_RULES_FILE)).1
}

/// Where a repository keeps its own rules.
pub const PROJECT_RULES_FILE: &str = ".oxen-harness/rules.json";

/// Every enabled rule for this workspace: the user's own first, then the
/// repository's — so a project can add to what you carry, and a name defined
/// in both resolves to the project's (it is the more specific claim).
pub fn load(workspace: &Path) -> Vec<RuleSpec> {
    let mut specs: Vec<RuleSpec> = Vec::new();
    for spec in user_rules()
        .rules
        .into_iter()
        .chain(project_rules(workspace).rules)
    {
        // Override first, *then* filter: a repository shipping
        // `{"name": "no-unwrap", "enabled": false}` is saying "not here", and
        // dropping the disabled entry early would leave the user's own copy
        // firing. Same for `/rules off` against a name the project also
        // defines.
        match specs.iter_mut().find(|existing| existing.name == spec.name) {
            Some(existing) => *existing = spec,
            None => specs.push(spec),
        }
    }
    specs.retain(|spec| spec.enabled);
    specs
}

/// Persist the user's global rules.
pub fn save(rules: &Rules) -> Result<(), crate::RuntimeError> {
    crate::config::write_and_snapshot(
        &harness_config::paths::rules_file()?,
        SCHEMA_VERSION,
        rules,
        "Update stream rules",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_temp_home;

    fn write_project_rules(root: &Path, body: &str) {
        let dir = root.join(".oxen-harness");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rules.json"), body).unwrap();
    }

    #[test]
    fn a_project_without_rules_has_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(with_temp_home(|| load(tmp.path())).is_empty());
    }

    #[test]
    fn project_rules_load_and_disabled_ones_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        write_project_rules(
            tmp.path(),
            r#"{"schema_version":1,"rules":[
                {"name":"no-unwrap","when":"unwrap","message":"don't","interrupt":true},
                {"name":"off","when":"x","message":"y","enabled":false}
            ]}"#,
        );

        let specs = with_temp_home(|| load(tmp.path()));

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "no-unwrap");
        // Unstated fields take the documented defaults.
        assert!(specs[0].interrupt);
        assert!(specs[0].scope.is_empty());
    }

    #[test]
    fn a_project_can_switch_off_a_rule_of_yours() {
        let tmp = tempfile::tempdir().unwrap();
        write_project_rules(
            tmp.path(),
            r#"{"schema_version":1,"rules":[
                {"name":"shared","when":"p","message":"not in this repo","enabled":false}
            ]}"#,
        );

        let specs = with_temp_home(|| {
            save(&Rules {
                rules: vec![RuleSpec {
                    name: "shared".into(),
                    pattern: "g".into(),
                    scope: vec![],
                    message: "from the user".into(),
                    interrupt: true,
                    repeat: None,
                    enabled: true,
                }],
            })
            .unwrap();
            load(tmp.path())
        });

        assert!(
            specs.is_empty(),
            "the project's override should win: {specs:?}"
        );
    }

    #[test]
    fn a_project_rule_overrides_a_global_one_of_the_same_name() {
        let tmp = tempfile::tempdir().unwrap();
        write_project_rules(
            tmp.path(),
            r#"{"schema_version":1,"rules":[
                {"name":"shared","when":"p","message":"from the project"}
            ]}"#,
        );

        let specs = with_temp_home(|| {
            save(&Rules {
                rules: vec![RuleSpec {
                    name: "shared".into(),
                    pattern: "g".into(),
                    scope: vec![],
                    message: "from the user".into(),
                    interrupt: true,
                    repeat: None,
                    enabled: true,
                }],
            })
            .unwrap();
            load(tmp.path())
        });

        assert_eq!(specs.len(), 1, "one name, one rule");
        assert_eq!(specs[0].message, "from the project");
    }
}
