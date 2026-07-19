//! The `/location` command (and its themed alias `/departing`): set, show, or
//! clear where you're riding from.
//!
//! The value is user data, not theme data: it persists in
//! `~/.oxen-harness/config.toml` via the theme store and is overlaid onto the
//! active theme's first flavor row wherever that theme loads — the CLI
//! banner's "Departing :" line here, and the hero screen's status rows in the
//! desktop app. It survives restarts and theme switches on both front ends.
//!
//! A bare `/location` opens an interactive card (like the `/model` picker)
//! that says what the setting does and takes the new place as free text — no
//! need to know the `/location <place>` form up front. `/location <place>` and
//! `/location clear` still work directly, and remain the whole story for
//! piped/non-interactive sessions.

use std::sync::Arc;

use anyhow::Result;
use harness_agent::Agent;

use crate::picker::{self, Choice};
use crate::repl_loop::ReplContext;
use crate::theme::{self, Ui};

/// Arguments that clear the saved location instead of setting it.
const CLEAR_WORDS: &[&str] = &["clear", "reset", "unset"];

/// What the user asked to do with the location.
enum Action {
    Set(String),
    Clear,
    Keep,
}

/// `/location [place|clear]` — configure the location shown on the banner and
/// the desktop hero. Bare `/location` opens the interactive card; a set/clear
/// repaints the welcome banner so the new row shows in context.
pub(crate) async fn handle_repl(
    rest: Option<String>,
    agent: &Agent,
    ui: &mut Ui,
    ctx: &ReplContext<'_>,
) -> Result<()> {
    let store = match harness_theme::Store::open() {
        Ok(store) => store,
        Err(e) => {
            println!("  {} {e}", ui.dim("couldn't open the theme store:"));
            return Ok(());
        }
    };

    let action = match rest {
        Some(arg) if CLEAR_WORDS.iter().any(|w| arg.eq_ignore_ascii_case(w)) => Action::Clear,
        Some(arg) => Action::Set(arg),
        None => prompt(ui, &store)?,
    };

    match action {
        Action::Keep => show_current(ui),
        Action::Clear => {
            if let Err(e) = store.set_location(None) {
                println!("  {} {e}", ui.dim("couldn't clear the location:"));
                return Ok(());
            }
            // Reload the active theme so the row reverts to its themed default.
            *ui = ui.with_theme(Arc::new(store.load_active()));
            reprint_banner(agent, ui, ctx).await;
            println!(
                "  {} {}",
                ui.green("⛺ location cleared:"),
                ui.cream("back to the theme's own departure point"),
            );
        }
        Action::Set(place) => {
            if let Err(e) = store.set_location(Some(&place)) {
                println!("  {} {e}", ui.dim("couldn't save the location:"));
                return Ok(());
            }
            let label = ui.set_departing(&place);
            reprint_banner(agent, ui, ctx).await;
            println!(
                "  {} {}",
                ui.green(&format!("⛺ {label} set:")),
                ui.cream(&place),
            );
            println!(
                "  {}",
                ui.dim("saved — it'll greet you here and on the desktop hero screen"),
            );
        }
    }
    Ok(())
}

/// The interactive card for a bare `/location`: explains what the setting
/// does, offers the current/theme-default rows, and takes a new place as free
/// text. Falls back to showing the current value when there's no interactive
/// terminal or the user cancels.
fn prompt(ui: &Ui, store: &harness_theme::Store) -> Result<Action> {
    let saved = store.location();
    let theme_default = theme_default(store);
    let label = ui.departing().map(|(l, _)| l.to_string());

    let question = format!(
        "Where does your trail begin? It shows as the \"{}\" line on the \
         terminal welcome banner and on the desktop app's hero screen. \
         Type a place below, or pick a row.",
        label.as_deref().unwrap_or("Location"),
    );
    let options = menu(saved.as_deref(), theme_default.as_deref());
    match picker::select(ui, "Location", &question, &options, false)? {
        Some(sel) => {
            let picked = sel.into_iter().next().unwrap_or_default();
            Ok(action_for(
                &picked,
                saved.as_deref(),
                theme_default.as_deref(),
            ))
        }
        // Cancelled, or no interactive terminal (piped input) — report the
        // current value and how to set one non-interactively.
        None => Ok(Action::Keep),
    }
}

/// The active theme's own first-flavor-row value (what the banner shows when
/// no location is saved), read from the theme *without* the user's overlay.
fn theme_default(store: &harness_theme::Store) -> Option<String> {
    store
        .resolve(&store.active_slug())
        .ok()?
        .voice
        .flavor_top
        .first()
        .map(|row| row[1].clone())
}

