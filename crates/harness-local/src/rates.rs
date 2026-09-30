//! Cache of catalog-reported per-model rates.
//!
//! A hosted catalog fetch takes seconds; the startup banner's all-time spend
//! is drawn in milliseconds. So every catalog fetch (see
//! `source::oxen_model_pricing_catalog_at`) records the rates it saw here,
//! and a front end can price what's on record from this file instantly, then
//! let the background fetch refresh it. Rates are merged per model, last
//! fetch wins: a self-hosted endpoint that prices a model the public hub
//! doesn't list keeps that model priced.

use std::collections::BTreeMap;

use harness_config::{paths, read_versioned, write_versioned};
use serde::{Deserialize, Serialize};

use crate::source::ModelPricing;

/// Schema version for `model-rates.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// The persisted cache: model id → per-token rates.
#[derive(Debug, Default, Serialize, Deserialize)]
struct RatesFile {
    #[serde(default)]
    models: BTreeMap<String, ModelPricing>,
}

fn load_file() -> RatesFile {
    paths::model_rates_file()
        .map(|p| read_versioned::<RatesFile>(&p).1)
        .unwrap_or_default()
}

/// Every rate on record, from whichever catalog fetches have landed so far.
/// Empty until the first fetch ever completes.
pub fn load() -> BTreeMap<String, ModelPricing> {
    load_file().models
}

/// Merge freshly fetched rates into the cache. Best-effort (a cache write
/// must never fail a catalog fetch) and only touches disk when something
/// actually changed.
pub fn record<I: IntoIterator<Item = (String, ModelPricing)>>(entries: I) {
    let Ok(path) = paths::model_rates_file() else {
        return;
    };
    let mut file = load_file();
    let mut changed = false;
    for (id, rate) in entries {
        if file.models.get(&id) != Some(&rate) {
            file.models.insert(id, rate);
            changed = true;
        }
    }
    if changed {
        let _ = write_versioned(&path, SCHEMA_VERSION, &file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_then_read_back_and_merge() {
        let _home = crate::temp_harness_dir();
        assert!(load().is_empty());

        let opus = ModelPricing {
            input_cost_per_token: 0.000_005,
            output_cost_per_token: 0.000_025,
        };
        record(vec![("claude-opus-4-8".to_string(), opus)]);
        assert_eq!(load().get("claude-opus-4-8"), Some(&opus));

        // A later fetch (another endpoint, a price change) merges: new models
        // join, a changed rate replaces, untouched ones stay.
        let cheaper = ModelPricing {
            input_cost_per_token: 0.000_004,
            output_cost_per_token: 0.000_020,
        };
        let local = ModelPricing {
            input_cost_per_token: 0.000_002_6,
            output_cost_per_token: 0.000_013,
        };
        record(vec![
            ("claude-opus-4-8".to_string(), cheaper),
            ("gpt-6-1-sol".to_string(), local),
        ]);
        let rates = load();
        assert_eq!(rates.get("claude-opus-4-8"), Some(&cheaper));
        assert_eq!(rates.get("gpt-6-1-sol"), Some(&local));
        assert_eq!(rates.len(), 2);
    }
}
