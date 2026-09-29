//! What a session has spent so far — its own model calls plus every subagent
//! lane beneath it — priced per model from the rate cache.
//!
//! The context trailer's second line reads from here. The figures come from
//! the usage ledger (`usage_events`) rather than the root agent's in-memory
//! counters, because lanes run as sessions of their own, often on another
//! model: the ledger is the one place their spend and the root's meet, and it
//! is the same source the desktop meter prices, so the two front ends agree
//! on what a chat cost.

use std::sync::Arc;

use harness_local::source::ModelPricing;
use harness_store::{HistoryError, HistoryStore, ModelUsage};

use crate::pricing;

/// Where a session's tree spend is read from: the history store, rooted at
/// the session whose lanes (at every depth) count toward the bill.
#[derive(Clone)]
pub(crate) struct Ledger {
    store: Arc<HistoryStore>,
    session: String,
}

impl Ledger {
    pub(crate) fn new(store: Arc<HistoryStore>, session: impl Into<String>) -> Self {
        Self {
            store,
            session: session.into(),
        }
    }

    /// The ledger for `agent`'s session in `store`.
    pub(crate) fn for_agent(store: &Arc<HistoryStore>, agent: &harness_agent::Agent) -> Self {
        Self::new(store.clone(), agent.session_id())
    }

    /// The session tree's spend, priced with whatever rates are cached.
    pub(crate) fn spend(&self) -> Result<Spend, HistoryError> {
        let rows = self.store.usage_for_tree_by_model(&self.session)?;
        Ok(price_rows(&rows))
    }
}

/// A session tree's spend: the token totals and what they cost.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Spend {
    pub(crate) prompt_tokens: usize,
    pub(crate) completion_tokens: usize,
    pub(crate) price: Price,
}

impl Spend {
    pub(crate) fn total_tokens(&self) -> usize {
        self.prompt_tokens + self.completion_tokens
    }

    /// The root agent's own counters priced at its model's rate — the figure
    /// to show when there is no ledger to read (tests, a session whose first
    /// call hasn't landed in the ledger yet).
    pub(crate) fn live(model: &str, prompt_tokens: usize, completion_tokens: usize) -> Self {
        let rows = [ModelUsage {
            model: model.to_string(),
            source: String::new(),
            prompt_tokens: prompt_tokens as i64,
            completion_tokens: completion_tokens as i64,
        }];
        price_rows(&rows)
    }
}

/// The dollar figure for a spend, or why there isn't one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Price {
    /// No catalog has been fetched yet (the warm-up is in flight, or the
    /// endpoint couldn't be reached): nothing to show rather than "$0.00".
    Pending,
    /// Every model in the tree has a published rate.
    Priced(f64),
    /// Some of the tree is priced; `unpriced` names the models whose tokens
    /// are *not* in `usd`, so the figure reads as a floor, not a total.
    Partial { usd: f64, unpriced: Vec<String> },
    /// Tokens were spent, but no model in the tree has a published rate.
    Unpriced(Vec<String>),
}

impl Price {
    /// The trailer's price segment: `$1.23`, `$1.23 + no rate for m`, or
    /// `no rate for m`. `None` when there is nothing honest to show — no
    /// catalog yet, or a spend that rounds to nothing.
    pub(crate) fn label(&self) -> Option<String> {
        // A figure that formats as all zeros ("$0.0000") would read as free.
        let usd = |usd: f64| {
            Some(crate::theme::format_usd(usd))
                .filter(|s| s.chars().any(|c| ('1'..='9').contains(&c)))
        };
        match self {
            Price::Pending => None,
            Price::Priced(cost) => usd(*cost),
            Price::Partial {
                usd: cost,
                unpriced,
            } => Some(match usd(*cost) {
                Some(cost) => format!("{cost} + no rate for {}", unpriced.join(", ")),
                None => format!("no rate for {}", unpriced.join(", ")),
            }),
            Price::Unpriced(models) => Some(format!("no rate for {}", models.join(", "))),
        }
    }
}

/// Price usage rows with the process-wide rate cache (see [`crate::pricing`]).
pub(crate) fn price_rows(rows: &[ModelUsage]) -> Spend {
    price_rows_with(rows, pricing::is_warm(), pricing::session_rate)
}

