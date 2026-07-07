//! Statistical JSON compression ("SmartCrusher-lite", after headroom).
//!
//! Big JSON tool results — search hits, API responses, log queries — are
//! usually arrays where most rows say the same thing. This module keeps the
//! rows that carry signal (boundaries, errors, numeric anomalies, one exemplar
//! per duplicate group) and offloads the rest to the [`CcrStore`], leaving a
//! `{"_ccr_dropped": "<<ccr:HASH N_rows_offloaded>>"}` sentinel in the array
//! so the model knows rows were removed and how to get them back.
//!
//! The safety rails matter more than the sampling: arrays of *distinct*
//! entities with no keep-signal are passed through untouched (sampling a
//! contact list loses data; sampling 500 near-identical log rows doesn't),
//! and rows mentioning errors are always kept.

use serde_json::Value;

use crate::ccr::{marker, CcrStore};
use crate::{contains_error_keyword, CompressConfig};

/// How deep to recurse into nested objects/arrays looking for crushable
/// arrays. Tool output nesting is shallow in practice; the cap just bounds
/// pathological inputs.
const MAX_DEPTH: usize = 10;

/// Scalar arrays shorter than this are never sampled.
const MIN_SCALAR_ITEMS: usize = 30;

/// The key of the sentinel object appended to a crushed array.
pub const DROPPED_KEY: &str = "_ccr_dropped";

/// Crush every eligible array inside `value` in place. Returns a short
/// strategy note when anything changed, `None` when the value passed through
/// untouched. `store` is `None` in audit mode: markers are still rendered (the
/// result is discarded), but nothing is stashed.
pub fn crush_json(
    value: &mut Value,
    cfg: &CompressConfig,
    store: Option<&CcrStore>,
) -> Option<String> {
    let mut notes = Vec::new();
    process_value(value, cfg, store, 0, &mut notes);
    if notes.is_empty() {
        None
    } else {
        Some(notes.join(", "))
    }
}

fn process_value(
    value: &mut Value,
    cfg: &CompressConfig,
    store: Option<&CcrStore>,
    depth: usize,
    notes: &mut Vec<String>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    match value {
        Value::Array(items) => {
            // Recurse first so nested arrays inside kept rows are also lean.
            for item in items.iter_mut() {
                process_value(item, cfg, store, depth + 1, notes);
            }
            if items.iter().all(Value::is_object) && items.len() > cfg.max_items_after_crush {
                if let Some(note) = crush_dict_array(items, cfg, store) {
                    notes.push(note);
                }
            } else if items.iter().all(|v| !v.is_object() && !v.is_array())
                && items.len() >= MIN_SCALAR_ITEMS
            {
                if let Some(note) = crush_scalar_array(items, cfg, store) {
                    notes.push(note);
                }
            }
        }
        Value::Object(map) => {
            for v in map.values_mut() {
                process_value(v, cfg, store, depth + 1, notes);
            }
        }
        _ => {}
    }
}

