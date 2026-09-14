//! The project's media library: generated files under `<root>/generations/`
//! (configurable) plus an append-only `manifest.jsonl` beside them.
//!
//! Paid, non-deterministic output is user state — it lives in the project,
//! visible in the file tree, versioned with the code, and survives a clone.
//! The manifest is JSONL like the harness's other logs: one row per state
//! change, folded by id on read (last row wins), so a queued job and its
//! finished file are the same item. Readable filenames
//! (`HHMM-<slug>-<n>.<ext>` under a date folder) keep directory order equal
//! to creation order and make the folder browsable without the harness.
//!
//! In-flight jobs live in memory too, and every change bumps a watch
//! channel hosts forward as a whole-list event.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;
use tokio::sync::watch;

use crate::MediaKind;

/// Where a generation is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStatus {
    Queued,
    Processing,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl MediaStatus {
    pub fn from_hub(status: &str) -> Self {
        match status {
            "queued" => Self::Queued,
            "processing" => Self::Processing,
            "succeeded" => Self::Succeeded,
            "cancelled" => Self::Cancelled,
            _ => Self::Failed,
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Processing)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Processing => "processing",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

/// One generation: a manifest row and a feed item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaItem {
    /// The hub's generation id (or a local id when enqueueing failed).
    pub id: String,
    /// The chat that asked for it.
    pub session: String,
    /// Groups the outputs of one tool call (a 2×2 contact sheet).
    #[serde(default)]
    pub batch: String,
    /// 1-based position within the batch.
    #[serde(default = "one")]
    pub index: u32,
    pub kind: MediaKind,
    pub model: String,
    pub prompt: String,
    /// The request parameters sent (minus prompt and reference data).
    #[serde(default)]
    pub params: Value,
    /// Project-relative paths of the reference copies used.
    #[serde(default)]
    pub refs: Vec<String>,
    /// Project-relative path of the saved output, once there is one.
    #[serde(default)]
    pub path: Option<String>,
    /// Project-relative poster frame for a video, when one was extracted.
    #[serde(default)]
    pub poster: Option<String>,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// Requested duration in seconds, for videos.
    #[serde(default)]
    pub duration_secs: Option<f64>,
    /// The catalog's estimate for this one output.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    pub status: MediaStatus,
    #[serde(default)]
    pub error: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    #[serde(default)]
    pub completed_at: Option<i64>,
    /// The item this one varies, upscales, or animates, when the agent said.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub seed: Option<Value>,
}

fn one() -> u32 {
    1
}

/// Where an upload is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    Uploading,
    Presigning,
    Done,
    /// The bytes were already on the hub; only a presign happened.
    Reused,
    Failed,
}

impl UploadStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uploading => "uploading",
            Self::Presigning => "presigning",
            Self::Done => "done",
            Self::Reused => "reused",
            Self::Failed => "failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Reused | Self::Failed)
    }
}

/// One reference on its way to the hub — a feed row the hosts draw as a
/// progress bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaUpload {
    pub id: String,
    pub session: String,
    /// The chip label it came from (`[Image #1]`), when it did.
    pub label: Option<String>,
    pub filename: String,
    /// `image`, `video`, `audio`.
    pub kind: String,
    pub bytes_sent: u64,
    pub bytes_total: u64,
    pub status: UploadStatus,
    pub error: Option<String>,
    /// Unix seconds.
    pub started_at: i64,
}

impl MediaItem {
    /// `generations/2026-09-13/1402-ox-1.png` → `1402-ox-1.png`.
    pub fn file_name(&self) -> Option<&str> {
        self.path.as_deref().and_then(|p| p.rsplit('/').next())
    }
}

/// One change notification: a sequence number and the session whose item
/// changed, so a host can tag the whole-list event it forwards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaTick {
    pub seq: u64,
    pub session: String,
}

/// The library for one project root.
pub struct MediaLibrary {
    root: PathBuf,
    /// Project-relative output folder (`generations`).
    dir: String,
    /// Items touched this process (in flight and just finished), newest last.
    live: Mutex<Vec<MediaItem>>,
    /// References being uploaded for a generation, in start order.
    uploads: Mutex<Vec<MediaUpload>>,
    changed: watch::Sender<MediaTick>,
}

