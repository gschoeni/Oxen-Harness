//! On-disk storage for binary attachments, plus hydration back to data URIs.
//!
//! Images and PDFs used to be base64-encoded straight into the message JSON, so
//! the history database and JSONL exports ballooned with every screenshot. They
//! now live as content-addressed files under the *project* (so they're versioned
//! alongside the code the chat is about), and the message records only a path
//! relative to the project root.
//!
//! That keeps the transcript small but means a stored message can't be sent to
//! the model as-is — the provider needs the bytes inline. [`hydrate_content`]
//! reads each referenced file back and rebuilds the `data:` URI just before a
//! request goes out. References that are already inline (`data:`) or remote
//! (`http(s):`) — e.g. messages from before this change — pass through untouched.

use std::path::{Path, PathBuf};

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::attachment::{mime_for_extension, Attachment, AttachmentKind};
use crate::types::{ContentPart, MessageContent};

/// Subdirectory (relative to the project root) holding stored attachments.
const ATTACHMENTS_SUBDIR: &str = ".oxen-harness/attachments";
/// Per-request budget for rehydrated binary attachments, in **bytes on the
/// wire**: the size of the `data:` URIs as they land in the request body (and
/// in memory while it's built), not the raw file size. Base64 is 4/3 the raw
/// size, so this admits roughly 6 MiB of raw image/PDF bytes per request.
pub const MAX_OUTBOUND_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
/// Per-request count budget for historical binary attachments.
pub const MAX_OUTBOUND_ATTACHMENT_PARTS: usize = 4;

/// The `data:` URI scaffolding around the base64 payload.
const DATA_URI_SCHEME: &str = "data:";
const DATA_URI_BASE64_MARKER: &str = ";base64,";

/// The base64-encoded length of `raw_len` bytes (standard alphabet, padded):
/// four output bytes per three input bytes, the last group padded to four.
pub fn base64_len(raw_len: usize) -> usize {
    raw_len.div_ceil(3) * 4
}

/// How many bytes a stored file with extension `ext` occupies once hydrated
/// into a `data:` URI — the exact length [`hydrate_content`] produces, so the
/// outbound budget can be charged before the file is read.
pub fn hydrated_data_uri_len(ext: &str, raw_len: usize) -> usize {
    DATA_URI_SCHEME.len()
        + mime_for_extension(ext).len()
        + DATA_URI_BASE64_MARKER.len()
        + base64_len(raw_len)
}

/// Persists binary attachments under a project root and resolves their stored
/// paths back to bytes.
#[derive(Debug, Clone)]
pub struct AttachmentStore {
    root: PathBuf,
}

impl AttachmentStore {
    /// Create a store rooted at a project directory. Attachments live under
    /// `<root>/.oxen-harness/attachments/`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The project root this store is anchored to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Persist `bytes` content-addressed (sha256), returning the path relative to
    /// the project root that should be stored in the message. Writing is
    /// idempotent: identical bytes map to the same file and aren't rewritten.
    pub fn store_bytes(&self, ext: &str, bytes: &[u8]) -> std::io::Result<String> {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash = hex(&hasher.finalize());
        let name = if ext.is_empty() {
            hash
        } else {
            format!("{hash}.{ext}")
        };
        // Stored with forward slashes so the reference is portable across OSes.
        let rel = format!("{ATTACHMENTS_SUBDIR}/{name}");

        let abs = self.root.join(&rel);
        if !abs.exists() {
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs, bytes)?;
        }
        Ok(rel)
    }

    /// Convert an attachment into the content part to store on a message. Images
    /// and PDFs are written to disk and referenced by relative path; text, video,
    /// and opaque files keep the same inline rendering as
    /// [`Attachment::to_content_part`] (their content is either small text the
    /// model reads directly or just a note).
    pub fn store_part(&self, att: &Attachment) -> std::io::Result<ContentPart> {
        match att.kind {
            AttachmentKind::Image => {
                let rel = self.store_bytes(&att.extension(), &att.bytes)?;
                Ok(ContentPart::image(rel))
            }
            AttachmentKind::Pdf => {
                let rel = self.store_bytes(&att.extension(), &att.bytes)?;
                Ok(ContentPart::file(att.filename.clone(), rel))
            }
            _ => Ok(att.to_content_part()),
        }
    }
}