/// [`price_rows`] with the cache state passed in, so the pricing rules can
/// be tested without a shared, order-dependent cache: `warm` is whether any
/// catalog has landed, `rate` the per-model lookup.
fn price_rows_with(
    rows: &[ModelUsage],
    warm: bool,
    rate: impl Fn(&str) -> Option<ModelPricing>,
) -> Spend {
    let mut prompt_tokens = 0usize;
    let mut completion_tokens = 0usize;
    let mut usd = 0.0;
    let mut priced_any = false;
    let mut unpriced: Vec<String> = Vec::new();
    for row in rows {
        let (prompt, completion) = (
            row.prompt_tokens.max(0) as usize,
            row.completion_tokens.max(0) as usize,
        );
        prompt_tokens += prompt;
        completion_tokens += completion;
        if prompt + completion == 0 {
            continue;
        }
        match rate(&row.model) {
            Some(rate) => {
                priced_any = true;
                usd += rate.cost_of(prompt, completion);
            }
            None if unpriced.contains(&row.model) => {}
            None => unpriced.push(row.model.clone()),
        }
    }
    let price = if !warm {
        Price::Pending
    } else if unpriced.is_empty() {
        Price::Priced(usd)
    } else if priced_any {
        Price::Partial { usd, unpriced }
    } else {
        Price::Unpriced(unpriced)
    };
    Spend {
        prompt_tokens,
        completion_tokens,
        price,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_store::{SessionMeta, UsageDetail};

    fn row(model: &str, prompt: i64, completion: i64) -> ModelUsage {
        ModelUsage {
            model: model.to_string(),
            source: "oxen_cloud".to_string(),
            prompt_tokens: prompt,
            completion_tokens: completion,
        }
    }

    /// $3/M in, $15/M out for `big`; $1/M in, $2/M out for `small`; nothing
    /// for anything else.
    fn rates(model: &str) -> Option<ModelPricing> {
        match model {
            "big" => Some(ModelPricing {
                input_cost_per_token: 0.000_003,
                output_cost_per_token: 0.000_015,
            }),
            "small" => Some(ModelPricing {
                input_cost_per_token: 0.000_001,
                output_cost_per_token: 0.000_002,
            }),
            _ => None,
        }
    }

    #[test]
    fn a_fully_priced_tree_sums_every_model_at_its_own_rate() {
        // The root on the big model, a lane on the small one: tokens add up
        // across the tree and each model is priced at its own rate.
        let spend = price_rows_with(
            &[row("big", 40_000, 10_000), row("small", 100_000, 5_000)],
            true,
            rates,
        );
        assert_eq!(spend.prompt_tokens, 140_000);
        assert_eq!(spend.completion_tokens, 15_000);
        assert_eq!(spend.total_tokens(), 155_000);
        // big: 0.12 + 0.15 = 0.27; small: 0.10 + 0.01 = 0.11
        let Price::Priced(usd) = spend.price else {
            panic!("priced: {:?}", spend.price);
        };
        assert!((usd - 0.38).abs() < 1e-9, "{usd}");
        assert_eq!(spend.price.label().as_deref(), Some("$0.38"));
    }

    #[test]
    fn an_unlisted_model_is_named_instead_of_silently_priced_at_zero() {
        // Every token on a model the catalog doesn't list: the trailer says
        // so, and never prints a dollar figure that would read as free.
        let spend = price_rows_with(&[row("gpt-6-1-sol", 773_600, 2_900)], true, rates);
        assert_eq!(
            spend.price,
            Price::Unpriced(vec!["gpt-6-1-sol".to_string()])
        );
        assert_eq!(
            spend.price.label().as_deref(),
            Some("no rate for gpt-6-1-sol")
        );
    }

    #[test]
    fn a_partly_priced_tree_shows_the_floor_and_names_the_rest() {
        // A priced lane under an unpriced root (or the reverse): the dollars
        // that are known, plus which model is missing from them. Two rows on
        // the same unlisted model name it once.
        let spend = price_rows_with(
            &[
                row("mystery", 1_000, 100),
                row("small", 100_000, 5_000),
                row("mystery", 500, 50),
            ],
            true,
            rates,
        );
        let Price::Partial { usd, unpriced } = &spend.price else {
            panic!("partial: {:?}", spend.price);
        };
        assert!((usd - 0.11).abs() < 1e-9, "{usd}");
        assert_eq!(unpriced, &["mystery".to_string()]);
        assert_eq!(
            spend.price.label().as_deref(),
            Some("$0.11 + no rate for mystery")
        );
    }

    #[test]
    fn a_cold_cache_is_pending_not_unpriced() {
        // Before any catalog lands nothing can be said about rates, so the
        // trailer shows no price at all rather than blaming the model.
        let spend = price_rows_with(&[row("big", 40_000, 10_000)], false, rates);
        assert_eq!(spend.price, Price::Pending);
        assert_eq!(spend.price.label(), None);
    }

    #[test]
    fn a_spend_that_rounds_to_nothing_shows_no_price() {
        // A handful of cheap tokens: "$0.00" would read as free, so the
        // segment is omitted until it rounds to something visible.
        let spend = price_rows_with(&[row("small", 10, 2)], true, rates);
        assert_eq!(spend.price.label(), None);
        // …and no usage at all is simply nothing to price.
        assert_eq!(price_rows_with(&[], true, rates).price, Price::Priced(0.0));
    }

    #[test]
    fn the_ledger_prices_the_root_and_every_lane_beneath_it() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let root = store.create_session(&SessionMeta::default()).unwrap();
        let lane = store
            .create_session(&SessionMeta {
                parent_session: root.clone(),
                ..Default::default()
            })
            .unwrap();
        let leaf = store
            .create_session(&SessionMeta {
                parent_session: lane.clone(),
                ..Default::default()
            })
            .unwrap();
        let record = |session: &str, model: &str, prompt: usize, completion: usize| {
            store
                .record_model_usage_detailed(
                    model,
                    "oxen_cloud",
                    prompt,
                    completion,
                    &UsageDetail {
                        session_id: session,
                        kind: "turn",
                        ..Default::default()
                    },
                )
                .unwrap();
        };
        // The root on a priced model, its lanes on a cheaper priced one.
        record(&root, "ledger-root-model", 40_000, 10_000);
        record(&lane, "ledger-lane-model", 100_000, 5_000);
        record(&leaf, "ledger-lane-model", 100_000, 5_000);
        crate::pricing::seed_for_test("ledger-root-model", rates("big"));
        crate::pricing::seed_for_test("ledger-lane-model", rates("small"));

        let spend = Ledger::new(store.clone(), root.clone()).spend().unwrap();
        // Tokens: root + both lanes; dollars: 0.27 + 0.11 + 0.11.
        assert_eq!(spend.prompt_tokens, 240_000);
        assert_eq!(spend.completion_tokens, 20_000);
        assert_eq!(spend.price.label().as_deref(), Some("$0.49"));

        // A lane's own ledger sees only its subtree, so a watched lane can be
        // priced on its own too.
        let lane_spend = Ledger::new(store, lane).spend().unwrap();
        assert_eq!(lane_spend.prompt_tokens, 200_000);
    }
}
