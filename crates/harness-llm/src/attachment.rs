//! Turning dropped files (images, PDFs, text documents, videos) into chat
//! [`ContentPart`]s.
//!
//! The CLI and desktop app let a user drag files into the chat. An
//! [`Attachment`] reads the file, classifies it, and serializes it the way the
//! model expects: images as `image_url` data URIs, PDFs as `file` data URIs,
//! text-based documents (Markdown, CSV, source, …) inlined as text, and
//! anything the model can't see natively (video, opaque binaries) as a short
//! text note so the conversation still records that it was attached.

use std::path::Path;

use base64::Engine;

use crate::types::ContentPart;

/// Largest file we'll inline as a data URI (20 MiB). Bigger files would blow up
/// the request body and the context budget, so they're rejected with a clear error.
pub const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

/// How many characters of a text document we inline before truncating. A full
/// 20 MiB text file would swamp the context window, so we send a generous head
/// and flag that it was cut.
pub const MAX_TEXT_CHARS: usize = 100_000;

/// Errors from reading or validating an attachment.
#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error("could not read attachment `{path}`: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("attachment `{0}` is empty")]
    Empty(String),
    #[error("attachment `{path}` is {size} bytes, over the {max} byte limit")]
    TooLarge { path: String, size: u64, max: u64 },
}

/// How an attachment is conveyed to the model, decided by file type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// A raster image the model can view (`image_url` part).
    Image,
    /// A PDF document (`file` part).
    Pdf,
    /// A text-based document (Markdown, CSV, JSON, source code, …) whose
    /// contents are inlined as text so the model can read them directly.
    Text,
    /// A video — not viewable by the model; sent as a text note.
    Video,
    /// An opaque binary the model can't read; sent as a text note.
    Other,
}

impl AttachmentKind {
    /// Classify by lower-cased file extension. Note that [`Self::Text`] is never
    /// returned here: whether an otherwise-unknown file is a readable text
    /// document or an opaque binary is decided by sniffing its bytes in
    /// [`Attachment::from_bytes`], not by its extension.
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "heic" => Self::Image,
            "pdf" => Self::Pdf,
            "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v" | "gif_video" => Self::Video,
            _ => Self::Other,
        }
    }

    /// Classify a path by its extension (defaulting to [`Self::Other`]).
    pub fn from_path(path: &Path) -> Self {
        path.extension()
            .and_then(|e| e.to_str())
            .map(Self::from_extension)
            .unwrap_or(Self::Other)
    }

    /// The MIME type to advertise in the data URI.
    fn mime(self, ext: &str) -> &'static str {
        mime_for_extension(ext)
    }
}

/// The MIME type for a file extension, used when building `data:` URIs (both for
/// fresh attachments and when [hydrating](hydrate_content) stored ones). Defaults
/// to `application/octet-stream` for anything unrecognized.
pub fn mime_for_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        _ => "application/octet-stream",
    }
}

/// A file the user attached to a chat message, ready to become a [`ContentPart`].
#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    pub kind: AttachmentKind,
    pub mime: String,
    pub bytes: Vec<u8>,
}

impl Attachment {
    /// Read and classify a file from disk.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, AttachmentError> {
        let path = path.as_ref();
        let display = path.display().to_string();
        let meta = std::fs::metadata(path).map_err(|source| AttachmentError::Read {
            path: display.clone(),
            source,
        })?;
        if meta.len() > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentError::TooLarge {
                path: display,
                size: meta.len(),
                max: MAX_ATTACHMENT_BYTES,
            });
        }
        let bytes = std::fs::read(path).map_err(|source| AttachmentError::Read {
            path: display.clone(),
            source,
        })?;
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("attachment")
            .to_string();
        Self::from_bytes(filename, bytes)
    }

    /// Build from a filename and raw bytes already in memory (e.g. a UI drop).
    pub fn from_bytes(
        filename: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Result<Self, AttachmentError> {
        let filename = filename.into();
        if bytes.is_empty() {
            return Err(AttachmentError::Empty(filename));
        }
        if bytes.len() as u64 > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentError::TooLarge {
                path: filename,
                size: bytes.len() as u64,
                max: MAX_ATTACHMENT_BYTES,
            });
        }
        let ext = Path::new(&filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        // Extensions name the media types; an unknown extension might still be a
        // readable text document (Markdown, CSV, source, an extension-less
        // README), so fall back to sniffing the bytes.
        let kind = match AttachmentKind::from_extension(ext) {
            AttachmentKind::Other if looks_like_text(&bytes) => AttachmentKind::Text,
            kind => kind,
        };
        Ok(Self {
            mime: kind.mime(ext).to_string(),
            kind,
            bytes,
            filename,
        })
    }

    /// A `data:<mime>;base64,<...>` URI carrying the file's bytes.
    pub fn data_uri(&self) -> String {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&self.bytes);
        format!("data:{};base64,{}", self.mime, b64)
    }

    /// The lower-cased file extension (without the dot), e.g. `png`. Empty if the
    /// filename has none.
    pub fn extension(&self) -> String {
        Path::new(&self.filename)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default()
    }

    /// The content part to send to the model. Images become `image_url` parts and
    /// PDFs `file` parts; videos and unknown types become a text note, since the
    /// model can't view them directly.
    pub fn to_content_part(&self) -> ContentPart {
        match self.kind {
            AttachmentKind::Image => ContentPart::image(self.data_uri()),
            AttachmentKind::Pdf => ContentPart::file(self.filename.clone(), self.data_uri()),
            AttachmentKind::Text => {
                let (body, truncated) = truncate_text(&String::from_utf8_lossy(&self.bytes));
                let note = if truncated {
                    format!(" — showing the first {MAX_TEXT_CHARS} characters")
                } else {
                    String::new()
                };
                ContentPart::text(format!(
                    "[Attached document `{}` ({}){note}]\n{body}",
                    self.filename,
                    human_size(self.bytes.len() as u64),
                ))
            }
            AttachmentKind::Video => ContentPart::text(format!(
                "[Attached video `{}` ({}). The model can't watch video; \
                 describe what you need from it.]",
                self.filename,
                human_size(self.bytes.len() as u64),
            )),
            AttachmentKind::Other => ContentPart::text(format!(
                "[Attached file `{}` ({}), which isn't a supported image/PDF.]",
                self.filename,
                human_size(self.bytes.len() as u64),
            )),
        }
    }
}

