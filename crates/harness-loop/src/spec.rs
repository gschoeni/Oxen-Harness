//! The definition of a loop: a goal, a way to know when it's done, and a rule
//! for when to give up.
//!
//! A [`LoopSpec`] is the durable, shareable description of a job. It is
//! deliberately small — the heart of it is the list of [`Gate`]s, because a
//! loop without a real check is just an agent agreeing with itself on repeat.
//! Each gate carries a [`RunWhen`] condition, so expensive checks (the test
//! suite) only run on passes that actually touched matching files.

use serde::{Deserialize, Serialize};

/// Current loop file schema version. Bump on incompatible changes; files
/// written before versioning read back as this default.
pub const LOOP_SCHEMA_VERSION: u32 = 2;
fn default_loop_schema_version() -> u32 {
    LOOP_SCHEMA_VERSION
}

/// Default ceiling on iterations before a loop gives up and reports.
pub const DEFAULT_MAX_ITERATIONS: u32 = 8;
/// Default rubric pass threshold (each criterion must score at least this).
pub const DEFAULT_THRESHOLD: u8 = 8;
/// Default timeout for a verify command (5 minutes — builds/tests can be slow).
pub const DEFAULT_VERIFY_TIMEOUT_MS: u64 = 300_000;

fn default_max_iterations() -> u32 {
    DEFAULT_MAX_ITERATIONS
}
fn default_threshold() -> u8 {
    DEFAULT_THRESHOLD
}
fn default_timeout_ms() -> u64 {
    DEFAULT_VERIFY_TIMEOUT_MS
}

/// How a gate decides whether a pass succeeded — the check that turns
/// repetition into progress.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Verify {
    /// Run a shell command in the workspace; **exit code 0 means pass**. This is
    /// the strongest gate — an objective check the model can't talk itself past.
    Command {
        command: String,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// A separate, strict checker scores the work 1–10 against each success
    /// criterion; the pass succeeds only if every score is at least `threshold`.
    /// Softer than a command (the checker is still an LLM), but useful when
    /// "done" can't be reduced to an exit code.
    Rubric {
        #[serde(default = "default_threshold")]
        threshold: u8,
    },
}

impl Default for Verify {
    fn default() -> Self {
        Verify::Rubric {
            threshold: DEFAULT_THRESHOLD,
        }
    }
}

impl Verify {
    /// True if this gate runs an objective command (vs. an LLM rubric).
    pub fn is_command(&self) -> bool {
        matches!(self, Verify::Command { .. })
    }

    /// A short human label for the gate.
    pub fn label(&self) -> String {
        match self {
            Verify::Command { command, .. } => format!("command: {command}"),
            Verify::Rubric { threshold } => format!("rubric (≥{threshold}/10 on every criterion)"),
        }
    }
}

/// When a gate should run within a pass. In TOML: `run_when = "always"`,
/// `run_when = "on_change"` (any file the pass touched), or
/// `run_when = { on_change = ["**/*.rs"] }` (only matching files count).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RunWhenRepr", into = "RunWhenRepr")]
pub enum RunWhen {
    /// Run on every pass, whether or not anything changed.
    #[default]
    Always,
    /// Run only on passes that changed workspace files. An empty glob list
    /// means any change counts; otherwise at least one changed path must match.
    /// When change detection is unavailable (not a git repo, git error), the
    /// gate runs anyway — unknown fails safe toward checking.
    OnChange { paths: Vec<String> },
}

impl RunWhen {
    /// Decide whether the gate applies, given the paths this pass changed.
    /// `None` means change detection was unavailable — run the gate.
    pub fn should_run(&self, changed: Option<&[String]>) -> bool {
        match self {
            RunWhen::Always => true,
            RunWhen::OnChange { paths } => {
                let Some(changed) = changed else {
                    return true;
                };
                if changed.is_empty() {
                    return false;
                }
                if paths.is_empty() {
                    return true;
                }
                let mut builder = globset::GlobSetBuilder::new();
                let mut usable = false;
                for pattern in paths {
                    if let Ok(glob) = globset::Glob::new(pattern) {
                        builder.add(glob);
                        usable = true;
                    }
                }
                // Unusable globs fail safe: run rather than silently skip.
                if !usable {
                    return true;
                }
                match builder.build() {
                    Ok(set) => changed.iter().any(|p| set.is_match(p)),
                    Err(_) => true,
                }
            }
        }
    }