impl MediaLibrary {
    pub fn new(root: impl Into<PathBuf>, dir: impl Into<String>) -> Self {
        let (changed, _) = watch::channel(MediaTick::default());
        Self {
            root: root.into(),
            dir: dir.into(),
            live: Mutex::new(Vec::new()),
            uploads: Mutex::new(Vec::new()),
            changed,
        }
    }

    /// Put an upload's current state in the feed (in-memory only).
    pub fn set_upload(&self, upload: MediaUpload) {
        let session = upload.session.clone();
        {
            let mut uploads = self.uploads.lock().expect("media uploads poisoned");
            match uploads.iter_mut().find(|u| u.id == upload.id) {
                Some(existing) => *existing = upload,
                None => uploads.push(upload),
            }
        }
        self.notify(&session);
    }

    /// Every upload in the feed, oldest first.
    pub fn uploads(&self) -> Vec<MediaUpload> {
        self.uploads.lock().expect("media uploads poisoned").clone()
    }

    /// Uploads still moving, optionally for one session.
    pub fn uploads_in_flight(&self, session: Option<&str>) -> Vec<MediaUpload> {
        self.uploads()
            .into_iter()
            .filter(|u| !u.status.is_terminal())
            .filter(|u| session.is_none_or(|s| u.session == s))
            .collect()
    }

    /// Drop a session's finished uploads from the feed (its generation is
    /// recorded; the bars have done their job). Failed ones stay visible
    /// until the next batch starts.
    pub fn clear_uploads(&self, session: &str, keep_failed: bool) {
        {
            let mut uploads = self.uploads.lock().expect("media uploads poisoned");
            uploads.retain(|u| {
                u.session != session || (keep_failed && u.status == UploadStatus::Failed)
            });
        }
        self.notify(session);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The project-relative output folder.
    pub fn dir_rel(&self) -> &str {
        &self.dir
    }

    pub fn dir_abs(&self) -> PathBuf {
        self.root.join(&self.dir)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.dir_abs().join("manifest.jsonl")
    }

    /// Bumped on every change; hosts forward the new whole list.
    pub fn changes(&self) -> watch::Receiver<MediaTick> {
        self.changed.subscribe()
    }

    fn notify(&self, session: &str) {
        self.changed.send_modify(|t| {
            t.seq += 1;
            t.session = session.to_string();
        });
    }

    /// Put an item's current state in the live set (in-memory only).
    pub fn upsert(&self, item: MediaItem) {
        let session = item.session.clone();
        {
            let mut live = self.live.lock().expect("media live poisoned");
            match live.iter_mut().find(|i| i.id == item.id) {
                Some(existing) => *existing = item,
                None => live.push(item),
            }
        }
        self.notify(&session);
    }

    /// Append an item's state to the manifest and the live set.
    pub fn record(&self, item: MediaItem) -> std::io::Result<()> {
        std::fs::create_dir_all(self.dir_abs())?;
        let mut line = serde_json::to_string(&item).map_err(std::io::Error::other)?;
        line.push('\n');
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.manifest_path())?;
        f.write_all(line.as_bytes())?;
        self.upsert(item);
        Ok(())
    }