/// Whether a file's bytes look like a readable text document rather than an
/// opaque binary: valid UTF-8 with no embedded NUL byte (the classic cheap
/// text/binary heuristic). Empty input is rejected earlier, so this only sees
/// real content.
fn looks_like_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// Cap inlined document text at [`MAX_TEXT_CHARS`], returning the (possibly
/// shortened) text and whether it was truncated. Splits on a char boundary so
/// multi-byte UTF-8 is never cut mid-character.
fn truncate_text(text: &str) -> (String, bool) {
    if text.chars().count() <= MAX_TEXT_CHARS {
        return (text.to_string(), false);
    }
    (text.chars().take(MAX_TEXT_CHARS).collect(), true)
}

/// A compact human-readable byte size like `1.2 MB`.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(AttachmentKind::from_extension("PNG"), AttachmentKind::Image);
        assert_eq!(
            AttachmentKind::from_extension("jpeg"),
            AttachmentKind::Image
        );
        assert_eq!(AttachmentKind::from_extension("pdf"), AttachmentKind::Pdf);
        assert_eq!(AttachmentKind::from_extension("mov"), AttachmentKind::Video);
        assert_eq!(AttachmentKind::from_extension("txt"), AttachmentKind::Other);
        assert_eq!(
            AttachmentKind::from_path(Path::new("a/b/photo.JPG")),
            AttachmentKind::Image
        );
    }

    #[test]
    fn image_becomes_an_image_data_uri_part() {
        let a = Attachment::from_bytes("shot.png", vec![1, 2, 3]).unwrap();
        assert_eq!(a.kind, AttachmentKind::Image);
        assert_eq!(a.mime, "image/png");
        let uri = a.data_uri();
        assert!(uri.starts_with("data:image/png;base64,"));
        match a.to_content_part() {
            ContentPart::ImageUrl { image_url } => {
                assert!(image_url.url.starts_with("data:image/png;base64,"))
            }
            other => panic!("expected image part, got {other:?}"),
        }
    }

    #[test]
    fn pdf_becomes_a_file_part_with_filename() {
        let a = Attachment::from_bytes("paper.pdf", b"%PDF-1.4".to_vec()).unwrap();
        match a.to_content_part() {
            ContentPart::File { file } => {
                assert_eq!(file.filename, "paper.pdf");
                assert!(file.file_data.starts_with("data:application/pdf;base64,"));
            }
            other => panic!("expected file part, got {other:?}"),
        }
    }

    #[test]
    fn text_document_is_inlined_as_text() {
        let a = Attachment::from_bytes("notes.md", b"# Title\n\nhello world".to_vec()).unwrap();
        assert_eq!(a.kind, AttachmentKind::Text);
        match a.to_content_part() {
            ContentPart::Text { text } => {
                assert!(text.contains("notes.md"), "should name the file: {text}");
                assert!(text.contains("# Title"), "should inline the body: {text}");
                assert!(text.contains("hello world"));
            }
            other => panic!("expected text part, got {other:?}"),
        }
    }

    #[test]
    fn extensionless_text_is_detected_by_sniffing() {
        let a = Attachment::from_bytes("README", b"plain readme".to_vec()).unwrap();
        assert_eq!(a.kind, AttachmentKind::Text);
    }

    #[test]
    fn opaque_binary_stays_other_not_text() {
        // A NUL byte and invalid UTF-8 mark this as binary.
        let a = Attachment::from_bytes("blob.dat", vec![0x00, 0xff, 0xfe, 0x01]).unwrap();
        assert_eq!(a.kind, AttachmentKind::Other);
        match a.to_content_part() {
            ContentPart::Text { text } => assert!(text.contains("isn't a supported")),
            other => panic!("expected text note, got {other:?}"),
        }
    }

    #[test]
    fn long_text_document_is_truncated_with_a_note() {
        let big = "a".repeat(MAX_TEXT_CHARS + 500);
        let a = Attachment::from_bytes("big.txt", big.into_bytes()).unwrap();
        match a.to_content_part() {
            ContentPart::Text { text } => {
                assert!(text.contains("showing the first"), "note: {}", &text[..80]);
                // The body (after the header line) is exactly MAX_TEXT_CHARS of 'a'.
                let body = text.split_once('\n').expect("header then body").1;
                assert_eq!(body.chars().count(), MAX_TEXT_CHARS);
                assert!(body.chars().all(|c| c == 'a'));
            }
            other => panic!("expected text part, got {other:?}"),
        }
    }

    #[test]
    fn video_becomes_a_text_note() {
        let a = Attachment::from_bytes("clip.mp4", vec![0u8; 10]).unwrap();
        assert_eq!(a.kind, AttachmentKind::Video);
        match a.to_content_part() {
            ContentPart::Text { text } => {
                assert!(text.contains("clip.mp4"));
                assert!(text.contains("can't watch video"));
            }
            other => panic!("expected text note, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_files() {
        assert!(matches!(
            Attachment::from_bytes("empty.png", vec![]),
            Err(AttachmentError::Empty(_))
        ));
    }

    #[test]
    fn human_size_is_compact() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
