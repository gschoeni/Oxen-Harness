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
/// Per-request byte budget for rehydrated binary attachments.
pub const MAX_OUTBOUND_ATTACHMENT_BYTES: usize = 32 * 1024 * 1024;
/// Per-request count budget for historical binary attachments.
pub const MAX_OUTBOUND_ATTACHMENT_PARTS: usize = 4;

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
    Some(std::fs::read(&path).map(|bytes| {
        let ext = Path::new(value)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        format!("data:{};base64,{}", mime_for_extension(ext), b64)
    }))
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
                // Base64 is 4/3 of the source bytes; using encoded length is a
                // conservative request-memory budget and needs no decoding.
                slot.len()
            } else {
                std::fs::metadata(root.join(&*slot))
                    .map(|m| m.len() as usize)
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
}