    /// Every item the manifest knows plus what's live, folded by id (the
    /// newest state wins), newest first.
    pub fn items(&self) -> Vec<MediaItem> {
        let mut folded: Vec<MediaItem> = Vec::new();
        let mut fold = |item: MediaItem| match folded.iter_mut().find(|i| i.id == item.id) {
            Some(existing) => *existing = item,
            None => folded.push(item),
        };
        if let Ok(text) = std::fs::read_to_string(self.manifest_path()) {
            for line in text.lines() {
                if let Ok(item) = serde_json::from_str::<MediaItem>(line) {
                    fold(item);
                }
            }
        }
        for item in self.live.lock().expect("media live poisoned").iter() {
            fold(item.clone());
        }
        folded.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.batch.cmp(&a.batch))
                .then_with(|| a.index.cmp(&b.index))
        });
        folded
    }

    /// Items still queued or processing, for status and cancel.
    pub fn in_flight(&self, session: Option<&str>) -> Vec<MediaItem> {
        self.live
            .lock()
            .expect("media live poisoned")
            .iter()
            .filter(|i| !i.status.is_terminal())
            .filter(|i| session.is_none_or(|s| i.session == s))
            .cloned()
            .collect()
    }

    /// Recent items of a session (any status), newest first.
    pub fn recent(&self, session: &str, limit: usize) -> Vec<MediaItem> {
        self.items()
            .into_iter()
            .filter(|i| i.session == session)
            .take(limit)
            .collect()
    }

    /// Copy a reference file into `refs/` (content-addressed, so reusing a
    /// reference costs nothing) and return its project-relative path.
    pub fn store_ref(&self, source: &Path) -> std::io::Result<String> {
        let bytes = std::fs::read(source)?;
        let ext = source
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_else(|| "bin".into());
        let hash = hex(&sha2::Sha256::digest(&bytes));
        let rel = format!("{}/refs/{}.{ext}", self.dir, &hash[..16]);
        let abs = self.root.join(&rel);
        if !abs.exists() {
            let parent = abs.parent().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("reference path has no parent: {}", abs.display()),
                )
            })?;
            std::fs::create_dir_all(parent)?;
            std::fs::write(&abs, &bytes)?;
        }
        Ok(rel)
    }

    /// Write an output as `<dir>/<YYYY-MM-DD>/<HHMM>-<slug>-<index>.<ext>`,
    /// bumping a suffix if the name is taken. Returns the relative path.
    pub fn save_output(
        &self,
        bytes: &[u8],
        slug: &str,
        index: u32,
        ext: &str,
        created_at: i64,
    ) -> std::io::Result<String> {
        let (date, hhmm) = date_parts(created_at);
        let folder = self.dir_abs().join(&date);
        std::fs::create_dir_all(&folder)?;
        let base = format!("{hhmm}-{slug}-{index}");
        let mut name = format!("{base}.{ext}");
        let mut attempt = 1;
        while folder.join(&name).exists() {
            attempt += 1;
            name = format!("{base}-{attempt}.{ext}");
        }
        std::fs::write(folder.join(&name), bytes)?;
        Ok(format!("{}/{date}/{name}", self.dir))
    }

    /// Absolute path of a project-relative item path.
    pub fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A filename-safe slug from a prompt: lowercase words joined by `-`,
/// capped at 40 chars, never empty.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in text.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "generation".to_string()
    } else {
        trimmed
    }
}

/// `(YYYY-MM-DD, HHMM)` in local time for a unix timestamp — a civil-date
/// conversion without a chrono dependency (local offset from the C runtime
/// isn't portable, so this uses UTC; the folder is a grouping, not a clock).
pub fn date_parts(unix: i64) -> (String, String) {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (
        format!("{y:04}-{m:02}-{d:02}"),
        format!("{:02}{:02}", secs / 3600, (secs % 3600) / 60),
    )
}