/// Whether a reference is already an inline `data:` URI or a remote `http(s):`
/// URL — i.e. needs no hydration.
fn is_inline_ref(value: &str) -> bool {
    value.starts_with("data:") || value.starts_with("http://") || value.starts_with("https://")
}

/// Rebuild the `data:` URI for a stored relative reference by reading its bytes
/// from `root`. Returns `None` (leave the reference unchanged) when it's already
/// inline/remote, and an error when the file can't be read.
fn hydrate_ref(value: &str, root: &Path) -> Option<std::io::Result<String>> {
    if is_inline_ref(value) {
        return None;
    }
    let path = root.join(value);
    Some(std::fs::read(&path).map(|bytes| encode_data_uri(extension_of(value), &bytes)))
}

/// The file extension of a stored reference (`""` when it has none).
fn extension_of(value: &str) -> &str {
    Path::new(value)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
}

/// Build `data:<mime>;base64,<payload>` for `bytes`, encoding straight into a
/// String pre-sized to the final length — so the only transient copies alive
/// are the raw bytes and the URI, never a third `format!` buffer holding the
/// encoded payload a second time.
fn encode_data_uri(ext: &str, bytes: &[u8]) -> String {
    let mut uri = String::with_capacity(hydrated_data_uri_len(ext, bytes.len()));
    uri.push_str(DATA_URI_SCHEME);
    uri.push_str(mime_for_extension(ext));
    uri.push_str(DATA_URI_BASE64_MARKER);
    base64::engine::general_purpose::STANDARD.encode_string(bytes, &mut uri);
    uri
}

/// Hydrate every stored attachment reference in a message's content to an inline
/// data URI, reading bytes from `root`. A reference whose file is missing is
/// replaced with a short text note so a broken request is never sent to the
/// provider. Plain-text content and already-inline references are left as-is.
pub fn hydrate_content(content: &mut MessageContent, root: &Path) {
    let mut bytes = usize::MAX;
    let mut parts = usize::MAX;
    hydrate_content_bounded(content, root, &mut bytes, &mut parts);
}

/// Hydrate attachments while enforcing a request-wide byte and part budget.
/// Callers walk newest messages first, so stale media is replaced before recent.
///
/// The byte budget is charged in wire bytes — the length of the `data:` URI
/// each part becomes — so an on-disk reference (charged via
/// [`hydrated_data_uri_len`] from its file size, before reading it) and an
/// already-inline `data:` part (charged at its actual length) are measured
/// the same way, and `remaining_bytes` bounds what the request body actually
/// carries.
pub fn hydrate_content_bounded(
    content: &mut MessageContent,
    root: &Path,
    remaining_bytes: &mut usize,
    remaining_parts: &mut usize,
) {
    let MessageContent::Parts(parts) = content else {
        return;
    };
    for part in parts.iter_mut() {
        let (slot, label) = match part {
            ContentPart::ImageUrl { image_url } => (&mut image_url.url, "image"),
            ContentPart::File { file } => (&mut file.file_data, "file"),
            ContentPart::Text { .. } => continue,
        };
        let inline_data = slot.starts_with("data:");
        if inline_data || !is_inline_ref(slot) {
            let size = if inline_data {
                // Already what goes on the wire.
                slot.len()
            } else {
                // What it will be once hydrated (base64 is 4/3 the file size
                // plus the URI prefix) — not the raw file size, which would
                // let ~33% more than the budget into the request.
                std::fs::metadata(root.join(&*slot))
                    .map(|m| hydrated_data_uri_len(extension_of(slot), m.len() as usize))
                    .unwrap_or(0)
            };
            if *remaining_parts == 0 || size > *remaining_bytes {
                let omitted = slot.clone();
                *part = ContentPart::text(format!(
                    "[older attached {label} `{omitted}` omitted from the active context]"
                ));
                continue;
            }
            *remaining_parts -= 1;
            *remaining_bytes -= size;
        }
        match hydrate_ref(slot, root) {
            None => {}
            Some(Ok(uri)) => *slot = uri,
            Some(Err(_)) => {
                let missing = slot.clone();
                *part = ContentPart::text(format!("[attached {label} `{missing}` is unavailable]"));
            }
        }
    }
}