/// The picker rows for the current state: the saved location (kept when
/// re-picked) and the theme's own line (which clears the saved one). The
/// picker's free-text row supplies new places.
fn menu(saved: Option<&str>, theme_default: Option<&str>) -> Vec<Choice> {
    let mut options = Vec::new();
    if let Some(current) = saved.filter(|s| Some(*s) != theme_default) {
        options.push(Choice::new(current, "current — keep it"));
    }
    match theme_default {
        Some(default) => options.push(Choice::new(
            default,
            if saved.is_some() {
                "the theme's own line (clears your saved location)"
            } else {
                "current — the theme's own line"
            },
        )),
        None if saved.is_some() => options.push(Choice::new("No location", "clear the saved one")),
        // Nothing saved and no themed line: the free-text row is the menu.
        None => options.push(Choice::new("No location", "leave the banner without one")),
    }
    options
}

/// Map a picked label back to an action against the current state.
fn action_for(picked: &str, saved: Option<&str>, theme_default: Option<&str>) -> Action {
    if picked.trim().is_empty() {
        return Action::Keep;
    }
    // Picking the theme's own line (or "No location") clears the override —
    // checked before "keep" so a saved value equal to the default still clears
    // and tracks future theme switches.
    if Some(picked) == theme_default || picked == "No location" {
        return if saved.is_some() {
            Action::Clear
        } else {
            Action::Keep
        };
    }
    if Some(picked) == saved {
        return Action::Keep;
    }
    Action::Set(picked.to_string())
}

fn show_current(ui: &Ui) {
    match ui.departing() {
        Some((label, value)) => {
            println!("  {} {}", ui.brown(&format!("{label}:")), ui.cream(value))
        }
        None => println!("  {}", ui.dim("no location set")),
    }
    println!(
        "  {}",
        ui.dim("set one with /location <place> · revert to the theme's with /location clear")
    );
}

/// Repaint the welcome banner so the changed row shows in context.
async fn reprint_banner(agent: &Agent, ui: &Ui, ctx: &ReplContext<'_>) {
    // The banner shows the all-time grand total spend across every model and
    // project (best-effort; unavailable reads as `None` → "—").
    let cost = crate::commands::usage::total_cost_usd(ctx.store).await;
    print!(
        "{}",
        theme::banner(
            ui,
            agent.base_url(),
            agent.model(),
            &ctx.workspace_root.display().to_string(),
            ctx.session,
            crate::commands::usage::total_tokens(ctx.store),
            cost,
        )
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_offers_current_and_theme_default_rows() {
        let opts = menu(Some("Fort Laramie"), Some("Independence, Missouri · 1848"));
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].label, "Fort Laramie");
        assert!(opts[0].description.contains("current"));
        assert_eq!(opts[1].label, "Independence, Missouri · 1848");
        assert!(opts[1].description.contains("clears"));
    }

    #[test]
    fn menu_collapses_when_saved_equals_the_default() {
        let opts = menu(Some("Independence"), Some("Independence"));
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].label, "Independence");
    }

    #[test]
    fn menu_without_a_themed_line_still_has_a_row() {
        assert_eq!(menu(None, None).len(), 1);
        assert_eq!(menu(Some("Somewhere"), None).len(), 2);
    }

    #[test]
    fn actions_map_back_to_set_clear_keep() {
        let saved = Some("Fort Laramie");
        let default = Some("Independence, Missouri · 1848");
        assert!(matches!(
            action_for("Chimney Rock", saved, default),
            Action::Set(p) if p == "Chimney Rock"
        ));
        assert!(matches!(
            action_for("Fort Laramie", saved, default),
            Action::Keep
        ));
        assert!(matches!(
            action_for("Independence, Missouri · 1848", saved, default),
            Action::Clear
        ));
        // Nothing saved: re-picking the default is a no-op, not a write.
        assert!(matches!(
            action_for("Independence, Missouri · 1848", None, default),
            Action::Keep
        ));
        // A saved value equal to the default clears so it tracks the theme.
        assert!(matches!(
            action_for("Independence", Some("Independence"), Some("Independence")),
            Action::Clear
        ));
        assert!(matches!(
            action_for("No location", Some("Somewhere"), None),
            Action::Clear
        ));
        assert!(matches!(action_for("", saved, default), Action::Keep));
    }
}
