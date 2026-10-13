//! Cache of API-reported per-model limits.
//!
//! The Oxen models catalog reports each model's real `context_length` and
//! `max_output_tokens`, which input modalities it accepts, and which service
//! tiers (e.g. `ultrafast`) it can run on. Those are the
//! authoritative numbers for context budgeting, reply caps, and whether an
//! image may ride in a request at all — the name-derived table in
//! `harness-agent` is only a fallback for models the catalog hasn't described.
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLimits {
    /// Total context window in tokens.
    #[serde(default)]
    pub context_length: Option<u64>,
    /// Maximum tokens the model can generate in one reply.
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    /// Whether the model accepts image input (`capabilities.input` lists
    /// `"image"`). `None` when the catalog row carried no capabilities —
    /// unknown, not "no": a text-only model that is *known* text-only is what
    /// stops the agent from sending it a picture it would reject.
    #[serde(default)]
    pub accepts_images: Option<bool>,
    /// The non-default service tiers the catalog prices for this model
    /// (`pricing.token_cost_by_service_tier`), e.g. `["ultrafast"]`. Empty
    /// when the model only runs on the provider's default tier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_tiers: Vec<String>,
}

impl ModelLimits {
    /// True when the catalog reported nothing for this model.
    pub fn is_empty(&self) -> bool {
        self.context_length.is_none()
            && self.max_output_tokens.is_none()
            && self.accepts_images.is_none()
            && self.service_tiers.is_empty()
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
    load().models.remove(model)
}

/// The cached context window for `model`, in tokens.
pub fn context_window(model: &str) -> Option<usize> {
    get(model)?.context_length.map(|v| v as usize)
}

/// The cached maximum reply size for `model`, in tokens.
pub fn max_output_tokens(model: &str) -> Option<usize> {
    get(model)?.max_output_tokens.map(|v| v as usize)
}

/// Whether `model` accepts image input: `Some(false)` for a local model
/// (llama-server runs it without a vision projector, so an image part is a
/// rejected request), else what the last catalog fetch said. `None` =
/// unknown (never fetched); callers should keep attaching images then.
pub fn accepts_images(model: &str) -> Option<bool> {
    if is_local_model(model) {
        return Some(false);
    }
    get(model)?.accepts_images
}

/// Whether `id` names a local model: one installed in the store, or a
/// catalog spec. Cloud ids never collide with these.
fn is_local_model(id: &str) -> bool {
    if crate::catalog::find(id).is_some() {
        return true;
    }
    crate::ModelStore::open()
        .ok()
        .is_some_and(|store| store.installed_ref(id).is_some())
}

/// The non-default service tiers `model` can run on, as last reported by
/// the catalog. Empty when unknown or when it has none.
pub fn service_tiers(model: &str) -> Vec<String> {
    get(model).map(|l| l.service_tiers).unwrap_or_default()
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
                    accepts_images: Some(true),
                    service_tiers: Vec::new(),
                },
            ),
            // Reported nothing → skipped, not stored.
            ("flux-2-klein".to_string(), ModelLimits::default()),
        ]);

        assert_eq!(context_window("claude-opus-4-8"), Some(1_000_000));
        assert_eq!(max_output_tokens("claude-opus-4-8"), Some(64_000));
        assert_eq!(accepts_images("claude-opus-4-8"), Some(true));
        assert_eq!(get("flux-2-klein"), None);
        assert_eq!(accepts_images("flux-2-klein"), None);
        // A local model runs without a vision projector: never send it images.
        assert_eq!(accepts_images("gemma-4-26b-a4b-q3"), Some(false));

        // A later fetch updates in place…
        record(vec![(
            "claude-opus-4-8".to_string(),
            ModelLimits {
                context_length: Some(2_000_000),
                max_output_tokens: Some(64_000),
                accepts_images: Some(true),
                service_tiers: Vec::new(),
            },
        )]);
        assert_eq!(context_window("claude-opus-4-8"), Some(2_000_000));

        // A text-only model is recorded as such — that "no" is the signal
        // the agent uses to keep pictures out of its requests.
        record(vec![(
            "deepseek-v4-pro".to_string(),
            ModelLimits {
                context_length: Some(1_048_576),
                max_output_tokens: Some(384_000),
                accepts_images: Some(false),
                service_tiers: Vec::new(),
            },
        )]);
        assert_eq!(accepts_images("deepseek-v4-pro"), Some(false));

        // …and an empty-limits row never erases known-good values.
        record(vec![(
            "claude-opus-4-8".to_string(),
            ModelLimits::default(),
        )]);
        assert_eq!(context_window("claude-opus-4-8"), Some(2_000_000));
    }
}
