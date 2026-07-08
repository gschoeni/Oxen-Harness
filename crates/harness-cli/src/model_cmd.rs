//! The `/model` command: show, switch, or add a model — plus the shared
//! model-row catalog the interactive picker and the composer's completion
//! list both render from, so the two surfaces can't drift.

use anyhow::Result;
use harness_agent::Agent;

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// One model the user can pick: a cloud-catalog entry or an installed local
/// model, described the same way everywhere it's shown.
pub(crate) struct ModelRow {
    /// The id to switch to (what `/model <id>` takes).
    pub(crate) id: String,
    /// Human-friendly display name (may be empty for bare cloud ids).
    pub(crate) title: String,
    /// Where it runs: `cloud built-in`, `cloud custom`, or `local · 8B · Q4`.
    pub(crate) source: String,
    /// Whether this is an installed local (llama.cpp) model.
    pub(crate) local: bool,
}

impl ModelRow {
    /// The `title · source` description, skipping an empty title so a bare id
    /// never renders a dangling separator.
    pub(crate) fn describe(&self) -> String {
        if self.title.is_empty() {
            self.source.clone()
        } else {
            format!("{} · {}", self.title, self.source)
        }
    }
}

/// Every model the user can pick — the cloud catalog plus installed local
/// models — sorted by id and deduplicated. The single source of the rows
/// behind the interactive `/model` picker and the live composer's
/// `/model <arg>` completion.
pub(crate) fn model_rows() -> Vec<ModelRow> {
    let mut rows: Vec<ModelRow> = harness_runtime::models::catalog()
        .into_iter()
        .map(|m| ModelRow {
            title: m.name,
            source: if m.builtin {
                "cloud built-in".to_string()
            } else {
                "cloud custom".to_string()
            },
            local: false,
            id: m.id,
        })
        .collect();
    if let Ok(store) = harness_local::ModelStore::open() {
        rows.extend(store.installed().into_iter().map(|m| {
            let meta = [m.params.as_str(), m.quant.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            ModelRow {
                title: m.display,
                source: if meta.is_empty() {
                    "local".to_string()
                } else {
                    format!("local · {meta}")
                },
                local: true,
                id: m.id,
            }
        }));
    }
    rows.sort_by_key(|r| r.id.to_lowercase());
    rows.dedup_by(|a, b| a.id == b.id);
    rows
}

/// `/model [id]` — switch models. With no argument, opens the interactive
/// picker over the cloud catalog + installed local models (current one
/// marked); its "type my own answer" row takes a brand-new id. An id we've
/// never seen is saved as a custom catalog entry so it shows up in the picker
/// from then on — here and in the desktop. The choice is persisted as the
/// default for future sessions.
pub(crate) fn handle_repl(rest: Option<String>, agent: &mut Agent, ui: &Ui) -> Result<()> {
    let rows = model_rows();
    let chosen = match rest {
        Some(id) => id,
        None => {
            let current = agent.model().to_string();
            let mark = |id: &str| if id == current { " ← current" } else { "" };
            let options: Vec<Choice> = rows
                .iter()
                .map(|r| Choice::new(r.id.clone(), format!("{}{}", r.describe(), mark(&r.id))))
                .collect();
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

    // A local model can't be swapped into a live cloud session (it needs its
    // own llama-server, started at launch). Persist it as the active local
    // model — the same switch the desktop dropdown makes — and say how to
    // ride it, instead of pointing the cloud client at a GGUF id.
    if rows.iter().any(|r| r.local && r.id == id) {
        match harness_runtime::models::set_active_local(id) {
            Ok(()) => {
                println!(
                    "  {} {}",
                    ui.brown("🐂 local oxen picked:"),
                    ui.cream(&format!("{id} — saved as your local model")),
                );
                println!(
                    "  {}",
                    ui.dim(&format!(
                        "restart to ride it: oxen-harness (or oxen-harness --local {id})"
                    )),
                );
            }
            Err(e) => println!("  {} {e}", ui.dim("couldn't save the local selection:")),
        }
        return Ok(());
    }

    agent.set_model(id);
    // An id we've never seen (not in the cloud catalog, not an installed local
    // model) is saved as a custom catalog entry so it shows up in the picker
    // from now on — here and in the desktop. Only a *cataloged* id is
    // persisted as the default: if the save fails, the live session still
    // switches, but the config never points at a model the catalog can't show.
    let known = rows.iter().any(|r| r.id == id);
    let cataloged = known
        || match harness_runtime::models::add(id, "") {
            Ok(_) => {
                println!(
                    "  {} {}",
                    ui.dim("new model saved to the catalog:"),
                    ui.cream(id)
                );
                true
            }
            Err(e) => {
                println!("  {} {e}", ui.dim("couldn't save to the catalog:"));
                false
            }
        };
    // Persist the choice (clearing any local selection) so it's the default
    // next launch — here and in the desktop dropdown.
    if cataloged {
        let _ = harness_runtime::models::set_selected(id);
    }
    println!("  {} {}", ui.brown("fresh oxen yoked:"), ui.accent(id));
    Ok(())
}
