//! The `/model` command: show, switch, or add a model.

use anyhow::Result;
use harness_agent::Agent;

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// `/model [id]` — switch models. With no argument, opens the interactive
/// picker over the cloud catalog + installed local models (current one
/// marked); its "type my own answer" row takes a brand-new id. An id we've
/// never seen is saved as a custom catalog entry so it shows up in the picker
/// from then on — here and in the desktop. The choice is persisted as the
/// default for future sessions.
pub(crate) fn handle_repl(rest: Option<String>, agent: &mut Agent, ui: &Ui) -> Result<()> {
    let chosen = match rest {
        Some(id) => id,
        None => {
            let current = agent.model().to_string();
            let options = model_choices(&current);
            let question = format!("Yoked to `{current}` — trade for another?");
            match picker::select(ui, "Model", &question, &options, false)? {
                Some(sel) => sel.into_iter().next().unwrap_or_default(),
                // Cancelled, or no interactive terminal (piped input) — just
                // report the current model like `/model` always did there.
                None => {
                    println!("  {} {}", ui.brown("oxen yoked:"), ui.cream(agent.model()));
                    return Ok(());
                }
            }
        }
    };

    let id = chosen.trim();
    if id.is_empty() {
        println!("  {} {}", ui.brown("oxen yoked:"), ui.cream(agent.model()));
        return Ok(());
    }

    agent.set_model(id);
    // An id we've never seen (not in the cloud catalog, not an installed local
    // model) is saved as a custom catalog entry so it shows up in the picker
    // from now on — here and in the desktop.
    let known_cloud = harness_runtime::models::catalog()
        .iter()
        .any(|c| c.id == id);
    let known_local = harness_local::ModelStore::open()
        .map(|s| s.installed().iter().any(|l| l.id == id))
        .unwrap_or(false);
    if !known_cloud && !known_local {
        match harness_runtime::models::add(id, "") {
            Ok(_) => println!(
                "  {} {}",
                ui.dim("new model saved to the catalog:"),
                ui.cream(id)
            ),
            Err(e) => println!("  {} {e}", ui.dim("couldn't save to the catalog:")),
        }
    }
    // Persist the choice (clearing any local selection) so it's the default
    // next launch — here and in the desktop dropdown.
    let _ = harness_runtime::models::set_selected(id);
    println!("  {} {}", ui.brown("fresh oxen yoked:"), ui.accent(id));
    Ok(())
}

/// The rows for the interactive `/model` picker: the cloud catalog plus
/// installed local models, sorted by id with the session's current model
/// marked. Mirrors the live composer's `/model <arg>` completion list.
fn model_choices(current: &str) -> Vec<Choice> {
    let mark = |id: &str| if id == current { " ← current" } else { "" };
    let mut choices: Vec<Choice> = harness_runtime::models::catalog()
        .into_iter()
        .map(|m| {
            let source = if m.builtin {
                "cloud built-in"
            } else {
                "cloud custom"
            };
            let desc = if m.name.is_empty() {
                format!("{source}{}", mark(&m.id))
            } else {
                format!("{} · {source}{}", m.name, mark(&m.id))
            };
            Choice::new(m.id, desc)
        })
        .collect();
    if let Ok(store) = harness_local::ModelStore::open() {
        choices.extend(store.installed().into_iter().map(|m| {
            let meta = [m.params.as_str(), m.quant.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            let source = if meta.is_empty() {
                "local".to_string()
            } else {
                format!("local · {meta}")
            };
            let desc = format!("{} · {source}{}", m.display, mark(&m.id));
            Choice::new(m.id, desc)
        }));
    }
    choices.sort_by_key(|c| c.label.to_lowercase());
    choices.dedup_by(|a, b| a.label == b.label);
    choices
}
