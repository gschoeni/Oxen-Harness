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
    /// What the user asked for, when the model wrote this rule. Kept so
    /// reopening it restores the conversation's starting point instead of an
    /// empty box — the description is usually a better record of *intent*
    /// than the regex it produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// A line this rule is meant to catch, used to seed the editor's tester.
    /// Without it, reopening a rule about `kill` offers a sample about
    /// `.unwrap()` and reports "no match", which reads as a broken rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
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

/// A rule worth suggesting, with the words a person needs to decide.
///
/// Suggestions live here rather than in either front end, so the desktop
/// gallery and `/rules suggest` offer the same set described the same way.
#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    /// What it does, in plain language — the headline someone reads first.
    pub title: String,
    /// Why you'd want it, in one line.
    pub why: String,
    /// What it catches, as prose rather than as the regex.
    pub catches: String,
    /// Which family it belongs to ("Any project", "Rust", …), for grouping.
    pub group: String,
    /// The rule itself.
    pub rule: RuleSpec,
}

/// The parts that vary between suggestions. A borrowed twin of [`Suggestion`]
/// so the library below reads as a list of descriptions rather than a wall of
/// `.to_string()`.
struct Draft {
    group: &'static str,
    name: &'static str,
    title: &'static str,
    why: &'static str,
    /// What it catches, in prose, for the card.
    catches: &'static str,
    /// A real line the pattern matches, which becomes the tester's sample so
    /// an added rule can demonstrate itself immediately.
    example: &'static str,
    pattern: &'static str,
    interrupt: bool,
    message: &'static str,
}

impl From<Draft> for Suggestion {
    fn from(d: Draft) -> Self {
        Self {
            title: d.title.into(),
            why: d.why.into(),
            catches: d.catches.into(),
            group: d.group.into(),
            rule: RuleSpec {
                name: d.name.into(),
                pattern: d.pattern.into(),
                // Suggestions watch tool calls: that's where a change becomes
                // visible early enough to stop, and a prose-scoped rule fires
                // when the model merely discusses the thing.
                scope: vec!["tool".into()],
                message: d.message.into(),
                interrupt: d.interrupt,
                repeat: Some("once".into()),
                enabled: true,
                prompt: None,
                sample: Some(d.example.into()),
            },
        }
    }
}