/// Howard Hinnant's days-from-civil inverse.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Now, in unix seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, status: MediaStatus, created_at: i64) -> MediaItem {
        MediaItem {
            id: id.into(),
            session: "s1".into(),
            batch: "b1".into(),
            index: 1,
            kind: MediaKind::Image,
            model: "m".into(),
            prompt: "an ox".into(),
            params: Value::Null,
            refs: vec![],
            path: None,
            poster: None,
            bytes: 0,
            width: None,
            height: None,
            duration_secs: None,
            cost_usd: Some(0.01),
            status,
            error: None,
            created_at,
            completed_at: None,
            parent: None,
            seed: None,
        }
    }

    #[test]
    fn manifest_folds_by_id_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let lib = MediaLibrary::new(dir.path(), "generations");
        lib.record(item("a", MediaStatus::Queued, 100)).unwrap();
        lib.record(item("b", MediaStatus::Queued, 200)).unwrap();
        let mut done = item("a", MediaStatus::Succeeded, 100);
        done.path = Some("generations/x.png".into());
        lib.record(done).unwrap();

        let items = lib.items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "b");
        assert_eq!(items[1].id, "a");
        assert_eq!(items[1].status, MediaStatus::Succeeded);
        assert_eq!(items[1].path.as_deref(), Some("generations/x.png"));

        // A fresh library over the same folder reads the manifest back.
        let again = MediaLibrary::new(dir.path(), "generations");
        assert_eq!(again.items().len(), 2);
        assert_eq!(lib.in_flight(Some("s1")).len(), 1);
    }

    #[test]
    fn changes_bump_on_record_and_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let lib = MediaLibrary::new(dir.path(), "generations");
        let rx = lib.changes();
        assert_eq!(rx.borrow().seq, 0);
        lib.upsert(item("a", MediaStatus::Queued, 1));
        assert_eq!(rx.borrow().seq, 1);
        assert_eq!(rx.borrow().session, "s1");
        lib.record(item("a", MediaStatus::Succeeded, 1)).unwrap();
        assert_eq!(rx.borrow().seq, 2);
    }

    #[test]
    fn uploads_ride_the_feed_and_clear_when_done() {
        let dir = tempfile::tempdir().unwrap();
        let lib = MediaLibrary::new(dir.path(), "generations");
        let rx = lib.changes();
        let up = MediaUpload {
            id: "u1".into(),
            session: "s1".into(),
            label: Some("[Image #1]".into()),
            filename: "photo.png".into(),
            kind: "image".into(),
            bytes_sent: 0,
            bytes_total: 100,
            status: UploadStatus::Uploading,
            error: None,
            started_at: 1,
        };
        lib.set_upload(up.clone());
        assert_eq!(rx.borrow().seq, 1);
        assert_eq!(lib.uploads_in_flight(Some("s1")).len(), 1);
        lib.set_upload(MediaUpload {
            bytes_sent: 100,
            status: UploadStatus::Done,
            ..up.clone()
        });
        assert_eq!(lib.uploads().len(), 1);
        assert!(lib.uploads_in_flight(None).is_empty());
        lib.set_upload(MediaUpload {
            id: "u2".into(),
            status: UploadStatus::Failed,
            error: Some("nope".into()),
            ..up
        });
        lib.clear_uploads("s1", true);
        let left = lib.uploads();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "u2");
        lib.clear_uploads("s1", false);
        assert!(lib.uploads().is_empty());
    }

    #[test]
    fn outputs_get_readable_unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let lib = MediaLibrary::new(dir.path(), "generations");
        let ts = 1_789_000_000; // 2026-09-10 00:26:40 UTC
        let a = lib.save_output(b"1", "an-ox", 1, "png", ts).unwrap();
        let b = lib.save_output(b"2", "an-ox", 1, "png", ts).unwrap();
        assert_eq!(a, "generations/2026-09-10/0026-an-ox-1.png");
        assert_eq!(b, "generations/2026-09-10/0026-an-ox-1-2.png");
        assert!(dir.path().join(&b).exists());
    }

    #[test]
    fn refs_are_content_addressed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("photo.JPG");
        std::fs::write(&src, b"jpeg").unwrap();
        let lib = MediaLibrary::new(dir.path(), "generations");
        let a = lib.store_ref(&src).unwrap();
        let b = lib.store_ref(&src).unwrap();
        assert_eq!(a, b);
        assert!(
            a.starts_with("generations/refs/") && a.ends_with(".jpg"),
            "{a}"
        );
    }

    #[test]
    fn slugs_and_dates() {
        assert_eq!(
            slugify("Two racers, one on a DINOSAUR!"),
            "two-racers-one-on-a-dinosaur"
        );
        assert_eq!(slugify("   "), "generation");
        assert!(slugify(&"word ".repeat(30)).len() <= 40);
        assert_eq!(
            date_parts(0),
            ("1970-01-01".to_string(), "0000".to_string())
        );
        assert_eq!(date_parts(1_789_000_000).0, "2026-09-10");
    }
}
