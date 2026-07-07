//! Built-in loop presets that ship with the harness.
//!
//! These are starting points users can run as-is or fork (save one with the
//! same slug to override it). The flagship is the **default coding loop**: the
//! exact "make the checks green" job this project uses on itself. Its gates
//! are conditional — a pass that didn't touch Rust code (a commit, a docs
//! edit) skips clippy and the test suite instead of paying for them.

use crate::spec::{Gate, LoopSpec, RunWhen, Verify, LOOP_SCHEMA_VERSION};

/// The slug of the loop used when none is named.
pub const DEFAULT_SLUG: &str = "default";

/// Files that can affect Rust compilation or test results.
const RUST_CODE_GLOBS: &[&str] = &["**/*.rs", "**/Cargo.toml", "**/Cargo.lock"];

fn on_rust_change() -> RunWhen {
    RunWhen::OnChange {
        paths: RUST_CODE_GLOBS.iter().map(|s| s.to_string()).collect(),
    }
}

/// Every built-in loop, default first.
pub fn all() -> Vec<LoopSpec> {
    vec![default_coding_loop(), green_tests(), clean_clippy()]
}

/// Resolve a built-in by slug.
pub fn by_slug(slug: &str) -> Option<LoopSpec> {
    all()
        .into_iter()
        .find(|s| crate::spec::slug(&s.name) == slug)
}

/// The default loop: keep working until the project's full check suite is green.
/// This is the "Ralph Wiggum loop" gate this repo runs on itself.
pub fn default_coding_loop() -> LoopSpec {
    LoopSpec {
        schema_version: LOOP_SCHEMA_VERSION,
        name: "default".into(),
        description: "Work until formatting, lint, and tests are all green.".into(),
        goal: "Make the requested change and leave the project with formatting, \
               lint, and tests all passing."
            .into(),
        success_criteria: vec![
            "cargo fmt --check reports no changes".into(),
            "cargo clippy has zero warnings".into(),
            "cargo test passes with no failures".into(),
        ],
        verify: None,
        gates: vec![
            Gate {
                name: "fmt".into(),
                run_when: RunWhen::OnChange {
                    paths: vec!["**/*.rs".into()],
                },
                verify: Verify::Command {
                    command: "cargo fmt --all -- --check".into(),
                    timeout_ms: 120_000,
                },
            },
            Gate {
                name: "clippy".into(),
                run_when: on_rust_change(),
                verify: Verify::Command {
                    command: "cargo clippy --workspace --all-targets -- -D warnings".into(),
                    timeout_ms: 600_000,
                },
            },
            Gate {
                name: "tests".into(),
                run_when: on_rust_change(),
                verify: Verify::Command {
                    command: "cargo test --workspace".into(),
                    timeout_ms: 900_000,
                },
            },
        ],
        max_iterations: 8,
        token_budget: None,
    }
}

/// Just the test suite, green.
pub fn green_tests() -> LoopSpec {
    LoopSpec {
        schema_version: LOOP_SCHEMA_VERSION,
        name: "green-tests".into(),
        description: "Make the test suite pass.".into(),
        goal: "Make every test in the project pass.".into(),
        success_criteria: vec!["cargo test passes with no failures".into()],
        verify: None,
        gates: vec![Gate {
            name: "tests".into(),
            run_when: RunWhen::Always,
            verify: Verify::Command {
                command: "cargo test --workspace".into(),
                timeout_ms: 600_000,
            },
        }],
        max_iterations: 8,
        token_budget: None,
    }
}

/// Zero clippy warnings.
pub fn clean_clippy() -> LoopSpec {
    LoopSpec {
        schema_version: LOOP_SCHEMA_VERSION,
        name: "clean-clippy".into(),
        description: "Drive clippy to zero warnings.".into(),
        goal: "Make `cargo clippy` report zero warnings across the workspace.".into(),
        success_criteria: vec!["cargo clippy emits no warnings".into()],
        verify: None,
        gates: vec![Gate {
            name: "clippy".into(),
            run_when: RunWhen::Always,
            verify: Verify::Command {
                command: "cargo clippy --workspace --all-targets -- -D warnings".into(),
                timeout_ms: 600_000,
            },
        }],
        max_iterations: 6,
        token_budget: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_present_and_command_gated() {
        let d = by_slug(DEFAULT_SLUG).expect("default loop exists");
        let gates = d.resolved_gates();
        assert!(!gates.is_empty());
        assert!(gates.iter().all(|g| g.verify.is_command()));
        assert!(!d.success_criteria.is_empty());
    }

    #[test]
    fn default_gates_are_conditional_on_code_changes() {
        let d = default_coding_loop();
        for gate in d.resolved_gates() {
            assert!(
                matches!(gate.run_when, RunWhen::OnChange { .. }),
                "gate `{}` should be change-conditional",
                gate.name
            );
            // A commit-only or docs-only pass skips every default gate.
            assert!(!gate.run_when.should_run(Some(&[])));
            assert!(!gate.run_when.should_run(Some(&["README.md".to_string()])));
            // A Rust edit triggers them all.
            assert!(gate.run_when.should_run(Some(&["src/lib.rs".to_string()])));
        }
    }

    #[test]
    fn builtin_slugs_are_unique() {
        let mut slugs: Vec<String> = all().iter().map(|s| crate::spec::slug(&s.name)).collect();
        slugs.sort();
        let len = slugs.len();
        slugs.dedup();
        assert_eq!(slugs.len(), len, "built-in loop slugs must be unique");
    }
}
