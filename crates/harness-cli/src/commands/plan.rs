//! The `/plan` command: plan mode.
//!
//! While plan mode is on the permission gate holds the working tree read-only
//! (see `harness_permissions::PermissionGate::set_plan_mode`) and the model
//! researches, then writes an execution plan to `.oxen-harness/plans/` — the
//! only path it can still write. Approving that file turns the latch off and
//! runs one turn with the plan as the authoritative brief.
//!
//! The mode is a *live* latch, never persisted: a plan belongs to the
//! conversation that produced it, and a new session should never start with
//! the tree silently locked.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use harness_agent::{plan_approved_prompt, Agent, PLAN_MODE_ENTER, PLAN_MODE_EXIT};

use crate::theme::Ui;

/// Plan mode's one writable directory, relative to the workspace root. Must
/// match the gate's own allowance in `harness-permissions`.
const PLANS_DIR: &str = ".oxen-harness/plans";

/// What the user asked `/plan` to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// `/plan` or `/plan on` — lock the tree and start researching.
    On,
    /// `/plan off` — unlock without approving anything.
    Off,
    /// `/plan approve [path]` — execute the plan (newest one by default).
    Approve(Option<String>),
    /// `/plan show [path]` — print the plan without running it.
    Show(Option<String>),
    /// Anything else, kept verbatim for the error message.
    Unknown(String),
}

/// Parse the raw remainder after `/plan`. A path keeps its spaces, so only the
/// first word is treated as the subcommand.
pub(crate) fn parse(rest: Option<&str>) -> Action {
    let rest = rest.map(str::trim).unwrap_or_default();
    if rest.is_empty() {
        return Action::On;
    }
    let (word, tail) = match rest.split_once(char::is_whitespace) {
        Some((word, tail)) => (word, Some(tail.trim()).filter(|t| !t.is_empty())),
        None => (rest, None),
    };
    let tail = tail.map(str::to_string);
    match word.to_ascii_lowercase().as_str() {
        "on" | "start" => Action::On,
        "off" | "stop" | "exit" => Action::Off,
        "approve" | "go" | "accept" => Action::Approve(tail),
        "show" | "cat" => Action::Show(tail),
        _ => Action::Unknown(rest.to_string()),
    }
}

/// `/plan [on|off|approve [path]|show [path]]`.
///
/// Returns the prompt the REPL should run next — `Some` only for an approved
/// plan, which executes through the same path a typed prompt takes.
pub(crate) fn handle_repl(
    rest: Option<String>,
    agent: &mut Agent,
    ui: &Ui,
    workspace: &Path,
) -> Result<Option<String>> {
    let action = parse(rest.as_deref());
    // Every action but `show` needs the gate: without one, "plan mode" would
    // be an instruction the model could simply ignore, which is worse than
    // saying so.
    let gate = agent.permission_gate().cloned();
    if gate.is_none() && !matches!(action, Action::Show(_)) {
        println!(
            "  {} {}",
            ui.red("✗"),
            ui.dim("plan mode needs the permission gate, and this session has none"),
        );
        return Ok(None);
    }

    match action {
        Action::On => {
            if let Some(gate) = &gate {
                gate.set_plan_mode(true);
            }
            agent.inject_exchange(
                PLAN_MODE_ENTER,
                "Understood — plan mode is on. I'll explore read-only and write the plan to \
                 `.oxen-harness/plans/` before touching anything.",
            )?;
            println!(
                "  {} {}",
                ui.brown("🗺 plan mode: on"),
                ui.dim("— the tree is read-only; ask for a plan, then /plan approve to run it"),
            );
        }
        Action::Off => {
            if let Some(gate) = &gate {
                gate.set_plan_mode(false);
            }
            agent.inject_exchange(
                PLAN_MODE_EXIT,
                "Understood — plan mode is off. I won't start any work until you ask.",
            )?;
            println!(
                "  {} {}",
                ui.brown("🗺 plan mode: off"),
                ui.dim("— the tree is writable again; no plan was approved"),
            );
        }
        Action::Show(path) => {
            let Some(path) = resolve(workspace, path.as_deref()) else {
                print_no_plan(ui);
                return Ok(None);
            };
            let text = read_plan(&path)?;
            println!(
                "  {}",
                ui.brown(&format!("🗺 {}", display(workspace, &path)))
            );
            println!("{}", ui.cream(text.trim_end()));
        }
        Action::Approve(path) => {
            let Some(path) = resolve(workspace, path.as_deref()) else {
                print_no_plan(ui);
                return Ok(None);
            };
            let text = read_plan(&path)?;
            let shown = display(workspace, &path);
            // Unlatch first: the approved turn is the work, and it must not
            // trip over the mode that produced the plan.
            if let Some(gate) = &gate {
                gate.set_plan_mode(false);
            }
            println!(
                "  {} {}",
                ui.green("🗺 plan approved:"),
                ui.cream(&format!("{shown} — executing")),
            );
            return Ok(Some(plan_approved_prompt(&shown, &text)));
        }
        Action::Unknown(rest) => println!(
            "  {} {}",
            ui.red("✗"),
            ui.dim(&format!(
                "unknown `/plan {rest}` — expected on, off, approve [path], or show [path]"
            )),
        ),
    }
    Ok(None)
}