/// Sample an array of objects down to the rows that carry signal.
fn crush_dict_array(
    items: &mut Vec<Value>,
    cfg: &CompressConfig,
    store: Option<&CcrStore>,
) -> Option<String> {
    let n = items.len();
    if n < cfg.min_items_to_analyze || n <= cfg.max_items_after_crush {
        return None;
    }

    let canonical: Vec<String> = items.iter().map(Value::to_string).collect();
    let stats = FieldStats::analyze(items);

    // Keep-signals: rows that must survive regardless of sampling.
    let error_rows: Vec<usize> = canonical
        .iter()
        .enumerate()
        .filter(|(_, row)| contains_error_keyword(row))
        .map(|(i, _)| i)
        .collect();
    let anomaly_rows = stats.numeric_anomaly_rows(items);
    let has_signal = !error_rows.is_empty() || !anomaly_rows.is_empty();

    // The "unique entities" guard (headroom's most important rail): an array
    // where the content fields are mostly distinct and nothing flags a row as
    // special is a list of records, not a stream of noise — sampling it would
    // silently lose data. Repetitive content (low uniqueness) or flagged rows
    // are safe to sample.
    if stats.content_uniqueness > 0.3 && !has_signal {
        return None;
    }

    // Select the kept rows: boundary anchors, every signal row, then fill the
    // remaining budget with a spread of content-unique rows.
    let mut keep = vec![false; n];
    for k in keep.iter_mut().take(cfg.first_keep) {
        *k = true;
    }
    for k in keep.iter_mut().skip(n.saturating_sub(cfg.last_keep)) {
        *k = true;
    }
    for &i in error_rows.iter().chain(anomaly_rows.iter()) {
        keep[i] = true;
    }

    let mut kept_forms: std::collections::HashSet<&str> = keep
        .iter()
        .enumerate()
        .filter(|(_, k)| **k)
        .map(|(i, _)| canonical[i].as_str())
        .collect();
    let budget = cfg.max_items_after_crush;
    let mut kept_count = keep.iter().filter(|k| **k).count();
    if kept_count < budget {
        // Stride over the middle, skipping rows identical to one already kept.
        let remaining = budget - kept_count;
        let stride = (n / (remaining + 1)).max(1);
        let mut i = stride;
        while i < n && kept_count < budget {
            if !keep[i] && kept_forms.insert(canonical[i].as_str()) {
                keep[i] = true;
                kept_count += 1;
            }
            i += stride;
        }
    }

    let dropped = n - kept_count;
    if dropped == 0 {
        return None;
    }

    // Offload the full original array and splice in the kept rows + sentinel.
    let original = Value::Array(items.clone()).to_string();
    let hash = match store {
        Some(store) => store.put(&original),
        None => crate::ccr::hash_content(&original),
    };
    let mut out = Vec::with_capacity(kept_count + 1);
    for (i, item) in items.drain(..).enumerate() {
        if keep[i] {
            out.push(item);
        }
    }
    out.push(serde_json::json!({
        DROPPED_KEY: marker(&hash, Some(&format!("{dropped}_rows_offloaded"))),
    }));
    *items = out;
    Some(format!("array {n}\u{2192}{kept_count} rows"))
}

/// Sample a long array of scalars (strings/numbers) down to its boundaries
/// plus any error-bearing strings, with a stats note for numbers.
fn crush_scalar_array(
    items: &mut Vec<Value>,
    cfg: &CompressConfig,
    store: Option<&CcrStore>,
) -> Option<String> {
    let n = items.len();
    let head = cfg.first_keep.max(5);
    let tail = cfg.last_keep.max(3);
    if n <= head + tail + 2 {
        return None;
    }

    let mut keep = vec![false; n];
    for (i, k) in keep.iter_mut().enumerate() {
        *k = i < head || i >= n - tail;
    }
    for (i, item) in items.iter().enumerate() {
        if let Value::String(s) = item {
            if contains_error_keyword(s) {
                keep[i] = true;
            }
        }
    }

    let kept_count = keep.iter().filter(|k| **k).count();
    let dropped = n - kept_count;
    if dropped == 0 {
        return None;
    }

    let numbers: Vec<f64> = items.iter().filter_map(Value::as_f64).collect();
    let stats_note = if numbers.len() == n {
        let (min, max, mean) = min_max_mean(&numbers);
        format!(" (min {min:.4}, max {max:.4}, mean {mean:.4})")
    } else {
        String::new()
    };

    let original = Value::Array(items.clone()).to_string();
    let hash = match store {
        Some(store) => store.put(&original),
        None => crate::ccr::hash_content(&original),
    };
    let mut out = Vec::with_capacity(kept_count + 1);
    for (i, item) in items.drain(..).enumerate() {
        if keep[i] {
            out.push(item);
        }
    }
    out.push(Value::String(format!(
        "{}{stats_note}",
        marker(&hash, Some(&format!("{dropped}_items_offloaded")))
    )));
    *items = out;
    Some(format!("array {n}\u{2192}{kept_count} items"))
}

