//! `/usage` — all-time per-model token throughput and estimated Oxen spend.

use std::sync::Arc;

use harness_core::fmt::human_tokens;
use harness_llm::ChatMessage;
use harness_store::{HistoryStore, ModelUsage};

use crate::theme::{format_usd, Ui};

struct PricedUsage {
    usage: ModelUsage,
    cost: Option<f64>,
}

async fn priced_rows(store: &HistoryStore) -> Vec<PricedUsage> {
    let Ok(usage) = store.model_usage_breakdown() else {
        return Vec::new();
    };
    let token = harness_config::secrets::get("OXEN_API_KEY").filter(|t| !t.trim().is_empty());
    let pricing = harness_local::source::oxen_model_pricing_catalog(token.as_deref())
        .await
        .ok();
    let mut rows = Vec::with_capacity(usage.len());
    for usage in usage {
        let cost = if usage.source == "oxen_cloud" {
            pricing
                .as_ref()
                .and_then(|catalog| catalog.get(&usage.model))
                .map(|p| {
                    p.cost_of(
                        usage.prompt_tokens.max(0) as usize,
                        usage.completion_tokens.max(0) as usize,
                    )
                })
        } else {
            None
        };
        rows.push(PricedUsage { usage, cost });
    }
    rows.sort_by(|a, b| {
        b.cost
            .partial_cmp(&a.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.usage.model.cmp(&b.usage.model))
    });
    rows
}

/// Known Oxen-cloud cost across every recorded model. Returns `None` when
/// there is cloud usage but its catalog pricing could not be resolved.
pub(crate) async fn total_cost_usd(store: &HistoryStore) -> Option<f64> {
    let rows = priced_rows(store).await;
    let has_cloud = rows.iter().any(|r| r.usage.source == "oxen_cloud");
    let costs: Vec<f64> = rows.iter().filter_map(|r| r.cost).collect();
    if has_cloud && costs.is_empty() {
        None
    } else {
        Some(costs.into_iter().sum())
    }
}

/// All-time token throughput. Existing transcripts are estimated once as a
/// migration baseline; model calls then maintain the persisted counter with
/// provider-reported counts.
pub(crate) fn total_tokens(store: &HistoryStore) -> usize {
    const KEY: &str = "total_tokens_used";
    if let Ok(Some(total)) = store.meta_get_i64(KEY) {
        return total.max(0) as usize;
    }
    let estimated = store
        .list_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|session| store.messages(&session.id).ok())
        .map(|raw| {
            let messages: Vec<ChatMessage> = raw
                .into_iter()
                .filter_map(|value| serde_json::from_value(value).ok())
                .collect();
            harness_agent::budget::estimate_prompt_tokens(&messages, &[])
        })
        .sum::<usize>();
    let _ = store.meta_set_i64(KEY, estimated as i64);
    estimated
}

pub(crate) async fn handle_repl(store: &Arc<HistoryStore>, ui: &Ui) {
    let rows = priced_rows(store).await;
    if rows.is_empty() {
        println!("  {}", ui.dim("No model usage recorded yet."));
        return;
    }

    println!("  {}", ui.brown("MODEL USAGE"));
    println!(
        "  {}",
        ui.dim(&format!(
            "{:<30} {:>10} {:>10} {:>12}",
            "model", "input", "output", "cost"
        ))
    );
    let mut total = 0.0;
    for row in &rows {
        let cost = row.cost.map(format_usd).unwrap_or_else(|| "—".into());
        if let Some(value) = row.cost {
            total += value;
        }
        println!(
            "  {:<30} {:>10} {:>10} {:>12}",
            ui.cream(&row.usage.model),
            human_tokens(row.usage.prompt_tokens.max(0) as usize),
            human_tokens(row.usage.completion_tokens.max(0) as usize),
            ui.green(&cost),
        );
    }
    println!(
        "  {}",
        ui.dim(&format!(
            "estimated Oxen cloud spend: {}",
            format_usd(total)
        ))
    );
    if rows.iter().any(|r| r.cost.is_none()) {
        println!(
            "  {}",
            ui.dim("— = local/custom endpoint or catalog price unavailable")
        );
    }
}