/// Lower-case hex encoding of a byte slice.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_image_by_relative_path_and_hydrates_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path());

        let att = Attachment::from_bytes("shot.png", vec![1, 2, 3, 4]).unwrap();
        let part = store.store_part(&att).unwrap();

        // The stored part references a project-relative path, not a data URI.
        let rel = match &part {
            ContentPart::ImageUrl { image_url } => image_url.url.clone(),
            other => panic!("expected image part, got {other:?}"),
        };
        assert!(rel.starts_with(".oxen-harness/attachments/"));
        assert!(!rel.contains("data:"));
        assert!(dir.path().join(&rel).is_file());

        // Hydration turns it back into a data URI the provider can consume.
        let mut content = MessageContent::Parts(vec![part]);
        hydrate_content(&mut content, dir.path());
        match content {
            MessageContent::Parts(parts) => match &parts[0] {
                ContentPart::ImageUrl { image_url } => {
                    assert!(image_url.url.starts_with("data:image/png;base64,"))
                }
                other => panic!("expected image part, got {other:?}"),
            },
            other => panic!("expected parts, got {other:?}"),
        }
    }

    #[test]
    fn content_addressing_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path());
        let a = store.store_bytes("png", b"same bytes").unwrap();
        let b = store.store_bytes("png", b"same bytes").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn already_inline_references_pass_through() {
        let dir = tempfile::tempdir().unwrap();
        // An old transcript with an inline data URI must be left untouched.
        let original = "data:image/png;base64,AAAA".to_string();
        let mut content = MessageContent::Parts(vec![ContentPart::image(original.clone())]);
        hydrate_content(&mut content, dir.path());
        match content {
            MessageContent::Parts(parts) => match &parts[0] {
                ContentPart::ImageUrl { image_url } => assert_eq!(image_url.url, original),
                other => panic!("expected image part, got {other:?}"),
            },
            other => panic!("expected parts, got {other:?}"),
        }
    }

    #[test]
    fn missing_file_becomes_a_text_note() {
        let dir = tempfile::tempdir().unwrap();
        let mut content = MessageContent::Parts(vec![ContentPart::image(
            ".oxen-harness/attachments/deadbeef.png".to_string(),
        )]);
        hydrate_content(&mut content, dir.path());
        match content {
            MessageContent::Parts(parts) => match &parts[0] {
                ContentPart::Text { text } => assert!(text.contains("unavailable")),
                other => panic!("expected text note, got {other:?}"),
            },
            other => panic!("expected parts, got {other:?}"),
        }
    }

    #[test]
    fn hydration_omits_older_media_past_the_request_budget() {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path());
        let a = store.store_bytes("png", b"first").unwrap();
        let b = store.store_bytes("png", b"second").unwrap();
        let mut content = MessageContent::Parts(vec![ContentPart::image(a), ContentPart::image(b)]);
        let mut bytes = 1024;
        let mut parts = 1;
        hydrate_content_bounded(&mut content, dir.path(), &mut bytes, &mut parts);
        let MessageContent::Parts(parts) = content else {
            panic!("expected parts")
        };
        assert!(matches!(parts[0], ContentPart::ImageUrl { .. }));
        assert!(matches!(parts[1], ContentPart::Text { .. }));
    }

    #[test]
    fn hydrated_length_estimate_is_exact() {
        // The budget is charged from the estimate before the file is read, so
        // the estimate must equal the URI the hydrator actually builds — for
        // every base64 padding case and for a file with no extension.
        for len in [0usize, 1, 2, 3, 4, 5, 6, 100, 1023] {
            let bytes = vec![0xABu8; len];
            for ext in ["png", "jpg", "pdf", ""] {
                let uri = encode_data_uri(ext, &bytes);
                assert_eq!(
                    uri.len(),
                    hydrated_data_uri_len(ext, len),
                    "ext={ext} len={len}"
                );
                assert!(uri.starts_with("data:"));
                assert!(uri.contains(";base64,"));
            }
        }
        assert_eq!(base64_len(0), 0);
        assert_eq!(base64_len(1), 4);
        assert_eq!(base64_len(3), 4);
        assert_eq!(base64_len(4), 8);
    }

    #[test]
    fn on_disk_and_inline_parts_are_charged_the_same_wire_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path());
        let raw = vec![7u8; 3001];
        let rel = store.store_bytes("png", &raw).unwrap();

        // Hydrate once (unbounded) to learn the URI's real length.
        let mut hydrated = MessageContent::Parts(vec![ContentPart::image(rel.clone())]);
        hydrate_content(&mut hydrated, dir.path());
        let MessageContent::Parts(parts) = &hydrated else {
            panic!("expected parts")
        };
        let ContentPart::ImageUrl { image_url } = &parts[0] else {
            panic!("expected image")
        };
        let wire = image_url.url.len();
        assert!(wire > raw.len(), "the URI is bigger than the file");

        // The on-disk reference is charged exactly the wire size…
        let mut bytes = usize::MAX;
        let mut n = usize::MAX;
        let mut content = MessageContent::Parts(vec![ContentPart::image(rel)]);
        hydrate_content_bounded(&mut content, dir.path(), &mut bytes, &mut n);
        assert_eq!(usize::MAX - bytes, wire);

        // …and so is the same part when it arrives already inline.
        let mut bytes = usize::MAX;
        let mut n = usize::MAX;
        hydrate_content_bounded(&mut hydrated, dir.path(), &mut bytes, &mut n);
        assert_eq!(usize::MAX - bytes, wire);
    }

    #[test]
    fn budget_admits_a_file_just_under_six_mib_raw_and_omits_one_at_six_mib() {
        // 8 MiB on the wire ≈ 6 MiB raw. The largest PNG that fits is
        // 6 MiB − 18 bytes (8 MiB − 22-byte prefix, floored to a base64 group);
        // one byte more spills the encoded size past the budget.
        let dir = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(dir.path());
        let six_mib = 6 * 1024 * 1024;
        let under = store.store_bytes("png", &vec![1u8; six_mib - 18]).unwrap();
        let over = store.store_bytes("png", &vec![2u8; six_mib]).unwrap();
        assert!(hydrated_data_uri_len("png", six_mib - 18) <= MAX_OUTBOUND_ATTACHMENT_BYTES);
        assert!(hydrated_data_uri_len("png", six_mib - 17) > MAX_OUTBOUND_ATTACHMENT_BYTES);
        // (Under the old raw-size accounting a 6 MiB file would have fit and
        // then put 8 MiB + prefix into the request.)
        assert!(six_mib <= MAX_OUTBOUND_ATTACHMENT_BYTES);

        let fits = |rel: &str| {
            let mut bytes = MAX_OUTBOUND_ATTACHMENT_BYTES;
            let mut parts = MAX_OUTBOUND_ATTACHMENT_PARTS;
            let mut content = MessageContent::Parts(vec![ContentPart::image(rel.to_string())]);
            hydrate_content_bounded(&mut content, dir.path(), &mut bytes, &mut parts);
            let MessageContent::Parts(parts) = content else {
                panic!("expected parts")
            };
            match &parts[0] {
                ContentPart::ImageUrl { image_url } => {
                    assert!(image_url.url.starts_with("data:image/png;base64,"));
                    assert!(image_url.url.len() <= MAX_OUTBOUND_ATTACHMENT_BYTES);
                    true
                }
                ContentPart::Text { text } => {
                    assert!(text.contains("omitted from the active context"));
                    false
                }
                other => panic!("unexpected part {other:?}"),
            }
        };
        assert!(
            fits(&under),
            "6 MiB − 18 raw (8 MiB − 2 on the wire) must fit"
        );
        assert!(
            !fits(&over),
            "6 MiB raw (> 8 MiB on the wire) must be omitted"
        );
    }
}
