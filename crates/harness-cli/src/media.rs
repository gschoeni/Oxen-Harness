//! Dropped/pasted media files as `[Image #N]`-style chips instead of raw paths.
//!
//! Dropping or pasting a media file path (image, PDF, or video) stages the file
//! in a session-wide registry and leaves a short chip — `[Image #1]`,
//! `[PDF #1]`, `[Video #1]` — in the composer in place of the path. Ctrl+V goes
//! one further: bracketed paste can only carry *text*, so a copied screenshot
//! never reaches the terminal — Ctrl+V reads the system clipboard directly,
//! encodes the image to a temp PNG, and stages it the same way.
//!
//! At submit time [`resolve_labels`] turns the chips back into [`Attachment`]s
//! that ride the normal attachment pipeline. The chip text itself stays in the
//! prompt, so "what's in `[Image #2]`?" reaches the model next to the matching
//! image part. Chips number up per kind across the whole session — recalling an
//! old prompt from history re-attaches its files.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_llm::{Attachment, AttachmentKind};

/// A media file staged behind a chip label.
struct Staged {
    label: String,
    path: PathBuf,
}

/// Staged media for this session, looked up by exact chip label.
static STAGED: Mutex<Vec<Staged>> = Mutex::new(Vec::new());

/// The chip word for a media kind the model receives as a real part (or a
/// deliberate note, for video). Text/other files are never chipped — they stay
/// visible as paths for the agent's file tools.
fn chip_word(kind: AttachmentKind) -> Option<&'static str> {
    match kind {
        AttachmentKind::Image => Some("Image"),
        AttachmentKind::Pdf => Some("PDF"),
        AttachmentKind::Video => Some("Video"),
        AttachmentKind::Text | AttachmentKind::Other => None,
    }
}

/// Whether this path names a stageable media file (by extension).
fn media_word(path: &Path) -> Option<&'static str> {
    chip_word(AttachmentKind::from_path(path))
}

/// Register a media file path and return its chip label, e.g. `[Image #3]`.
/// Chips number up independently per kind. Paths whose extension doesn't
/// classify as media fall back to the `Image` word (only reachable from tests
/// staging fake files).
pub(crate) fn stage_path(path: impl Into<PathBuf>) -> String {
    let path = path.into();
    let word = media_word(&path).unwrap_or("Image");
    let open = format!("[{word} #");
    let mut staged = STAGED.lock().expect("media registry poisoned");
    let n = staged.iter().filter(|s| s.label.starts_with(&open)).count() + 1;
    let label = format!("{open}{n}]");
    staged.push(Staged {
        label: label.clone(),
        path,
    });
    label
}

/// The path staged behind a chip label, if that label was ever handed out.
fn staged_path(label: &str) -> Option<PathBuf> {
    let staged = STAGED.lock().expect("media registry poisoned");
    staged
        .iter()
        .find(|s| s.label == label)
        .map(|s| s.path.clone())
}

/// Rewrite pasted text, replacing tokens that point at existing media files
/// (images, PDFs, videos) with fresh chips. Returns `None` when the paste holds
/// no media path — insert it verbatim, since re-joining tokens would collapse
/// spacing.
pub(crate) fn rewrite_paste(text: &str) -> Option<String> {
    let mut replaced = false;
    let tokens: Vec<String> = crate::attach::tokenize(text)
        .into_iter()
        .map(|tok| {
            let p = Path::new(&tok);
            // Extension first: it's free, and `is_file` is a stat syscall this
            // runs per token (including on every composer keystroke check).
            if media_word(p).is_some() && p.is_file() {
                replaced = true;
                stage_path(p)
            } else {
                tok
            }
        })
        .collect();
    replaced.then(|| tokens.join(" "))
}

/// Load the attachments behind every chip label in `text`, in order of first
/// appearance. A label that was never handed out is left alone (the user may
/// have typed those words); one whose staged file can't be read anymore becomes
/// a warning.
pub(crate) fn resolve_labels(text: &str) -> (Vec<Attachment>, Vec<String>) {
    let mut attachments = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for label in scan_labels(text) {
        if !seen.insert(label.clone()) {
            continue;
        }
        let Some(path) = staged_path(&label) else {
            continue;
        };
        match Attachment::from_path(&path) {
            Ok(a) => attachments.push(a),
            Err(e) => warnings.push(format!("{label}: {e}")),
        }
    }
    (attachments, warnings)
}

