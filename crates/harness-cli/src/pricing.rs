//! Per-model rates for the context trailer's running price.
//!
//! The `🧭 context …` trailer wants to show what this session has cost so far,
//! but it's rendered synchronously (and on every keystroke in the live
//! composer), while pricing comes from an async endpoint-catalog request. So we
//! keep a small process-wide cache of per-model rates: turn boundaries warm it
//! with [`warm_for`] (an `.await` we're already paying), and the sync trailer
//! reads it through [`crate::spend`], which prices a session tree's usage
//! rows with [`session_rate`].
//!
//! A model with no published rate in the session endpoint's catalog caches as
//! `None` — "no rate" — which the trailer says outright rather than implying
//! the session was free.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use harness_local::source::ModelPricing;

/// Per-model rates learned from the endpoint catalog. A present `None` value
/// means "we asked and the catalog has no rate for this model" — distinct from
/// an absent key ("not fetched yet"), so we don't re-fetch a known-unpriced
/// model every turn.
static CACHE: Mutex<Option<HashMap<String, Option<ModelPricing>>>> = Mutex::new(None);

/// Whether a catalog has been fetched over the network this process. A
/// cache seeded from disk ([`seed_from_disk`]) is warm enough to price with,
/// but still owes the endpoint one fetch for current rates.
static FETCHED: AtomicBool = AtomicBool::new(false);

/// Prime the cache with the rates the last catalog fetch left on disk (see
/// `harness_local::rates`), so startup surfaces can price all-time spend
/// before the network answers. A no-op once anything is cached.
pub(crate) fn seed_from_disk() {
    seed(harness_local::rates::load());
}

fn seed(rates: impl IntoIterator<Item = (String, ModelPricing)>) {
    let mut guard = CACHE.lock().expect("pricing cache poisoned");
    if guard.as_ref().is_some_and(|c| !c.is_empty()) {
        return;
    }
    let cache = guard.get_or_insert_with(HashMap::new);
    for (id, rate) in rates {
        cache.insert(id, Some(rate));
    }
}

/// Fetch pricing for `model` from the catalog at `base_url` — the endpoint
/// the session actually talks to, not the saved default, since a `--host`
/// or `--base-url` session (a local hub, say) is priced by *its* catalog —
/// and cache it, unless it's already cached. Call at turn boundaries (it's
/// async); the sync trailer then reads the result via [`session_rate`].
///
/// The request returns the whole catalog, so every listed model's rate is
/// cached too — that's what lets other synchronous surfaces (the `/model`
/// completion picker's price tags, the lanes of a fleet on another model)
/// read rates without their own request.
pub(crate) async fn warm_for(base_url: &str, model: &str) {
    if FETCHED.load(Ordering::Relaxed) && cached(model).is_some() {
        return;
    }
    let token = harness_runtime::connection::effective_api_key(base_url);
    let pricing = harness_local::source::oxen_model_pricing_catalog_at(
        base_url,
        (!token.trim().is_empty()).then_some(token.as_str()),
    )
    .await
    .ok();
    // A failed catalog request leaves the model uncached, so a later turn
    // retries; a successful one records every listed rate, plus an explicit
    // `None` for the asked-about model when the catalog doesn't list it.
    if let Some(catalog) = pricing {
        let mut guard = CACHE.lock().expect("pricing cache poisoned");
        let cache = guard.get_or_insert_with(HashMap::new);
        cache.insert(model.to_string(), catalog.get(model).copied());
        for (id, rate) in catalog {
            cache.insert(id, Some(rate));
        }
        FETCHED.store(true, Ordering::Relaxed);
    }
}

/// The cached rate for `model`: `Some(Some(rate))` when priced, `Some(None)`
/// when the catalog has no rate for it, `None` when not yet fetched.
fn cached(model: &str) -> Option<Option<ModelPricing>> {
    let guard = CACHE.lock().expect("pricing cache poisoned");
    guard.as_ref().and_then(|c| c.get(model).copied())
}

/// Whether any rates are on hand — a catalog fetch, or the disk seed of the
/// last one — a lock read, never a request.
///
/// Startup surfaces (the banner's all-time spend) price whatever is on record
/// from this cache instead of awaiting the network: `false` means "not priced
/// *yet*" (a first run ever), and the figure fills in on the next usage
/// update rather than holding the first prompt back.
pub(crate) fn is_warm() -> bool {
    let guard = CACHE.lock().expect("pricing cache poisoned");
    guard.as_ref().is_some_and(|c| !c.is_empty())
}

/// `model`'s cached per-token input/output rates, or `None` when it isn't priced
/// (unfetched, or absent from the catalog). The trailer shows this up front so
/// the price is visible before the first token is even spent.
///
/// Once any catalog has landed ([`is_warm`]) the two `None`s mean the same
/// thing — a fetch caches every listed model, so an unfetched model is an
/// unlisted one — which is what lets the trailer call it "no rate".
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
    fn unfetched_model_has_no_rate() {
        assert!(session_rate("never-fetched-xyz").is_none());
    }

    #[test]
    fn a_disk_seed_makes_the_cache_warm_without_a_fetch() {
        super::seed(vec![(
            "seeded-model-d".to_string(),
            ModelPricing {
                input_cost_per_token: 0.000_001,
                output_cost_per_token: 0.000_002,
            },
        )]);
        assert!(is_warm(), "yesterday's rates are enough to price with");
    }

    #[test]
    fn unlisted_model_caches_as_no_rate() {
        // Fetched, but the catalog had no rate: cached `None` → still no rate,
        // distinct from "not fetched" but read the same way by the trailer.
        seed("unlisted-model-b", None);
        assert!(session_rate("unlisted-model-b").is_none());
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
    fn cold_cache_still_renders_the_context_trailer_without_a_rate() {
        // The REPL no longer awaits a catalog fetch before its first prompt, so
        // the trailer has to read a cold cache without losing a line — the rate
        // and the cost simply aren't shown until a fetch lands.
        let ui =
            crate::theme::Ui::with(false, std::sync::Arc::new(harness_theme::Theme::default()));
        let facts = crate::turn::MeterFacts {
            model: "cold-cache-model".to_string(),
            window: 100,
            context_tokens: 10,
            prompt_tokens: 8,
            completion_tokens: 2,
            ledger: None,
            prior: crate::spend::Tokens::default(),
        };
        let lines = facts.lines(&ui);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("cold-cache-model"), "{lines:?}");
        assert!(
            !lines[0].contains("/M"),
            "no rate on a cold cache: {lines:?}"
        );
        assert!(
            !lines[1].contains('$'),
            "no cost on a cold cache: {lines:?}"
        );

        // Once the rate lands (the background warm-up, or the next turn's
        // `warm_for`), the same call shows it.
        seed(
            "cold-cache-model",
            Some(ModelPricing {
                input_cost_per_token: 0.000_003,
                output_cost_per_token: 0.000_015,
            }),
        );
        let warm = facts.lines(&ui);
        assert!(warm[0].contains("$3/M in · $15/M out"), "{warm:?}");
        assert!(is_warm(), "a seeded cache reads as warm");
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
