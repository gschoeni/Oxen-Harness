//! The `/permissions` command: show the active mode + rules, switch modes.

use anyhow::Result;
use harness_agent::Agent;
use harness_permissions::{policy, PermissionMode};

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// `/permissions [relaxed|cautious|bypass]` — show or switch the permission
/// mode. With no argument, prints the rules in force and opens the mode
/// picker (current mode marked). A switch applies to the live session
/// immediately (`PermissionGate::set_mode`) and persists as the global
/// default — mirroring `/compression`'s live-plus-persist shape.
pub(crate) fn handle_repl(rest: Option<String>, agent: &mut Agent, ui: &Ui) -> Result<()> {
    let gate = agent.permission_gate().cloned();
    let current = gate
        .as_ref()
        .map(|g| g.mode())
        .unwrap_or_else(|| policy::load_global().mode.unwrap_or_default());

    let choice = match rest {
        Some(arg) => arg,
        None => {
            print_summary(gate.as_deref(), current, ui);
            let mark = |m: PermissionMode| if m == current { "  ← current" } else { "" };
            let options = [
                Choice::new(
                    "relaxed",
                    format!(
                        "only dangerous/unparseable commands ask first{}",
                        mark(PermissionMode::Relaxed)
                    ),
                ),
                Choice::new(
                    "cautious",
                    format!(
                        "only read-only commands run unprompted; edits and commits ask too{}",
                        mark(PermissionMode::Cautious)
                    ),
                ),
                Choice::new(
                    "bypass",
                    format!(
                        "never ask — circuit breakers still refuse{}",
                        mark(PermissionMode::Bypass)
                    ),
                ),
            ];
            match picker::select(
                ui,
                "Permissions",
                &format!("Permission mode is `{}` — switch it?", current.label()),
                &options,
                false,
            )? {
                Some(sel) => sel.into_iter().next().unwrap_or_default(),
                // Cancelled (or no interactive terminal) — leave it untouched.
                None => return Ok(()),
            }
        }
    };

    let mode = match choice.trim().to_ascii_lowercase().as_str() {
        "relaxed" => PermissionMode::Relaxed,
        "cautious" => PermissionMode::Cautious,
        "bypass" => PermissionMode::Bypass,
        other => {
            println!(
                "  {} {}",
                ui.red("✗"),
                ui.dim(&format!(
                    "unknown mode `{other}` — expected relaxed, cautious, or bypass"
                )),
            );
            return Ok(());
        }
    };

    let scope = match &gate {
        // Live switch + persisted global default in one call.
        Some(gate) => {
            gate.set_mode(mode);
            "for this chat and new sessions"
        }
        None => {
            let _ = policy::persist_global_mode(mode);
            "for new sessions"
        }
    };
    println!(
        "  {} {}",
        ui.brown("🛡 permissions:"),
        ui.cream(&format!("{} — {scope}", mode.label())),
    );
    Ok(())
}

/// Print the mode and every allow/deny rule in force, with its scope.
fn print_summary(
    gate: Option<&harness_permissions::PermissionGate>,
    current: PermissionMode,
    ui: &Ui,
) {
    println!(
        "  {} {}",
        ui.brown("🛡 permissions:"),
        ui.cream(current.label()),
    );
    let global = policy::load_global();
    let project = gate.map(|g| policy::load_project(g.workspace()));
    let mut printed = false;
    for (scope, config) in [("global", Some(&global)), ("project", project.as_ref())] {
        let Some(config) = config else { continue };
        for (kind, values) in [
            ("deny", &config.deny),
            ("allow", &config.allow),
            ("allow exact", &config.allow_exact),
        ] {
            for value in values {
                println!(
                    "    {} {}  {}",
                    ui.accent(&format!("{kind:<11}")),
                    ui.cream(value),
                    ui.dim(&format!("({scope})")),
                );
                printed = true;
            }
        }
    }
    if !printed {
        println!(
            "    {}",
            ui.dim("no saved rules — approve a command with “always allow” to add one"),
        );
    }
    println!(
        "    {}",
        ui.dim("hard limits (rm -rf /, ~, .git writes) always refuse · audit: ~/.oxen-harness/permissions.jsonl"),
    );
}
