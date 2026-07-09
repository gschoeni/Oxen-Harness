//! Pasted images as `[Image N]` chips instead of raw paths.
//!
//! Pasting an image file path (drag-drop or a copied path) stages the image in
//! a session-wide registry and leaves a short `[Image N]` label in the composer
//! in place of the path. Ctrl+V goes one further: bracketed paste can only
//! carry *text*, so a copied screenshot never reaches the terminal — Ctrl+V
//! reads the system clipboard directly, encodes the image to a temp PNG, and
//! stages it the same way.
//!
//! At submit time [`resolve_labels`] turns the labels back into
//! [`Attachment`]s that ride the normal attachment pipeline. The label text
//! itself stays in the prompt, so "what's in [Image 2]?" reaches the model next
//! to the matching image part. Labels number up across the whole session —
//! recalling an old prompt from history re-attaches its images.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_llm::{Attachment, AttachmentKind};

/// Staged images for this session; index `i` backs the label `[Image i+1]`.
static STAGED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Register an image path and return its `[Image N]` label.
pub(crate) fn stage_path(path: impl Into<PathBuf>) -> String {
    let mut staged = STAGED.lock().expect("images registry poisoned");
    staged.push(path.into());
    format!("[Image {}]", staged.len())
}

/// The path staged behind `[Image n]`, if that label was ever handed out.
fn staged_path(n: usize) -> Option<PathBuf> {
    let staged = STAGED.lock().expect("images registry poisoned");
    n.checked_sub(1).and_then(|i| staged.get(i).cloned())
}

/// Rewrite pasted text, replacing tokens that point at existing image files
/// with fresh `[Image N]` labels. Returns `None` when the paste holds no image
/// path — insert it verbatim, since re-joining tokens would collapse spacing.
pub(crate) fn rewrite_paste(text: &str) -> Option<String> {
    let mut replaced = false;
    let tokens: Vec<String> = crate::attach::tokenize(text)
        .into_iter()
        .map(|tok| {
            let p = Path::new(&tok);
            if p.is_file() && AttachmentKind::from_path(p) == AttachmentKind::Image {
                replaced = true;
                stage_path(p)
            } else {
                tok
            }
        })
        .collect();
    replaced.then(|| tokens.join(" "))
}

/// Load the attachments behind every `[Image N]` label in `text`, in order of
/// first appearance. A label that was never handed out is left alone (the user
/// may have typed those words); one whose staged file can't be read anymore
/// becomes a warning.
pub(crate) fn resolve_labels(text: &str) -> (Vec<Attachment>, Vec<String>) {
    let mut attachments = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for n in scan_labels(text) {
        if !seen.insert(n) {
            continue;
        }
        let Some(path) = staged_path(n) else {
            continue;
        };
        match Attachment::from_path(&path) {
            Ok(a) => attachments.push(a),
            Err(e) => warnings.push(format!("[Image {n}]: {e}")),
        }
    }
    (attachments, warnings)
}

/// The `N`s of every well-formed `[Image N]` in `text`, in order.
fn scan_labels(text: &str) -> Vec<usize> {
    const OPEN: &str = "[Image ";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(OPEN) {
        rest = &rest[i + OPEN.len()..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with(']') {
            if let Ok(n) = rest[..digits].parse() {
                out.push(n);
            }
        }
    }
    out
}

/// What Ctrl+V found on the system clipboard.
pub(crate) enum ClipboardPaste {
    /// An image was staged; insert this `[Image N]` label.
    Image(String),
    /// Plain text — insert it like any other paste.
    Text(String),
    /// Nothing usable (empty clipboard, no display server, read error).
    None,
}

/// Read the system clipboard for Ctrl+V: an image stages as an `[Image N]`
/// chip; copied files paste as their paths (image paths become chips on
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
    // drag-drop, so `insert_paste`'s rewrite turns image paths into chips.
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
    let next = STAGED.lock().expect("images registry poisoned").len() + 1;
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

    /// A tiny on-disk "PNG" (contents don't matter — classification is by
    /// extension and resolution just reads bytes).
    fn temp_png(dir: &tempfile::TempDir, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&[1, 2, 3])
            .unwrap();
        path
    }

    #[test]
    fn scan_labels_finds_well_formed_labels_only() {
        assert_eq!(scan_labels("see [Image 1] and [Image 23]."), vec![1, 23]);
        assert!(scan_labels("[Image ] [Image x] [image 1] [Image 1").is_empty());
    }

    #[test]
    fn staged_label_round_trips_to_an_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let label = stage_path(temp_png(&dir, "shot.png"));
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
    fn unknown_labels_resolve_to_nothing_and_missing_files_warn() {
        // Never handed out — the user just typed those words.
        let (attachments, warnings) = resolve_labels("look at [Image 987654]");
        assert!(attachments.is_empty());
        assert!(warnings.is_empty());

        // Handed out, but the file has since vanished.
        let dir = tempfile::tempdir().unwrap();
        let path = temp_png(&dir, "gone.png");
        let label = stage_path(&path);
        std::fs::remove_file(&path).unwrap();
        let (attachments, warnings) = resolve_labels(&label);
        assert!(attachments.is_empty());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn rewrite_paste_replaces_image_paths_and_leaves_plain_text_alone() {
        let dir = tempfile::tempdir().unwrap();
        let img = temp_png(&dir, "drop.png");

        let rewritten = rewrite_paste(&format!("describe {}\n", img.display())).unwrap();
        let labels = scan_labels(&rewritten);
        assert_eq!(labels.len(), 1);
        assert_eq!(rewritten, format!("describe [Image {}]", labels[0]));
        // The chip resolves back to the dropped file.
        let (attachments, _) = resolve_labels(&rewritten);
        assert_eq!(attachments[0].filename, "drop.png");

        // No image path → verbatim insert (None), even for other real files.
        assert_eq!(rewrite_paste("just some  spaced   text"), None);
        assert_eq!(rewrite_paste("/not/a/real/file.png"), None);
        let doc = dir.path().join("notes.md");
        std::fs::write(&doc, "hi").unwrap();
        assert_eq!(rewrite_paste(&doc.display().to_string()), None);
    }
}
