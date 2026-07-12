//! Compress-cache-retrieve: the store that makes compression reversible.
//!
//! When the compressor drops content, the original is stashed here keyed by a
//! short content hash, and the compressed text carries a `<<ccr:HASH>>` marker
//! in its place. The `retrieve_original` tool looks the hash up, so the model
//! can always recover anything compression removed — "lossy" on the wire, not
//! in fact. (The scheme and marker format follow headroom's CCR design.)
//!
//! The index is entry- and byte-bounded (oldest entry evicted first). Production
//! agents put payloads on disk and retain only metadata in memory; originals
//! also survive verbatim in history, so eviction never loses conversation data.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
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

/// Bounded store of compressed-away originals, shared (via `Arc`) between the
/// compressor (writes) and the `retrieve_original` tool (reads).
#[derive(Debug)]
pub struct CcrStore {
    inner: Mutex<Inner>,
    capacity: usize,
    max_bytes: usize,
    directory: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<String, Entry>,
    /// Insertion order for FIFO eviction.
    order: VecDeque<String>,
    bytes: usize,
}

#[derive(Debug)]
struct Entry {
    content: Option<String>,
    bytes: usize,
}

pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

impl Default for CcrStore {
    fn default() -> Self {
        Self::with_limits(512, DEFAULT_MAX_BYTES)
    }
}

impl CcrStore {
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_limits(capacity, usize::MAX)
    }

    pub fn with_limits(capacity: usize, max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            capacity: capacity.max(1),
            max_bytes: max_bytes.max(1),
            directory: None,
        }
    }

    pub fn disk_backed(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: Some(directory.into()),
            ..Self::default()
        }
    }

    /// Store an original, returning its marker hash. Idempotent: the same
    /// content always maps to the same hash (and refreshes its slot).
    pub fn put(&self, content: &str) -> String {
        let hash = hash_content(content);
        let mut inner = self.inner.lock().expect("ccr store lock");
        if !inner.entries.contains_key(&hash) {
            let bytes = content.len();
            if bytes > self.max_bytes {
                return hash;
            }
            while inner.order.len() >= self.capacity
                || inner.bytes.saturating_add(bytes) > self.max_bytes
            {
                let Some(oldest) = inner.order.pop_front() else {
                    break;
                };
                if let Some(entry) = inner.entries.remove(&oldest) {
                    inner.bytes = inner.bytes.saturating_sub(entry.bytes);
                    if let Some(directory) = &self.directory {
                        let _ = std::fs::remove_file(directory.join(&oldest));
                    }
                }
            }
            let on_disk = self.directory.as_ref().is_some_and(|directory| {
                std::fs::create_dir_all(directory).is_ok()
                    && std::fs::write(directory.join(&hash), content).is_ok()
            });
            inner.order.push_back(hash.clone());
            inner.bytes = inner.bytes.saturating_add(bytes);
            inner.entries.insert(
                hash.clone(),
                Entry {
                    content: (!on_disk).then(|| content.to_string()),
                    bytes,
                },
            );
        }
        hash
    }

    /// Look up a stored original by its marker hash.
    pub fn get(&self, hash: &str) -> Option<String> {
        let content = self
            .inner
            .lock()
            .expect("ccr store lock")
            .entries
            .get(hash)
            .map(|entry| entry.content.clone());
        match content {
            Some(Some(content)) => Some(content),
            Some(None) => self
                .directory
                .as_ref()
                .and_then(|directory| std::fs::read_to_string(directory.join(hash)).ok()),
            None => None,
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("ccr store lock").entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes_len(&self) -> usize {
        self.inner.lock().expect("ccr store lock").bytes
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
    fn byte_budget_evicts_even_below_the_entry_limit() {
        let store = CcrStore::with_limits(10, 6);
        let first = store.put("1234");
        let second = store.put("5678");
        assert!(store.get(&first).is_none());
        assert_eq!(store.get(&second).as_deref(), Some("5678"));
        assert!(store.bytes_len() <= 6);
    }

    #[test]
    fn a_single_oversized_value_is_not_retained() {
        let store = CcrStore::with_limits(10, 3);
        let hash = store.put("1234");
        assert!(store.get(&hash).is_none());
        assert_eq!(store.bytes_len(), 0);
    }

    #[test]
    fn disk_backed_store_keeps_originals_out_of_the_heap_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = CcrStore::disk_backed(dir.path());
        let hash = store.put("large original");
        assert_eq!(store.get(&hash).as_deref(), Some("large original"));
        assert!(dir.path().join(hash).is_file());
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
