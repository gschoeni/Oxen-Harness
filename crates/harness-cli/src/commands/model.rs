//! The `/model` command: show, switch, or add a model — plus the shared
//! model-row catalog the interactive picker and the composer's completion
//! list both render from, so the two surfaces can't drift.

use anyhow::Result;
use harness_agent::Agent;
use harness_local::source::ModelPricing;

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// One model the user can pick: a cloud-catalog entry or an installed local
/// model, described the same way everywhere it's shown.
pub(crate) struct ModelRow {
    /// The id to switch to (what `/model <id>` takes).
    pub(crate) id: String,
    /// Human-friendly display name (may be empty for bare cloud ids).
    pub(crate) title: String,
    /// Where it runs: `cloud` or `local · 8B · Q4`.
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
            source: "cloud".to_string(),
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

/// A short price tag for a model, shown in the picker so the cost is visible
/// at a glance: `free` for local (llama.cpp) models — they run on your own
/// hardware — and a per-million-token rate like `$3/M in · $15/M out` for a
/// cloud model the endpoint catalog prices. A cloud model the catalog doesn't
/// list yields `None` (no tag) rather than implying it's free.
fn price_tag(
    row: &ModelRow,
    pricing: Option<&std::collections::HashMap<String, ModelPricing>>,
) -> Option<String> {
    if row.local {
        return Some("free".to_string());
    }
    let rate = pricing?.get(&row.id)?;
    crate::pricing::format_rate(rate)
}

/// Fetch the active endpoint's pricing catalog for the picker. Best-effort: a
/// failed request just means no price tags, and local models are free
/// regardless.
async fn pricing_catalog() -> Option<std::collections::HashMap<String, ModelPricing>> {
    let connection = harness_runtime::connection::load();
    let base_url = harness_runtime::connection::effective_base_url(&connection);
    let token = harness_runtime::connection::effective_api_key(&base_url);
    harness_local::source::oxen_model_pricing_catalog_at(
        &base_url,
        (!token.trim().is_empty()).then_some(token.as_str()),
    )
    .await
    .ok()
}

/// `/model [id]` — switch models. With no argument, opens the interactive
/// picker over the cloud catalog + installed local models (current one
/// marked, each with its price — free for local, per-million-token rates for
/// cloud); its "type my own answer" row takes a brand-new id. An id we've
/// never seen is saved as a custom catalog entry so it shows up in the picker
/// from then on — here and in the desktop. The choice is persisted as the
/// default for future sessions.
pub(crate) async fn handle_repl(rest: Option<String>, agent: &mut Agent, ui: &Ui) -> Result<()> {
    // The current-model readout, with its cached per-million rate when known
    // (warmed at startup/turn boundaries) — price stays visible even when the
    // user just asks what they're riding.
    let yoked = |model: &str| {
        let rate = crate::pricing::session_rate(model)
            .and_then(|r| crate::pricing::format_rate(&r))
            .map(|r| format!(" · {r}"))
            .unwrap_or_default();
        println!(
            "  {} {}{}",
            ui.brown("oxen yoked:"),
            ui.cream(model),
            ui.dim(&rate)
        );
    };
    let rows = model_rows();
    let chosen = match rest {
        Some(id) => id,
        None => {
            let current = agent.model().to_string();
            let mark = |id: &str| if id == current { " ← current" } else { "" };
            // One catalog request feeds every row's price tag.
            let pricing = pricing_catalog().await;
            let options: Vec<Choice> = rows
                .iter()
                .map(|r| {
                    let price = price_tag(r, pricing.as_ref())
                        .map(|p| format!(" · {p}"))
                        .unwrap_or_default();
                    Choice::new(
                        r.id.clone(),
                        format!("{}{price}{}", r.describe(), mark(&r.id)),
                    )
                })
                .collect();
            let question = format!("Yoked to `{current}` — trade for another?");
            match picker::select(ui, "Model", &question, &options, false)? {
                Some(sel) => sel.into_iter().next().unwrap_or_default(),
                // Cancelled, or no interactive terminal (piped input) — just
                // report the current model like `/model` always did there.
                None => {
                    yoked(agent.model());
                    return Ok(());
                }
            }
        }
    };

    let id = chosen.trim();
    if id.is_empty() {
        yoked(agent.model());
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
    // The new model's catalog-reported limits, when cached; `None` falls back
    // to the name-derived window and the configured reply reserve.
    agent.set_context_window(harness_local::limits::context_window(id));
    agent.set_max_output_tokens(harness_local::limits::max_output_tokens(id));
    // Follow the swap through to the fleet spawner so a later spawn_agents
    // fleet runs on the new model, not the one captured at startup.
    crate::endpoint::update_fleet_endpoint(None, Some(id));
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
    // Price transparency on every switch: warm the shared rate cache for the
    // new model (also feeds the context trailer + completion picker) and show
    // what it costs right in the confirmation.
    crate::pricing::warm_for(id).await;
    let rate = crate::pricing::session_rate(id)
        .and_then(|r| crate::pricing::format_rate(&r))
        .map(|r| format!(" · {r}"))
        .unwrap_or_default();
    println!(
        "  {} {}{}",
        ui.brown("fresh oxen yoked:"),
        ui.accent(id),
        ui.dim(&rate)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn row(id: &str, local: bool) -> ModelRow {
        ModelRow {
            id: id.to_string(),
            title: id.to_string(),
            source: if local {
                "local".into()
            } else {
                "cloud".into()
            },
            local,
        }
    }

    #[test]
    fn local_models_are_free() {
        // Local models run on your own hardware — always free, no catalog lookup.
        let tag = price_tag(&row("qwen3-8b", true), None);
        assert_eq!(tag.as_deref(), Some("free"));
    }

    #[test]
    fn cloud_model_shows_its_catalog_rate() {
        let mut catalog = HashMap::new();
        catalog.insert(
            "muse-spark-1".to_string(),
            ModelPricing {
                input_cost_per_token: 0.000_003,
                output_cost_per_token: 0.000_015,
            },
        );
        let tag = price_tag(&row("muse-spark-1", false), Some(&catalog));
        assert_eq!(tag.as_deref(), Some("$3/M in · $15/M out"));
    }

    #[test]
    fn cloud_model_absent_from_catalog_has_no_tag() {
        // Unlisted (or no catalog at all) → no tag, rather than implying free.
        let catalog = HashMap::new();
        assert!(price_tag(&row("mystery-model", false), Some(&catalog)).is_none());
        assert!(price_tag(&row("mystery-model", false), None).is_none());
    }
}