/// The plan file to act on: the given path (relative to the workspace, or
/// absolute), else the most recently modified file in the plans directory.
pub(crate) fn resolve(workspace: &Path, given: Option<&str>) -> Option<PathBuf> {
    match given {
        Some(given) => {
            let path = Path::new(given);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                workspace.join(path)
            };
            path.is_file().then_some(path)
        }
        None => newest_plan(&workspace.join(PLANS_DIR)),
    }
}

/// The newest plan on disk. Ties break on the file name, so the answer is
/// stable when two plans share a timestamp (coarse filesystem clocks make that
/// likelier than it sounds).
pub(crate) fn newest_plan(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|(best_time, best_path)| (modified, &path) > (*best_time, best_path))
        {
            best = Some((modified, path));
        }
    }
    best.map(|(_, path)| path)
}

fn read_plan(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

/// A plan path as the user (and the model) should see it: workspace-relative.
fn display(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn print_no_plan(ui: &Ui) {
    println!(
        "  {} {}",
        ui.red("✗"),
        ui.dim(&format!(
            "no plan found — `/plan` first, then let the model write one to {PLANS_DIR}/"
        )),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_parse_into_actions() {
        assert_eq!(parse(None), Action::On);
        assert_eq!(parse(Some("  ")), Action::On);
        assert_eq!(parse(Some("on")), Action::On);
        assert_eq!(parse(Some("OFF")), Action::Off);
        assert_eq!(parse(Some("approve")), Action::Approve(None));
        assert_eq!(
            parse(Some("approve plans/my plan.md")),
            Action::Approve(Some("plans/my plan.md".into()))
        );
        assert_eq!(parse(Some("show")), Action::Show(None));
        assert_eq!(parse(Some("show a.md")), Action::Show(Some("a.md".into())));
        assert_eq!(
            parse(Some("frobnicate")),
            Action::Unknown("frobnicate".into())
        );
    }

    #[test]
    fn the_newest_plan_wins_and_an_explicit_path_overrides_it() {
        let ws = tempfile::tempdir().unwrap();
        let dir = ws.path().join(PLANS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(resolve(ws.path(), None), None);

        // Written oldest-first, then stamped explicitly: relying on the clock
        // between two writes is exactly the flake this test would hide.
        let older = dir.join("older.md");
        let newer = dir.join("newer.md");
        std::fs::write(&older, "# older").unwrap();
        std::fs::write(&newer, "# newer").unwrap();
        let stamp = |path: &Path, secs: u64| {
            let file = std::fs::File::options().write(true).open(path).unwrap();
            let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
            file.set_times(std::fs::FileTimes::new().set_modified(when))
                .unwrap();
        };
        stamp(&newer, 1_700_000_000);
        stamp(&older, 1_600_000_000);
        assert_eq!(resolve(ws.path(), None), Some(newer.clone()));

        // A subdirectory is not a plan; an explicit path (relative or
        // absolute) wins over the newest one; a missing path resolves to None.
        std::fs::create_dir(dir.join("archive")).unwrap();
        assert_eq!(resolve(ws.path(), None), Some(newer));
        assert_eq!(
            resolve(ws.path(), Some(".oxen-harness/plans/older.md")),
            Some(older.clone())
        );
        assert_eq!(
            resolve(ws.path(), Some(&older.display().to_string())),
            Some(older)
        );
        assert_eq!(resolve(ws.path(), Some("nope.md")), None);
    }
}
