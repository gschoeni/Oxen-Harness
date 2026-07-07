//! Compress-cache-retrieve: the store that makes compression reversible.
//!
//! When the compressor drops content, the original is stashed here keyed by a
//! short content hash, and the compressed text carries a `<<ccr:HASH>>` marker
//! in its place. The `retrieve_original` tool looks the hash up, so the model
//! can always recover anything compression removed — "lossy" on the wire, not
//! in fact. (The scheme and marker format follow headroom's CCR design.)
//!
//! The store is in-memory and capacity-bounded (oldest entry evicted first);
//! originals also survive verbatim in the history store, so eviction only
//! costs the model the convenient path, never the data.

use std::collections::HashMap;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// Every CCR marker starts with this; content containing it is never
/// re-compressed (a marker inside a marker would be unresolvable).
pub const MARKER_PREFIX: &str = "<<ccr:";

/// Hash `content` to the short hex key used in markers: the first 12 hex chars
/// (6 bytes) of its SHA-256, matching headroom's row-drop scheme.
pub fn hash_content(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// Render the inline marker for a stored original, with an optional short note
/// (e.g. `42_rows_offloaded`) telling the model what was removed.
pub fn marker(hash: &str, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{MARKER_PREFIX}{hash} {note}>>"),
        None => format!("{MARKER_PREFIX}{hash}>>"),
    }
}

/// In-memory store of compressed-away originals, shared (via `Arc`) between
/// the compressor (writes) and the `retrieve_original` tool (reads).
#[derive(Debug)]
pub struct CcrStore {
    inner: Mutex<Inner>,
    capacity: usize,
}

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<String, String>,
    /// Insertion order for FIFO eviction.
    order: Vec<String>,
}

impl Default for CcrStore {
    fn default() -> Self {
        Self::with_capacity(512)
    }
}

impl CcrStore {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            capacity: capacity.max(1),
        }
    }

    /// Store an original, returning its marker hash. Idempotent: the same
    /// content always maps to the same hash (and refreshes its slot).
    pub fn put(&self, content: &str) -> String {
        let hash = hash_content(content);
        let mut inner = self.inner.lock().expect("ccr store lock");
        if !inner.entries.contains_key(&hash) {
            while inner.order.len() >= self.capacity {
                let oldest = inner.order.remove(0);
                inner.entries.remove(&oldest);
            }
            inner.order.push(hash.clone());
            inner.entries.insert(hash.clone(), content.to_string());
        }
        hash
    }

    /// Look up a stored original by its marker hash.
    pub fn get(&self, hash: &str) -> Option<String> {
        self.inner
            .lock()
            .expect("ccr store lock")
            .entries
            .get(hash)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("ccr store lock").entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_then_get_round_trips_and_is_idempotent() {
        let store = CcrStore::default();
        let h1 = store.put("the original payload");
        let h2 = store.put("the original payload");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 12);
        assert_eq!(store.get(&h1).as_deref(), Some("the original payload"));
        assert_eq!(store.len(), 1);
        assert!(store.get("ffffffffffff").is_none());
    }

    #[test]
    fn capacity_evicts_oldest_first() {
        let store = CcrStore::with_capacity(2);
        let h1 = store.put("one");
        let h2 = store.put("two");
        let h3 = store.put("three");
        assert!(store.get(&h1).is_none(), "oldest entry should be evicted");
        assert!(store.get(&h2).is_some());
        assert!(store.get(&h3).is_some());
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn marker_formats_with_and_without_note() {
        assert_eq!(marker("abc123", None), "<<ccr:abc123>>");
        assert_eq!(
            marker("abc123", Some("42_rows_offloaded")),
            "<<ccr:abc123 42_rows_offloaded>>"
        );
    }
}