/// Rules worth offering to someone who has none.
///
/// Chosen to be legible: each catches something concrete, and the ones that
/// interrupt are the ones where landing the correction late means undoing work
/// (a force-push, a rewritten generated file) rather than merely reading worse.
pub fn suggestions() -> Vec<Suggestion> {
    [
        Draft {
            group: "Any project",
            name: "no-force-push",
            title: "Don't force-push",
            why: "Rewriting shared history is the one git mistake that costs other people their work.",
            catches: "git push --force, push -f",
            example: "git push --force origin main",
            pattern: r"push\s+--force|push\s+-f\b",
            interrupt: true,
            message: "Don't force-push. If history needs fixing, say what you'd do and let me decide.",
        },
        Draft {
            group: "Any project",
            name: "leave-generated-alone",
            title: "Protect generated files",
            why: "Edits to generated code vanish on the next build, and the real fix is upstream.",
            catches: "paths under generated/",
            example: "write_file generated/api_client.ts",
            pattern: r"generated/|\.generated\.",
            interrupt: true,
            message: "Files under generated/ are produced by the build. Change the generator or its input instead, then re-run it.",
        },
        Draft {
            group: "Any project",
            name: "no-hardcoded-secrets",
            title: "Keep credentials out of the code",
            why: "A key written into a file is a key in your git history, whether or not it ships.",
            catches: "api_key = \"…\", password: \"…\"",
            example: r#"API_KEY = "xxxx-not-a-real-key""#,
            pattern: r#"(?i)(api[_-]?key|secret|password|token)\s*[:=]\s*["'][^"']{12,}"#,
            interrupt: true,
            message: "Don't write credentials into files. Read them from the environment, and tell me which variable to set.",
        },
        Draft {
            group: "Any project",
            name: "ask-before-adding-deps",
            title: "Ask before adding a dependency",
            why: "A dependency is permanent code you don't control — worth one sentence of justification.",
            catches: "cargo add, npm install, pip install",
            example: "cargo add serde_yaml",
            pattern: r"cargo add |npm install |pnpm add |pip install ",
            interrupt: false,
            message: "Before adding a dependency, check whether the project or its standard library already covers it. If you still want it, say what it buys us.",
        },
        Draft {
            group: "Rust",
            name: "no-unwrap",
            title: "No .unwrap() outside tests",
            why: "An unwrap is a panic waiting for the one input you didn't think of.",
            catches: ".unwrap() anywhere in an edit",
            example: r#"let port = config.get("port").unwrap();"#,
            pattern: r"\.unwrap\(\)",
            interrupt: true,
            message: "This project doesn't use `.unwrap()` outside tests — return a Result, or use `expect` with a reason that names the invariant you're relying on.",
        },
        Draft {
            group: "Rust",
            name: "no-allow-attributes",
            title: "Don't silence the linter",
            why: "An allow attribute hides the warning without answering it, and outlives whoever added it.",
            catches: "#[allow(...)], #![allow(...)]",
            example: "#[allow(dead_code)]",
            pattern: r"#!?\[allow\(",
            interrupt: false,
            message: "Don't silence a lint with an allow attribute. Fix what it's pointing at, or explain here why the lint is wrong.",
        },
        Draft {
            group: "TypeScript",
            name: "no-any",
            title: "No `any`",
            why: "One `any` disables checking for everything downstream of it.",
            catches: ": any, as any",
            example: "const payload: any = await res.json();",
            pattern: r":\s*any\b|as\s+any\b",
            interrupt: false,
            message: "Avoid `any` — give the real type, or `unknown` plus a narrowing check if you genuinely don't know it.",
        },
        Draft {
            group: "TypeScript",
            name: "no-stray-console-log",
            title: "No console.log left behind",
            why: "Debug output that ships is noise in someone else's terminal.",
            catches: "console.log(",
            example: r#"console.log("here", value);"#,
            pattern: r"console\.log\(",
            interrupt: false,
            message: "Remove the console.log before you finish, or switch it to the project's logger if it's worth keeping.",
        },
        Draft {
            group: "Python",
            name: "no-bare-except",
            title: "No bare except",
            why: "A bare except swallows KeyboardInterrupt and every bug you haven't met yet.",
            catches: "except: with no exception type",
            example: "except:",
            pattern: r"except\s*:",
            interrupt: false,
            message: "Catch the exception you mean — `except ValueError:` — rather than a bare `except:`.",
        },
    ]
    .into_iter()
    .map(Suggestion::from)
    .collect()
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
    fn every_suggestion_compiles_and_is_described() {
        for s in suggestions() {
            assert!(
                regex::Regex::new(&s.rule.pattern).is_ok(),
                "{} has an uncompilable pattern",
                s.rule.name
            );
            // A suggestion nobody can read is not a suggestion.
            assert!(!s.title.is_empty() && !s.why.is_empty() && !s.catches.is_empty());
            assert!(
                !s.rule.message.trim().is_empty(),
                "{} says nothing",
                s.rule.name
            );
            assert!(!s.rule.scope.is_empty(), "{} watches nothing", s.rule.name);
        }
    }

    #[test]
    fn every_suggestion_carries_a_sample_its_pattern_catches() {
        // The sample seeds the editor's tester. One that doesn't match would
        // greet you with "no match" on a rule you just added, which reads as a
        // broken rule rather than a mis-chosen example.
        for s in suggestions() {
            let sample = s
                .rule
                .sample
                .as_deref()
                .unwrap_or_else(|| panic!("{} has no sample", s.rule.name));
            assert!(
                regex::Regex::new(&s.rule.pattern).unwrap().is_match(sample),
                "{}'s sample `{sample}` doesn't match its own pattern",
                s.rule.name
            );
        }
    }

    #[test]
    fn suggested_patterns_catch_what_they_advertise() {
        let hits = |name: &str, sample: &str| {
            let s = suggestions()
                .into_iter()
                .find(|s| s.rule.name == name)
                .unwrap();
            regex::Regex::new(&s.rule.pattern).unwrap().is_match(sample)
        };
        assert!(hits("no-force-push", "git push --force origin main"));
        assert!(hits("no-force-push", "git push -f"));
        assert!(!hits("no-force-push", "git push origin main"));
        assert!(hits("no-unwrap", "let v = x.unwrap();"));
        assert!(!hits("no-unwrap", "let v = x?;"));
        assert!(hits(
            "no-hardcoded-secrets",
            r#"API_KEY = "xxxx-not-a-real-key""#
        ));
        assert!(!hits("no-hardcoded-secrets", r#"api_key = env("API_KEY")"#));
        assert!(hits("no-any", "const x: any = 1;"));
        assert!(hits("no-bare-except", "except:"));
        assert!(!hits("no-bare-except", "except ValueError:"));
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
                    prompt: None,
                    sample: None,
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
                    prompt: None,
                    sample: None,
                }],
            })
            .unwrap();
            load(tmp.path())
        });

        assert_eq!(specs.len(), 1, "one name, one rule");
        assert_eq!(specs[0].message, "from the project");
    }
}
