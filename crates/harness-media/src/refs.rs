//! Reference media: how the user's dropped files reach a generation request.
//!
//! Both hosts stage a turn's attachments into the session's [`MediaRefs`]
//! registry under chip labels — `[Image #1]`, `[Video #1]`, `[Audio #1]` —
//! and put the same labels in the prompt text the model sees, so "make
//! `[Image #2]` the first frame" resolves to a real file here. A generation
//! tool's `refs` accepts those labels, a workspace-relative path (an earlier
//! generation), or an absolute path the user attached.
//!
//! The hub wants URLs its workers can fetch: every resolved reference is
//! uploaded to the hub and presigned (see [`crate::upload`]) before it goes
//! into a request.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_tools::ToolError;

/// What a reference file is, by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    Image,
    Video,
    Audio,
}

impl RefKind {
    /// The chip word (`[Image #N]`).
    pub fn word(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
        }
    }

    /// The largest file the hub documents for this kind (seedance's limits:
    /// 30 MB images, 50 MB video, 15 MB audio).
    pub fn max_bytes(self) -> u64 {
        match self {
            Self::Image => 30 * 1024 * 1024,
            Self::Video => 50 * 1024 * 1024,
            Self::Audio => 15 * 1024 * 1024,
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "heic" | "avif" => {
                Some(Self::Image)
            }
            "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v" => Some(Self::Video),
            "mp3" | "wav" | "m4a" | "aac" | "ogg" | "oga" | "flac" | "opus" => Some(Self::Audio),
            _ => None,
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Self::from_extension)
    }
}

/// The MIME type for a `data:` URI, by extension.
pub fn mime_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        "heic" => "image/heic",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
}

/// A media file staged behind a chip label.
#[derive(Debug, Clone)]
struct Staged {
    label: String,
    path: PathBuf,
    kind: RefKind,
}

/// The session's staged reference media, looked up by chip label. Shared by
/// the host (which stages attachments as turns arrive) and the tools (which
/// resolve labels the model passes back).
#[derive(Debug, Default)]
pub struct MediaRefs {
    staged: Mutex<Vec<Staged>>,
}

impl MediaRefs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stage a file and hand back its chip label, numbering up per kind
    /// (`[Image #1]`, `[Image #2]`, `[Video #1]`). A path staged before
    /// keeps its original label, so re-attaching a file doesn't renumber.
    /// Returns `None` for a file that isn't image/video/audio.
    pub fn stage(&self, path: impl Into<PathBuf>) -> Option<String> {
        let path = path.into();
        let kind = RefKind::from_path(&path)?;
        let mut staged = self.staged.lock().expect("media refs poisoned");
        if let Some(existing) = staged.iter().find(|s| s.path == path) {
            return Some(existing.label.clone());
        }
        let n = staged.iter().filter(|s| s.kind == kind).count() + 1;
        let label = format!("[{} #{n}]", kind.word());
        staged.push(Staged {
            label: label.clone(),
            path,
            kind,
        });
        Some(label)
    }

    /// Stage a file under a label the host already handed out (the CLI
    /// numbers its own chips), so both registries agree.
    pub fn stage_as(&self, label: &str, path: impl Into<PathBuf>) {
        let path = path.into();
        let Some(kind) = RefKind::from_path(&path) else {
            return;
        };
        let mut staged = self.staged.lock().expect("media refs poisoned");
        if staged.iter().any(|s| s.label == label) {
            return;
        }
        staged.push(Staged {
            label: label.to_string(),
            path,
            kind,
        });
    }

    /// The file behind a chip label, if that label was handed out.
    pub fn lookup(&self, label: &str) -> Option<PathBuf> {
        let wanted = normalize_label(label)?;
        self.staged
            .lock()
            .expect("media refs poisoned")
            .iter()
            .find(|s| s.label == wanted)
            .map(|s| s.path.clone())
    }

    /// Every staged label with its file, in staging order.
    pub fn all(&self) -> Vec<(String, PathBuf)> {
        self.staged
            .lock()
            .expect("media refs poisoned")
            .iter()
            .map(|s| (s.label.clone(), s.path.clone()))
            .collect()
    }

    /// Whether `path` was staged (so an absolute path outside the workspace
    /// the user themselves attached is still an acceptable reference).
    pub fn contains_path(&self, path: &Path) -> bool {
        self.staged
            .lock()
            .expect("media refs poisoned")
            .iter()
            .any(|s| s.path == path)
    }

    /// Every chip label mentioned in `text` that this registry knows.
    pub fn labels_in(&self, text: &str) -> Vec<String> {
        let staged = self.staged.lock().expect("media refs poisoned");
        let mut found = Vec::new();
        for s in staged.iter() {
            if text.contains(&s.label) && !found.contains(&s.label) {
                found.push(s.label.clone());
            }
        }
        found
    }
}

/// Accept `[Image #2]`, `Image #2`, `[image 2]`, `image2` as the same chip.
fn normalize_label(raw: &str) -> Option<String> {
    let inner = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    let lower = inner.to_ascii_lowercase();
    let word = ["image", "video", "audio", "pdf"]
        .into_iter()
        .find(|w| lower.starts_with(w))?;
    let digits: String = lower[word.len()..]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    let mut w = word.to_string();
    if let Some(first) = w.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    if w == "Pdf" {
        w = "PDF".into();
    }
    Some(format!("[{w} #{digits}]"))
}

