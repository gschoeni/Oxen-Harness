//! Built-in loop presets that ship with the harness.
//!
//! These are starting points users can run as-is or fork (save one with the
//! same slug to override it). The flagship is the **default coding loop**: the
//! exact "make the checks green" job this project uses on itself.

use crate::spec::{LoopSpec, Verify, LOOP_SCHEMA_VERSION};

/// The slug of the loop used when none is named.
pub const DEFAULT_SLUG: &str = "default";

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
        verify: Verify::Command {
            command: "cargo fmt --all -- --check && cargo clippy --workspace --all-targets \
                      -- -D warnings && cargo test --workspace"
                .into(),
            timeout_ms: 900_000,
        },
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
        verify: Verify::Command {
            command: "cargo test --workspace".into(),
            timeout_ms: 600_000,
        },
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
        verify: Verify::Command {
            command: "cargo clippy --workspace --all-targets -- -D warnings".into(),
            timeout_ms: 600_000,
        },
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
        assert!(d.verify.is_command());
        assert!(!d.success_criteria.is_empty());
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