fn min_max_mean(values: &[f64]) -> (f64, f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for &v in values {
        min = min.min(v);
        max = max.max(v);
        sum += v;
    }
    (min, max, sum / values.len() as f64)
}

/// Field names that identify a row rather than describe it.
fn is_id_like_key(key: &str) -> bool {
    const ID_HINTS: &[&str] = &[
        "id",
        "uuid",
        "guid",
        "key",
        "hash",
        "sha",
        "token",
        "timestamp",
        "time",
        "date",
        "created",
        "updated",
    ];
    let k = key.to_ascii_lowercase();
    ID_HINTS
        .iter()
        .any(|h| k == *h || k.ends_with(&format!("_{h}")) || k.starts_with(&format!("{h}_")))
}

/// The canonical 8-4-4-4-12 hex UUID shape.
fn looks_like_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(len, p)| p.len() == *len && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Per-field statistics over an array of objects, driving the crushability
/// decision and anomaly detection.
struct FieldStats {
    /// Max unique-value ratio among string-typed fields that don't look like
    /// IDs. High → rows are distinct records; low → repetitive content.
    content_uniqueness: f64,
    /// (field name, mean, stddev) for numeric fields with spread.
    numeric: Vec<(String, f64, f64)>,
}

impl FieldStats {
    fn analyze(items: &[Value]) -> Self {
        use std::collections::{BTreeMap, HashSet};
        let n = items.len();

        // Sorted key iteration keeps the whole pass deterministic.
        let mut keys: BTreeMap<&str, ()> = BTreeMap::new();
        for item in items {
            if let Value::Object(map) = item {
                for k in map.keys() {
                    keys.insert(k, ());
                }
            }
        }

        let mut content_uniqueness: f64 = 0.0;
        let mut numeric = Vec::new();
        for (&key, ()) in &keys {
            let values: Vec<&Value> = items
                .iter()
                .filter_map(|i| i.as_object().and_then(|m| m.get(key)))
                .collect();
            if values.is_empty() {
                continue;
            }

            let strings: Vec<&str> = values.iter().filter_map(|v| v.as_str()).collect();
            if strings.len() == values.len() {
                // An identifier field (request ids, hashes, timestamps) is
                // unique on every row of anything, so it says nothing about
                // whether the *content* is distinct — leave it out of the
                // uniqueness signal.
                if is_id_like_key(key) || strings.iter().all(|s| looks_like_uuid(s)) {
                    continue;
                }
                let unique: HashSet<&str> = strings.iter().copied().collect();
                content_uniqueness = content_uniqueness.max(unique.len() as f64 / n as f64);
                continue;
            }

            let nums: Vec<f64> = values.iter().filter_map(|v| v.as_f64()).collect();
            if nums.len() == values.len() && nums.len() >= 10 {
                let (_, _, mean) = min_max_mean(&nums);
                let variance =
                    nums.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (nums.len() - 1) as f64;
                let stddev = variance.sqrt();
                if stddev > 0.0 && stddev.is_finite() {
                    numeric.push((key.to_string(), mean, stddev));
                }
            }
        }
        Self {
            content_uniqueness,
            numeric,
        }
    }