/// Whether `text` looks like a chip label rather than a path.
pub fn is_label(text: &str) -> bool {
    normalize_label(text).is_some() && !text.contains('/') && !text.contains('.')
}

/// A reference resolved to a real file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRef {
    /// The chip label it came from, when it did.
    pub label: Option<String>,
    pub path: PathBuf,
    pub kind: RefKind,
    pub filename: String,
}

/// Resolve the model's `refs` into files: chip labels through the registry;
/// paths relative to the workspace, or absolute when inside the workspace or
/// staged by the user. Anything else is refused, so the model can't feed an
/// arbitrary local file to the hub.
pub fn resolve(
    refs: &[String],
    registry: &MediaRefs,
    workspace_root: &Path,
) -> Result<Vec<ResolvedRef>, ToolError> {
    let mut out: Vec<ResolvedRef> = Vec::new();
    for raw in refs {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let (label, path) = if is_label(raw) {
            let path = registry.lookup(raw).ok_or_else(|| {
                let known = registry
                    .all()
                    .into_iter()
                    .map(|(l, p)| format!("{l} ({})", file_name(&p)))
                    .collect::<Vec<_>>();
                ToolError::InvalidArguments(if known.is_empty() {
                    format!(
                        "{raw} isn't an attachment the user has provided. Ask them to drop the \
                         file into the chat, then use its label."
                    )
                } else {
                    format!(
                        "{raw} isn't an attachment in this chat. Available: {}",
                        known.join(", ")
                    )
                })
            })?;
            (normalize_label(raw), path)
        } else {
            let candidate = PathBuf::from(raw);
            let path = if candidate.is_absolute() {
                candidate
            } else {
                workspace_root.join(&candidate)
            };
            let canonical = path
                .canonicalize()
                .map_err(|_| ToolError::InvalidArguments(format!("{raw} does not exist")))?;
            let root = workspace_root
                .canonicalize()
                .unwrap_or_else(|_| workspace_root.to_path_buf());
            if !canonical.starts_with(&root) && !registry.contains_path(&path) {
                return Err(ToolError::InvalidArguments(format!(
                    "{raw} is outside the project; only project files and the user's \
                     attachments can be references"
                )));
            }
            (None, canonical)
        };
        let kind = RefKind::from_path(&path).ok_or_else(|| {
            ToolError::InvalidArguments(format!("{raw} isn't an image, video, or audio file"))
        })?;
        if out.iter().any(|r| r.path == path) {
            continue;
        }
        out.push(ResolvedRef {
            label,
            filename: file_name(&path),
            path,
            kind,
        });
    }
    Ok(out)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_number_up_per_kind_and_stay_stable() {
        let refs = MediaRefs::new();
        assert_eq!(refs.stage("/tmp/a.png").as_deref(), Some("[Image #1]"));
        assert_eq!(refs.stage("/tmp/b.mp4").as_deref(), Some("[Video #1]"));
        assert_eq!(refs.stage("/tmp/c.jpg").as_deref(), Some("[Image #2]"));
        assert_eq!(refs.stage("/tmp/a.png").as_deref(), Some("[Image #1]"));
        assert_eq!(refs.stage("/tmp/d.wav").as_deref(), Some("[Audio #1]"));
        assert_eq!(refs.stage("/tmp/notes.txt"), None);
        assert_eq!(refs.lookup("[Image #2]"), Some(PathBuf::from("/tmp/c.jpg")));
        assert_eq!(refs.lookup("image 2"), Some(PathBuf::from("/tmp/c.jpg")));
        assert_eq!(refs.lookup("[Image #9]"), None);
    }

    #[test]
    fn host_labels_are_honored() {
        let refs = MediaRefs::new();
        refs.stage_as("[Image #7]", "/tmp/seven.png");
        assert_eq!(
            refs.lookup("[Image #7]"),
            Some(PathBuf::from("/tmp/seven.png"))
        );
        assert_eq!(
            refs.labels_in("use [Image #7] and [Image #8]"),
            vec!["[Image #7]"]
        );
    }

    #[test]
    fn label_detection() {
        assert!(is_label("[Image #1]"));
        assert!(is_label("Video #2"));
        assert!(!is_label("generations/2026/a.png"));
        assert!(!is_label("image.png"));
    }

    #[test]
    fn resolves_paths_inside_the_workspace_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("generations")).unwrap();
        std::fs::write(root.join("generations/a.png"), b"x").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("o.png"), b"x").unwrap();
        let refs = MediaRefs::new();

        let ok = resolve(&["generations/a.png".into()], &refs, root).unwrap();
        assert_eq!(ok[0].kind, RefKind::Image);
        assert_eq!(ok[0].filename, "a.png");

        let err = resolve(
            &[outside.path().join("o.png").to_string_lossy().into_owned()],
            &refs,
            root,
        )
        .unwrap_err();
        assert!(err.to_string().contains("outside the project"), "{err}");

        // The same outside file is fine once the user attached it.
        refs.stage(outside.path().join("o.png"));
        let ok = resolve(&["[Image #1]".into()], &refs, root).unwrap();
        assert_eq!(ok[0].label.as_deref(), Some("[Image #1]"));

        let err = resolve(&["[Video #3]".into()], &refs, root).unwrap_err();
        assert!(err.to_string().contains("Available: [Image #1]"), "{err}");
    }
}