    /// A short human label for the condition ("every pass", "when **/*.rs change").
    pub fn label(&self) -> String {
        match self {
            RunWhen::Always => "every pass".to_string(),
            RunWhen::OnChange { paths } if paths.is_empty() => "when files change".to_string(),
            RunWhen::OnChange { paths } => format!("when {} change", paths.join(", ")),
        }
    }
}

/// Serde surface for [`RunWhen`]: a bare word or a `{ on_change = [...] }` table.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RunWhenRepr {
    Word(String),
    Globs { on_change: Vec<String> },
}

impl From<RunWhen> for RunWhenRepr {
    fn from(value: RunWhen) -> Self {
        match value {
            RunWhen::Always => RunWhenRepr::Word("always".to_string()),
            RunWhen::OnChange { paths } if paths.is_empty() => {
                RunWhenRepr::Word("on_change".to_string())
            }
            RunWhen::OnChange { paths } => RunWhenRepr::Globs { on_change: paths },
        }
    }
}

impl TryFrom<RunWhenRepr> for RunWhen {
    type Error = String;

    fn try_from(value: RunWhenRepr) -> Result<Self, Self::Error> {
        match value {
            RunWhenRepr::Word(w) => match w.as_str() {
                "always" => Ok(RunWhen::Always),
                "on_change" => Ok(RunWhen::OnChange { paths: Vec::new() }),
                other => Err(format!(
                    "unknown run_when `{other}` (expected \"always\", \"on_change\", \
                     or {{ on_change = [globs] }})"
                )),
            },
            RunWhenRepr::Globs { on_change } => Ok(RunWhen::OnChange { paths: on_change }),
        }
    }
}

/// One named check in a loop's verify sequence, with a condition for when it
/// applies. Gates run in order; the first failure stops the sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    /// Short identifier ("fmt", "tests") used in events, journals, and prompts.
    pub name: String,
    /// When this gate applies within a pass. Defaults to every pass.
    #[serde(default)]
    pub run_when: RunWhen,
    /// The actual check.
    pub verify: Verify,
}

impl Gate {
    /// One-line human label: name, check, and condition (if conditional).
    pub fn label(&self) -> String {
        match self.run_when {
            RunWhen::Always => format!("{} — {}", self.name, self.verify.label()),
            _ => format!(
                "{} — {} ({})",
                self.name,
                self.verify.label(),
                self.run_when.label()
            ),
        }
    }
}

/// A reusable, shareable loop definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopSpec {
    /// File-format version (see [`LOOP_SCHEMA_VERSION`]).
    #[serde(default = "default_loop_schema_version")]
    pub schema_version: u32,
    /// Human name (also the basis for the on-disk slug).
    pub name: String,
    /// One-line description for listings.
    #[serde(default)]
    pub description: String,
    /// The job: what should be true when the loop is done.
    pub goal: String,
    /// Concrete, strict criteria the work is checked against (no soft passes).
    #[serde(default)]
    pub success_criteria: Vec<String>,
    /// Legacy single gate from schema v1 files. Prefer `gates`; when present it
    /// resolves to one always-run gate named "verify".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<Verify>,
    /// The gates run (in order) after each pass. Empty + no legacy `verify`
    /// falls back to a single always-run rubric gate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<Gate>,
    /// Hard stop: give up and report after this many iterations.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// Optional hard stop on cumulative token spend (estimated by the agent).
    #[serde(default)]
    pub token_budget: Option<usize>,
}