    /// Rows where any numeric field sits more than 2σ from its mean — spikes
    /// and outliers the model would want to see even in a sampled view.
    fn numeric_anomaly_rows(&self, items: &[Value]) -> Vec<usize> {
        let mut rows = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let Some(map) = item.as_object() else {
                continue;
            };
            for (key, mean, stddev) in &self.numeric {
                if let Some(v) = map.get(key).and_then(Value::as_f64) {
                    if (v - mean).abs() > 2.0 * stddev {
                        rows.push(i);
                        break;
                    }
                }
            }
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CompressConfig {
        CompressConfig::default()
    }

    /// A repetitive log-query-style array: same message, incrementing ids.
    fn repetitive_rows(n: usize) -> Value {
        let rows: Vec<Value> = (0..n)
            .map(|i| serde_json::json!({"id": i, "level": "info", "message": "heartbeat ok"}))
            .collect();
        Value::Array(rows)
    }

    #[test]
    fn repetitive_array_is_crushed_with_sentinel() {
        let mut value = repetitive_rows(100);
        let store = CcrStore::default();
        let note = crush_json(&mut value, &cfg(), Some(&store)).expect("should crush");
        assert!(note.contains("100"));

        let items = value.as_array().unwrap();
        assert!(items.len() <= cfg().max_items_after_crush + 1);
        let sentinel = items.last().unwrap();
        let text = sentinel[DROPPED_KEY].as_str().unwrap();
        assert!(text.starts_with("<<ccr:"), "sentinel carries a marker");
        assert!(text.contains("rows_offloaded"));

        // The original is retrievable via the marker hash.
        let hash = text
            .trim_start_matches("<<ccr:")
            .split_whitespace()
            .next()
            .unwrap();
        let original = store.get(hash).expect("original stored");
        assert!(original.contains("heartbeat ok"));
        assert_eq!(
            serde_json::from_str::<Value>(&original).unwrap(),
            repetitive_rows(100)
        );
    }

    #[test]
    fn distinct_entities_without_signal_pass_through() {
        // 50 rows of genuinely different records (unique names) — sampling
        // this would lose real data, so the guard must refuse.
        let rows: Vec<Value> = (0..50)
            .map(|i| serde_json::json!({"id": i, "name": format!("customer-{i}"), "city": format!("town-{i}")}))
            .collect();
        let mut value = Value::Array(rows);
        let before = value.clone();
        assert!(crush_json(&mut value, &cfg(), None).is_none());
        assert_eq!(value, before);
    }

    #[test]
    fn error_and_anomaly_rows_survive_crushing() {
        let mut rows: Vec<Value> = (0..80)
            .map(|i| serde_json::json!({"seq": i, "status": "ok", "latency_ms": 20.0}))
            .collect();
        rows[41] =
            serde_json::json!({"seq": 41, "status": "connection timeout", "latency_ms": 20.0});
        rows[57] = serde_json::json!({"seq": 57, "status": "ok", "latency_ms": 950.0});
        let mut value = Value::Array(rows);
        crush_json(&mut value, &cfg(), None).expect("should crush");

        let text = value.to_string();
        assert!(text.contains("connection timeout"), "error row kept");
        assert!(text.contains("950"), "latency spike kept");
    }

    #[test]
    fn small_arrays_are_untouched() {
        let mut value = repetitive_rows(10);
        let before = value.clone();
        assert!(crush_json(&mut value, &cfg(), None).is_none());
        assert_eq!(value, before);
    }

    #[test]
    fn nested_arrays_inside_objects_are_crushed() {
        let mut value = serde_json::json!({"query": "logs", "results": repetitive_rows(60)});
        crush_json(&mut value, &cfg(), None).expect("nested array should crush");
        assert!(value["results"].as_array().unwrap().len() < 60);
        assert_eq!(value["query"], "logs"); // siblings untouched
    }

    #[test]
    fn long_scalar_arrays_keep_boundaries_and_stats() {
        let nums: Vec<Value> = (0..200).map(|i| serde_json::json!(i as f64)).collect();
        let mut value = Value::Array(nums);
        crush_json(&mut value, &cfg(), None).expect("should crush");
        let items = value.as_array().unwrap();
        assert!(items.len() < 200);
        assert_eq!(items[0], serde_json::json!(0.0)); // head kept
        let note = items.last().unwrap().as_str().unwrap();
        assert!(note.contains("items_offloaded"));
        assert!(note.contains("mean"));
    }

    #[test]
    fn crushing_is_deterministic() {
        let mut a = repetitive_rows(100);
        let mut b = repetitive_rows(100);
        crush_json(&mut a, &cfg(), None);
        crush_json(&mut b, &cfg(), None);
        assert_eq!(a, b);
    }
}