/// Every well-formed chip label in `text` (`[Image #N]` / `[PDF #N]` /
/// `[Video #N]`), in order of appearance.
fn scan_labels(text: &str) -> Vec<String> {
    let mut found: Vec<(usize, String)> = Vec::new();
    for word in ["Image", "PDF", "Video"] {
        let open = format!("[{word} #");
        let mut from = 0;
        while let Some(i) = text[from..].find(&open) {
            let start = from + i;
            let rest = &text[start + open.len()..];
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 && rest[digits..].starts_with(']') {
                let end = start + open.len() + digits + 1;
                found.push((start, text[start..end].to_string()));
            }
            from = start + open.len();
        }
    }
    // Interleave the per-word scans back into textual order.
    found.sort_by_key(|(start, _)| *start);
    found.into_iter().map(|(_, label)| label).collect()
}

/// What Ctrl+V found on the system clipboard.
pub(crate) enum ClipboardPaste {
    /// An image was staged; insert this `[Image #N]` chip.
    Image(String),
    /// Plain text — insert it like any other paste.
    Text(String),
    /// Nothing usable (empty clipboard, no display server, read error).
    None,
}

/// Read the system clipboard for Ctrl+V: an image stages as an `[Image #N]`
/// chip; copied files paste as their paths (media paths become chips on
/// insert); text falls through as an ordinary paste (for terminals that don't
/// intercept Ctrl+V themselves). Best-effort — any failure is just `None`.
pub(crate) fn paste_from_clipboard() -> ClipboardPaste {
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return ClipboardPaste::None;
    };
    if let Ok(image) = clipboard.get_image() {
        if let Ok(label) = stage_clipboard_image(&image) {
            return ClipboardPaste::Image(label);
        }
    }
    // Some producers put only PNG-family flavors on the macOS pasteboard (no
    // TIFF), which arboard can't read — ask the pasteboard for PNG directly.
    #[cfg(target_os = "macos")]
    if let Some(bytes) = clipboard_png_via_osascript() {
        if let Ok(label) = stage_png_bytes(&bytes) {
            return ClipboardPaste::Image(label);
        }
    }
    // A copied file (e.g. Finder ⌘C) pastes as its path — escaped like a
    // drag-drop, so `insert_paste`'s rewrite turns media paths into chips.
    if let Ok(files) = clipboard.get().file_list() {
        let joined = files
            .iter()
            .map(|p| p.display().to_string().replace(' ', "\\ "))
            .collect::<Vec<_>>()
            .join(" ");
        if !joined.is_empty() {
            return ClipboardPaste::Text(joined);
        }
    }
    match clipboard.get_text() {
        Ok(text) if !text.is_empty() => ClipboardPaste::Text(text),
        _ => ClipboardPaste::None,
    }
}

/// A fresh `clipboard-N.png` path under this process's temp dir. The file only
/// has to outlive the prompt it's pasted into — resolution reads it back at
/// submit time.
fn next_clipboard_png_path() -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("oxen-harness-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let next = STAGED.lock().expect("media registry poisoned").len() + 1;
    Ok(dir.join(format!("clipboard-{next}.png")))
}

/// Encode the clipboard's RGBA pixels as a PNG on disk and stage it.
fn stage_clipboard_image(image: &arboard::ImageData) -> anyhow::Result<String> {
    let path = next_clipboard_png_path()?;
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width as u32, image.height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.bytes)?;
    Ok(stage_path(path))
}

/// Write ready-made PNG bytes to disk and stage them.
#[cfg(target_os = "macos")]
fn stage_png_bytes(bytes: &[u8]) -> anyhow::Result<String> {
    let path = next_clipboard_png_path()?;
    std::fs::write(&path, bytes)?;
    Ok(stage_path(path))
}