impl LoopSpec {
    /// A minimal ad-hoc loop from a goal, using a rubric gate and defaults.
    pub fn from_goal(goal: impl Into<String>) -> Self {
        Self {
            schema_version: LOOP_SCHEMA_VERSION,
            name: "ad-hoc".to_string(),
            description: String::new(),
            goal: goal.into(),
            success_criteria: Vec::new(),
            verify: None,
            gates: Vec::new(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            token_budget: None,
        }
    }

    /// The effective gate sequence: `gates` when set, a legacy `verify` as a
    /// single always-run gate, or the default rubric gate as a last resort.
    pub fn resolved_gates(&self) -> Vec<Gate> {
        if !self.gates.is_empty() {
            return self.gates.clone();
        }
        let verify = self.verify.clone().unwrap_or_default();
        let name = if verify.is_command() {
            "verify"
        } else {
            "rubric"
        };
        vec![Gate {
            name: name.to_string(),
            run_when: RunWhen::Always,
            verify,
        }]
    }

    /// A one-line summary of the gate sequence for headers and listings.
    pub fn gate_summary(&self) -> String {
        self.resolved_gates()
            .iter()
            .map(Gate::label)
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// True if any effective gate is a model-scored rubric (vs. all commands).
    pub fn has_rubric_gate(&self) -> bool {
        self.resolved_gates().iter().any(|g| !g.verify.is_command())
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }
}

/// A normalized, filesystem-safe identifier derived from a loop's name.
///
/// Thin wrapper over [`harness_core::text::slug`] that pins the empty-name
/// fallback to `"loop"`.
pub fn slug(name: &str) -> String {
    harness_core::text::slug(name, "loop")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_in_when_omitted() {
        let spec: LoopSpec = toml::from_str(
            r#"
            name = "Tidy"
            goal = "make the repo tidy"
        "#,
        )
        .unwrap();
        assert_eq!(spec.max_iterations, DEFAULT_MAX_ITERATIONS);
        let gates = spec.resolved_gates();
        assert_eq!(gates.len(), 1);
        assert!(matches!(gates[0].verify, Verify::Rubric { threshold: 8 }));
        assert_eq!(gates[0].run_when, RunWhen::Always);
        assert!(spec.success_criteria.is_empty());
        assert!(spec.token_budget.is_none());
    }

    #[test]
    fn legacy_single_verify_resolves_to_one_always_gate() {
        let spec: LoopSpec = toml::from_str(
            r#"
            name = "Legacy"
            goal = "old format still works"

            [verify]
            type = "command"
            command = "cargo test"
        "#,
        )
        .unwrap();
        let gates = spec.resolved_gates();
        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].name, "verify");
        assert_eq!(gates[0].run_when, RunWhen::Always);
        assert!(gates[0].verify.is_command());
    }

    #[test]
    fn gates_round_trip_through_toml() {
        let spec = LoopSpec {
            schema_version: LOOP_SCHEMA_VERSION,
            name: "Green tests".into(),
            description: "all tests pass".into(),
            goal: "make the test suite green".into(),
            success_criteria: vec!["cargo test passes".into()],
            verify: None,
            gates: vec![
                Gate {
                    name: "fmt".into(),
                    run_when: RunWhen::OnChange { paths: Vec::new() },
                    verify: Verify::Command {
                        command: "cargo fmt --check".into(),
                        timeout_ms: 120_000,
                    },
                },
                Gate {
                    name: "tests".into(),
                    run_when: RunWhen::OnChange {
                        paths: vec!["**/*.rs".into()],
                    },
                    verify: Verify::Command {
                        command: "cargo test".into(),
                        timeout_ms: 600_000,
                    },
                },
            ],
            max_iterations: 10,
            token_budget: Some(500_000),
        };
        let toml = spec.to_toml().unwrap();
        let back = LoopSpec::from_toml_str(&toml).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn run_when_parses_word_and_glob_forms() {
        #[derive(Deserialize)]
        struct Holder {
            run_when: RunWhen,
        }
        let w: Holder = toml::from_str(r#"run_when = "always""#).unwrap();
        assert_eq!(w.run_when, RunWhen::Always);
        let w: Holder = toml::from_str(r#"run_when = "on_change""#).unwrap();
        assert_eq!(w.run_when, RunWhen::OnChange { paths: Vec::new() });
        let w: Holder = toml::from_str(r#"run_when = { on_change = ["**/*.rs"] }"#).unwrap();
        assert_eq!(
            w.run_when,
            RunWhen::OnChange {
                paths: vec!["**/*.rs".into()]
            }
        );
        assert!(toml::from_str::<Holder>(r#"run_when = "sometimes""#).is_err());
    }

    #[test]
    fn should_run_honors_changes_and_globs() {
        let always = RunWhen::Always;
        assert!(always.should_run(Some(&[])));

        let any = RunWhen::OnChange { paths: Vec::new() };
        assert!(!any.should_run(Some(&[])));
        assert!(any.should_run(Some(&["README.md".into()])));
        // Unknown changes fail safe toward running.
        assert!(any.should_run(None));

        let rust = RunWhen::OnChange {
            paths: vec!["**/*.rs".into(), "**/Cargo.toml".into()],
        };
        assert!(!rust.should_run(Some(&["README.md".into()])));
        assert!(rust.should_run(Some(&["src/main.rs".into()])));
        assert!(rust.should_run(Some(&["Cargo.toml".into()])));
        assert!(rust.should_run(None));
    }

    #[test]
    fn slugs_are_filesystem_safe() {
        assert_eq!(slug("Green Tests"), "green-tests");
        assert_eq!(slug("  Make  it!! green "), "make-it-green");
        assert_eq!(slug("***"), "loop");
    }
}
