//! Cache of API-reported per-model limits.
//!
//! The Oxen models catalog reports each model's real `context_length` and
//! `max_output_tokens`. Those are the authoritative numbers for context
//! budgeting and reply caps — the name-derived table in `harness-agent` is
//! only a fallback for models the catalog hasn't described.
//!
//! The cache is refreshed automatically by every hosted-catalog fetch (model
//! search, pricing warm-ups, usage reports — see `source::fetch_oxen_models`)
//! and read synchronously when an agent is built, so accurate limits are
//! available without a network round-trip at session start. Entries whose
//! catalog row reports no limits are skipped, never erased — a rollout where
//! the API briefly returns nulls must not wipe known-good values.

use std::collections::BTreeMap;

use harness_config::{paths, read_versioned, write_versioned};
use serde::{Deserialize, Serialize};

/// Schema version for `model-limits.json`.
pub const SCHEMA_VERSION: u32 = 1;

/// One model's API-reported limits. Either field may be absent — the catalog
/// describes them independently.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLimits {
    /// Total context window in tokens.
    #[serde(default)]
    pub context_length: Option<u64>,
    /// Maximum tokens the model can generate in one reply.
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
}

impl ModelLimits {
    /// True when the catalog reported nothing for this model.
    pub fn is_empty(&self) -> bool {
        self.context_length.is_none() && self.max_output_tokens.is_none()
    }
}

/// The persisted cache: model id → limits.
#[derive(Debug, Default, Serialize, Deserialize)]
struct LimitsFile {
    #[serde(default)]
    models: BTreeMap<String, ModelLimits>,
}

fn load() -> LimitsFile {
    paths::model_limits_file()
        .map(|p| read_versioned::<LimitsFile>(&p).1)
        .unwrap_or_default()
}

/// The cached limits for `model`, if any catalog fetch has reported them.
pub fn get(model: &str) -> Option<ModelLimits> {
    load().models.get(model).copied()
}

/// The cached context window for `model`, in tokens.
pub fn context_window(model: &str) -> Option<usize> {
    get(model)?.context_length.map(|v| v as usize)
}

/// The cached maximum reply size for `model`, in tokens.
pub fn max_output_tokens(model: &str) -> Option<usize> {
    get(model)?.max_output_tokens.map(|v| v as usize)
}

/// Merge freshly fetched limits into the cache. Best-effort (a cache write
/// must never fail a catalog fetch), skips models reporting no limits, and
/// only touches disk when something actually changed.
pub fn record<I: IntoIterator<Item = (String, ModelLimits)>>(entries: I) {
    let Ok(path) = paths::model_limits_file() else {
        return;
    };
    let mut file = load();
    let mut changed = false;
    for (id, limits) in entries {
        if limits.is_empty() {
            continue;
        }
        if file.models.get(&id) != Some(&limits) {
            file.models.insert(id, limits);
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
        assert_eq!(get("claude-opus-4-8"), None);

        record(vec![
            (
                "claude-opus-4-8".to_string(),
                ModelLimits {
                    context_length: Some(1_000_000),
                    max_output_tokens: Some(64_000),
                },
            ),
            // Reported nothing → skipped, not stored.
            ("flux-2-klein".to_string(), ModelLimits::default()),
        ]);

        assert_eq!(context_window("claude-opus-4-8"), Some(1_000_000));
        assert_eq!(max_output_tokens("claude-opus-4-8"), Some(64_000));
        assert_eq!(get("flux-2-klein"), None);

        // A later fetch updates in place…
        record(vec![(
            "claude-opus-4-8".to_string(),
            ModelLimits {
                context_length: Some(2_000_000),
                max_output_tokens: Some(64_000),
            },
        )]);
        assert_eq!(context_window("claude-opus-4-8"), Some(2_000_000));

        // …and an empty-limits row never erases known-good values.
        record(vec![(
            "claude-opus-4-8".to_string(),
            ModelLimits::default(),
        )]);
        assert_eq!(context_window("claude-opus-4-8"), Some(2_000_000));
    }
}