/// Ask the macOS pasteboard for its PNG flavor directly. `osascript` prints it
/// as `«data PNGf<hex…>»`; decode the hex back into bytes.
#[cfg(target_os = "macos")]
fn clipboard_png_via_osascript() -> Option<Vec<u8>> {
    let out = std::process::Command::new("osascript")
        .args(["-e", "the clipboard as «class PNGf»"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let hex = text.trim().strip_prefix("«data PNGf")?.strip_suffix('»')?;
    if hex.is_empty() || hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A tiny on-disk file (contents don't matter — classification is by
    /// extension and resolution just reads bytes).
    fn temp_file(dir: &tempfile::TempDir, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&[1, 2, 3])
            .unwrap();
        path
    }

    #[test]
    fn scan_labels_finds_well_formed_labels_only() {
        assert_eq!(
            scan_labels("see [Image #1], [PDF #2] and [Video #23]."),
            vec!["[Image #1]", "[PDF #2]", "[Video #23]"]
        );
        assert!(scan_labels("[Image #] [Image x] [image #1] [Image #1").is_empty());
    }

    #[test]
    fn staged_label_round_trips_to_an_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let label = stage_path(temp_file(&dir, "shot.png"));
        let (attachments, warnings) = resolve_labels(&format!("what is in {label}?"));
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, AttachmentKind::Image);
        assert_eq!(attachments[0].filename, "shot.png");
        assert!(warnings.is_empty());

        // The same label twice attaches once.
        let (attachments, _) = resolve_labels(&format!("{label} vs {label}"));
        assert_eq!(attachments.len(), 1);
    }

    #[test]
    fn each_kind_gets_its_own_chip_word_and_numbering() {
        let dir = tempfile::tempdir().unwrap();
        let img = stage_path(temp_file(&dir, "a.png"));
        let pdf = stage_path(temp_file(&dir, "b.pdf"));
        let vid = stage_path(temp_file(&dir, "c.mov"));
        assert!(img.starts_with("[Image #"), "{img}");
        assert!(pdf.starts_with("[PDF #"), "{pdf}");
        assert!(vid.starts_with("[Video #"), "{vid}");

        // A second PDF numbers up within its own kind. (Tests share the global
        // registry and run concurrently, so only ordering is asserted.)
        let pdf2 = stage_path(temp_file(&dir, "d.pdf"));
        let n = |s: &str| -> usize {
            s.trim_end_matches(']')
                .rsplit('#')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        assert!(n(&pdf2) > n(&pdf), "{pdf2} vs {pdf}");

        let (attachments, warnings) = resolve_labels(&format!("{img} {pdf} {vid} {pdf2}"));
        assert_eq!(attachments.len(), 4);
        assert_eq!(attachments[1].kind, AttachmentKind::Pdf);
        assert_eq!(attachments[2].kind, AttachmentKind::Video);
        assert!(warnings.is_empty());
    }

    #[test]
    fn unknown_labels_resolve_to_nothing_and_missing_files_warn() {
        // Never handed out — the user just typed those words.
        let (attachments, warnings) = resolve_labels("look at [Image #987654]");
        assert!(attachments.is_empty());
        assert!(warnings.is_empty());

        // Handed out, but the file has since vanished.
        let dir = tempfile::tempdir().unwrap();
        let path = temp_file(&dir, "gone.png");
        let label = stage_path(&path);
        std::fs::remove_file(&path).unwrap();
        let (attachments, warnings) = resolve_labels(&label);
        assert!(attachments.is_empty());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn rewrite_paste_replaces_media_paths_and_leaves_plain_text_alone() {
        let dir = tempfile::tempdir().unwrap();
        let img = temp_file(&dir, "drop.png");

        let rewritten = rewrite_paste(&format!("describe {}\n", img.display())).unwrap();
        let labels = scan_labels(&rewritten);
        assert_eq!(labels.len(), 1);
        assert_eq!(rewritten, format!("describe {}", labels[0]));
        // The chip resolves back to the dropped file.
        let (attachments, _) = resolve_labels(&rewritten);
        assert_eq!(attachments[0].filename, "drop.png");

        // PDFs and videos chip too.
        let pdf = temp_file(&dir, "paper.pdf");
        let vid = temp_file(&dir, "clip.mp4");
        let rewritten = rewrite_paste(&format!("{} and {}", pdf.display(), vid.display())).unwrap();
        assert!(rewritten.contains("[PDF #"), "{rewritten}");
        assert!(rewritten.contains("[Video #"), "{rewritten}");
        let (attachments, _) = resolve_labels(&rewritten);
        assert_eq!(attachments.len(), 2);

        // No media path → verbatim insert (None), even for other real files.
        assert_eq!(rewrite_paste("just some  spaced   text"), None);
        assert_eq!(rewrite_paste("/not/a/real/file.png"), None);
        let doc = dir.path().join("notes.md");
        std::fs::write(&doc, "hi").unwrap();
        assert_eq!(rewrite_paste(&doc.display().to_string()), None);
    }
}
