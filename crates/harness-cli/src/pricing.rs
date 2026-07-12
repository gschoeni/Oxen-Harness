//! Live session cost for the context trailer.
//!
//! The `🧭 context …` trailer wants to show what this session has cost so far,
//! but it's rendered synchronously (and on every keystroke in the live
//! composer), while pricing comes from an async endpoint-catalog request. So we
//! keep a small process-wide cache of per-model rates: turn boundaries warm it
//! with [`warm_for`] (an `.await` we're already paying), and the sync trailer
//! reads it with [`session_cost`].
//!
//! A model with no published rate in the active endpoint's catalog caches as
//! `None` — "cost unavailable" — so the trailer simply omits the price rather
//! than implying the session was free.

use std::collections::HashMap;
use std::sync::Mutex;

use harness_local::source::ModelPricing;

/// Per-model rates learned from the endpoint catalog. A present `None` value
/// means "we asked and the catalog has no rate for this model" — distinct from
/// an absent key ("not fetched yet"), so we don't re-fetch a known-unpriced
/// model every turn.
static CACHE: Mutex<Option<HashMap<String, Option<ModelPricing>>>> = Mutex::new(None);

/// Fetch pricing for `model` from the active endpoint's catalog and cache it,
/// unless it's already cached. Call at turn boundaries (it's async); the sync
/// trailer then reads the result via [`session_cost`].
pub(crate) async fn warm_for(model: &str) {
    if cached(model).is_some() {
        return;
    }
    let connection = harness_runtime::connection::load();
    let base_url = harness_runtime::connection::effective_base_url(&connection);
    let token = harness_runtime::connection::effective_api_key(&base_url);
    let pricing = harness_local::source::oxen_model_pricing_catalog_at(
        &base_url,
        (!token.trim().is_empty()).then_some(token.as_str()),
    )
    .await
    .ok();
    // A failed catalog request leaves the model uncached, so a later turn
    // retries; a successful one records the rate (or `None` when unlisted).
    if let Some(catalog) = pricing {
        let mut guard = CACHE.lock().expect("pricing cache poisoned");
        guard
            .get_or_insert_with(HashMap::new)
            .insert(model.to_string(), catalog.get(model).copied());
    }
}

/// The cached rate for `model`: `Some(Some(rate))` when priced, `Some(None)`
/// when the catalog has no rate for it, `None` when not yet fetched.
fn cached(model: &str) -> Option<Option<ModelPricing>> {
    let guard = CACHE.lock().expect("pricing cache poisoned");
    guard.as_ref().and_then(|c| c.get(model).copied())
}

/// The dollar cost of `prompt_tokens` + `completion_tokens` at `model`'s cached
/// rate, or `None` when the model isn't priced (unfetched, or absent from the
/// catalog) — in which case the trailer omits the cost segment.
pub(crate) fn session_cost(model: &str, prompt_tokens: usize, completion_tokens: usize) -> Option<f64> {
    cached(model)?.map(|p| p.cost_of(prompt_tokens, completion_tokens))
}

/// `model`'s cached per-token input/output rates, or `None` when it isn't priced
/// (unfetched, or absent from the catalog). The trailer shows this up front so
/// the price is visible before the first token is even spent.
pub(crate) fn session_rate(model: &str) -> Option<ModelPricing> {
    cached(model)?
}

/// Seed the pricing cache directly (bypassing the network), for tests in other
/// modules that exercise the trailer's cost/rate output.
#[cfg(test)]
pub(crate) fn seed_for_test(model: &str, pricing: Option<ModelPricing>) {
    let mut guard = CACHE.lock().expect("pricing cache poisoned");
    guard
        .get_or_insert_with(HashMap::new)
        .insert(model.to_string(), pricing);
}

/// A compact per-million-token price label for a model's rates, e.g.
/// `$3/M in · $15/M out`. Per-token rates are tiny fractions of a cent; scaling
/// to a million tokens gives a number a human can actually compare. A rate that
/// rounds to `$0/M` is dropped, and if both are zero (a free/local model) the
/// whole label is `None` so the trailer omits it.
pub(crate) fn format_rate(rate: &ModelPricing) -> Option<String> {
    let per_million = |per_token: f64| -> Option<String> {
        let m = per_token * 1_000_000.0;
        (m > 0.0).then(|| {
            // Whole-dollar rates read cleaner without trailing zeros ($3/M),
            // fractional ones keep two decimals ($0.50/M).
            if (m - m.round()).abs() < f64::EPSILON {
                format!("${}/M", m.round() as i64)
            } else {
                format!("${m:.2}/M")
            }
        })
    };
    match (
        per_million(rate.input_cost_per_token),
        per_million(rate.output_cost_per_token),
    ) {
        (Some(input), Some(output)) => Some(format!("{input} in · {output} out")),
        (Some(input), None) => Some(format!("{input} in")),
        (None, Some(output)) => Some(format!("{output} out")),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed the cache directly (bypassing the network) so we can exercise the
    /// sync read path.
    fn seed(model: &str, pricing: Option<ModelPricing>) {
        let mut guard = CACHE.lock().expect("pricing cache poisoned");
        guard
            .get_or_insert_with(HashMap::new)
            .insert(model.to_string(), pricing);
    }

    #[test]
    fn unfetched_model_has_no_cost() {
        assert_eq!(session_cost("never-fetched-xyz", 1_000, 500), None);
    }

    #[test]
    fn priced_model_multiplies_tokens_by_rate() {
        seed(
            "priced-model-a",
            Some(ModelPricing {
                input_cost_per_token: 0.000_001,
                output_cost_per_token: 0.000_002,
            }),
        );
        // 1000 * 1e-6 + 500 * 2e-6 = 0.001 + 0.001 = 0.002
        assert_eq!(session_cost("priced-model-a", 1_000, 500), Some(0.002));
    }

    #[test]
    fn unlisted_model_caches_as_no_cost() {
        // Fetched, but the catalog had no rate: cached `None` → still no cost,
        // distinct from "not fetched" but treated the same by the trailer.
        seed("unlisted-model-b", None);
        assert_eq!(session_cost("unlisted-model-b", 1_000, 500), None);
    }

    #[test]
    fn rate_label_scales_to_per_million_tokens() {
        // 3e-6 / 15e-6 per token → $3/M in · $15/M out (whole dollars, no cents).
        let label = format_rate(&ModelPricing {
            input_cost_per_token: 0.000_003,
            output_cost_per_token: 0.000_015,
        });
        assert_eq!(label.as_deref(), Some("$3/M in · $15/M out"));
    }

    #[test]
    fn rate_label_keeps_cents_for_fractional_rates() {
        let label = format_rate(&ModelPricing {
            input_cost_per_token: 0.000_000_5,
            output_cost_per_token: 0.000_001_5,
        });
        assert_eq!(label.as_deref(), Some("$0.50/M in · $1.50/M out"));
    }

    #[test]
    fn free_model_has_no_rate_label() {
        let label = format_rate(&ModelPricing {
            input_cost_per_token: 0.0,
            output_cost_per_token: 0.0,
        });
        assert_eq!(label, None);
    }

    #[test]
    fn session_rate_returns_cached_pricing() {
        let rate = ModelPricing {
            input_cost_per_token: 0.000_001,
            output_cost_per_token: 0.000_002,
        };
        seed("rate-model-c", Some(rate));
        let got = session_rate("rate-model-c").expect("priced");
        assert_eq!(got.input_cost_per_token, 0.000_001);
        assert_eq!(got.output_cost_per_token, 0.000_002);
        // Unfetched stays None.
        assert!(session_rate("never-fetched-rate").is_none());
    }
}
