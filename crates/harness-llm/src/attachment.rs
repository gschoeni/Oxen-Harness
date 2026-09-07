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
use harness_core::fmt::format_bytes;

use crate::types::ContentPart;

/// Largest file we'll inline as a data URI (20 MiB). Bigger files would blow up
/// the request body and the context budget, so they're rejected with a clear error.
pub const MAX_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

/// How many characters of a text document we inline before truncating. A full
/// 20 MiB text file would swamp the context window, so we send a generous head
/// and flag that it was cut.
pub const MAX_TEXT_CHARS: usize = 100_000;

/// The longest edge an image is sent at. Vision models downsample anything
/// larger themselves (Anthropic's documented ceiling is 1568 px), so bytes
/// past this are upload time and request size for no extra detail — a 12 MP
/// screenshot becomes a few hundred kilobytes.
pub const MAX_IMAGE_EDGE: u32 = 1568;

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
/// fresh attachments and when [hydrating](crate::hydrate_content) stored ones). Defaults
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
    /// Pixel size of an image as it will be sent (after any downscale), for
    /// UIs to show next to the chip. `None` for non-images and undecodable
    /// formats (HEIC).
    pub dimensions: Option<(u32, u32)>,
    /// The image's size before downscaling, when it was shrunk.
    pub original_dimensions: Option<(u32, u32)>,
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
        let mut attachment = Self {
            mime: kind.mime(ext).to_string(),
            kind,
            bytes,
            filename,
            dimensions: None,
            original_dimensions: None,
        };
        if kind == AttachmentKind::Image {
            attachment.fit_image();
        }
        Ok(attachment)
    }

    /// Record an image's dimensions and shrink it to [`MAX_IMAGE_EDGE`] when
    /// it is larger, re-encoding as PNG (sources with transparency) or JPEG.
    /// Undecodable images (HEIC, corrupt files) are sent as they are.
    fn fit_image(&mut self) {
        let reader = match image::ImageReader::new(std::io::Cursor::new(&self.bytes))
            .with_guessed_format()
        {
            Ok(reader) => reader,
            Err(_) => return,
        };
        let Ok((width, height)) = reader.into_dimensions() else {
            return;
        };
        self.dimensions = Some((width, height));
        if width.max(height) <= MAX_IMAGE_EDGE {
            return;
        }
        let Ok(decoded) = image::load_from_memory(&self.bytes) else {
            return;
        };
        let scaled = decoded.resize(
            MAX_IMAGE_EDGE,
            MAX_IMAGE_EDGE,
            image::imageops::FilterType::Triangle,
        );
        // Only a picture that actually uses transparency stays PNG; a
        // screenshot with an opaque alpha channel is a photo for our purposes.
        let keep_alpha =
            scaled.color().has_alpha() && scaled.to_rgba8().pixels().any(|px| px[3] < u8::MAX);
        let mut out = std::io::Cursor::new(Vec::new());
        let encoded = if keep_alpha {
            scaled
                .write_to(&mut out, image::ImageFormat::Png)
                .map(|_| "image/png")
        } else {
            scaled
                .to_rgb8()
                .write_to(&mut out, image::ImageFormat::Jpeg)
                .map(|_| "image/jpeg")
        };
        if let Ok(mime) = encoded {
            self.original_dimensions = Some((width, height));
            self.dimensions = Some((scaled.width(), scaled.height()));
            self.mime = mime.to_string();
            self.bytes = out.into_inner();
        }
    }

    /// `1024×768`, when the attachment is an image with known dimensions.
    pub fn dimensions_label(&self) -> Option<String> {
        self.dimensions.map(|(w, h)| format!("{w}×{h}"))
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
                    format_bytes(self.bytes.len() as u64),
                ))
            }
            AttachmentKind::Video => ContentPart::text(format!(
                "[Attached video `{}` ({}). The model can't watch video; \
                 describe what you need from it.]",
                self.filename,
                format_bytes(self.bytes.len() as u64),
            )),
            AttachmentKind::Other => ContentPart::text(format!(
                "[Attached file `{}` ({}), which isn't a supported image/PDF.]",
                self.filename,
                format_bytes(self.bytes.len() as u64),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn small_images_keep_their_bytes_and_report_dimensions() {
        let bytes = png(640, 480);
        let a = Attachment::from_bytes("shot.png", bytes.clone()).unwrap();
        assert_eq!(a.dimensions, Some((640, 480)));
        assert_eq!(a.original_dimensions, None);
        assert_eq!(a.bytes, bytes);
        assert_eq!(a.dimensions_label().as_deref(), Some("640×480"));
    }

    #[test]
    fn oversized_images_are_shrunk_to_the_edge_limit() {
        let a = Attachment::from_bytes("big.png", png(4000, 2000)).unwrap();
        assert_eq!(a.original_dimensions, Some((4000, 2000)));
        assert_eq!(a.dimensions, Some((MAX_IMAGE_EDGE, MAX_IMAGE_EDGE / 2)));
        // Opaque pixels: re-encoded as JPEG, far smaller than the source.
        assert_eq!(a.mime, "image/jpeg");
        assert!(a.data_uri().starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn an_undecodable_image_is_sent_as_is() {
        let a = Attachment::from_bytes("photo.heic", vec![0, 1, 2, 3, 4]).unwrap();
        assert_eq!(a.kind, AttachmentKind::Image);
        assert_eq!(a.dimensions, None);
        assert_eq!(a.bytes, vec![0, 1, 2, 3, 4]);
    }

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
}
