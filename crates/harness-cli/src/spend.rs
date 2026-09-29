//! What a session has spent so far — its own model calls plus every subagent
//! lane beneath it — priced per model from the rate cache.
//!
//! The context trailer's second line reads from here. The figures come from
//! the usage ledger (`usage_events`) rather than the root agent's in-memory
//! counters, because lanes run as sessions of their own, often on another
//! model: the ledger is the one place their spend and the root's meet, and it
//! is the same source the desktop meter prices, so the two front ends agree
//! on what a chat cost.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use harness_agent::Agent;
use harness_local::source::ModelPricing;
use harness_store::{HistoryError, HistoryStore, ModelUsage};

use crate::pricing;

/// Where a session's tree spend is read from: the history store, rooted at
/// the session whose lanes (at every depth) count toward the bill.
#[derive(Clone)]
pub(crate) struct Ledger {
    store: Arc<HistoryStore>,
    session: String,
    /// Whether a read failure has been written to the developer log. The
    /// meter re-reads the ledger on every usage event and lane spend, so a
    /// ledger that stays broken is reported once, not once per repaint.
    reported: Arc<AtomicBool>,
}

impl Ledger {
    pub(crate) fn new(store: Arc<HistoryStore>, session: impl Into<String>) -> Self {
        Self {
            store,
            session: session.into(),
            reported: Arc::default(),
        }
    }

    /// The ledger `agent`'s usage rows land in, or `None` for an agent whose
    /// usage is attributed to no session (nothing to read back).
    pub(crate) fn for_agent(agent: &Agent) -> Option<Self> {
        let session = agent.usage_session();
        (!session.is_empty()).then(|| Self::new(agent.usage_store().clone(), session))
    }

    /// The root session's own recorded spend, lanes excluded.
    pub(crate) fn root_usage(&self) -> Result<Tokens, HistoryError> {
        let usage = self.store.usage_for_session(&self.session)?;
        Ok(Tokens {
            prompt: usage.prompt_tokens.max(0) as usize,
            completion: usage.completion_tokens.max(0) as usize,
        })
    }

    /// The session tree's spend plus `in_flight` — the root's call in
    /// progress on `model`, which the ledger only sees once it settles —
    /// priced with whatever rates are cached.
    pub(crate) fn spend(&self, model: &str, in_flight: Tokens) -> Result<Spend, HistoryError> {
        let mut rows = self.store.usage_for_tree_by_model(&self.session)?;
        if in_flight.total() > 0 {
            rows.push(in_flight.as_row(model));
        }
        Ok(price_rows(&rows))
    }

    /// Leave a read failure where a developer will find it — once.
    pub(crate) fn report_failure(&self, error: &HistoryError) {
        if self.reported.swap(true, Ordering::Relaxed) {
            return;
        }
        harness_agent::errlog::record(
            harness_config::paths::errors_log().ok().as_deref(),
            "spend_ledger_read_failed",
            serde_json::json!({ "session": self.session, "error": error.to_string() }),
        );
    }
}

/// A prompt/completion token pair.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Tokens {
    pub(crate) prompt: usize,
    pub(crate) completion: usize,
}

impl Tokens {
    pub(crate) fn new(prompt: usize, completion: usize) -> Self {
        Self { prompt, completion }
    }

    pub(crate) fn total(self) -> usize {
        self.prompt + self.completion
    }

    pub(crate) fn plus(self, other: Tokens) -> Tokens {
        Tokens::new(
            self.prompt + other.prompt,
            self.completion + other.completion,
        )
    }

    /// `self - other`, floored at zero per component.
    pub(crate) fn minus(self, other: Tokens) -> Tokens {
        Tokens::new(
            self.prompt.saturating_sub(other.prompt),
            self.completion.saturating_sub(other.completion),
        )
    }

    /// These tokens as one usage row on `model`, so they price like any other.
    fn as_row(self, model: &str) -> ModelUsage {
        ModelUsage {
            model: model.to_string(),
            source: String::new(),
            prompt_tokens: self.prompt as i64,
            completion_tokens: self.completion as i64,
        }
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
    /// to show when there is no ledger to read.
    pub(crate) fn live(model: &str, tokens: Tokens) -> Self {
        price_rows(&[tokens.as_row(model)])
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
    /// Whether every token in the spend is in the dollar figure.
    pub(crate) fn is_complete(&self) -> bool {
        matches!(self, Price::Priced(_))
    }

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
fn price_rows(rows: &[ModelUsage]) -> Spend {
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

        let ledger = Ledger::new(store.clone(), root.clone());
        let spend = ledger
            .spend("ledger-root-model", Tokens::default())
            .unwrap();
        // Tokens: root + both lanes; dollars: 0.27 + 0.11 + 0.11.
        assert_eq!(spend.prompt_tokens, 240_000);
        assert_eq!(spend.completion_tokens, 20_000);
        assert_eq!(spend.price.label().as_deref(), Some("$0.49"));
        // The root's own rows are readable apart from the lanes'.
        assert_eq!(ledger.root_usage().unwrap(), Tokens::new(40_000, 10_000));

        // A call of the root's in flight (sent, not yet settled in the ledger)
        // rides on top, priced at the root's rate: +20k in = +$0.06.
        let climbing = ledger
            .spend("ledger-root-model", Tokens::new(20_000, 0))
            .unwrap();
        assert_eq!(climbing.prompt_tokens, 260_000);
        assert_eq!(climbing.price.label().as_deref(), Some("$0.55"));

        // A lane's own ledger sees only its subtree, so a watched lane can be
        // priced on its own too.
        let lane_spend = Ledger::new(store, lane)
            .spend("ledger-lane-model", Tokens::default())
            .unwrap();
        assert_eq!(lane_spend.prompt_tokens, 200_000);
    }
}
