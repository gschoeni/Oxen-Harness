//! On-disk store of downloaded GGUF models.
//!
//! Models live in a single directory (`~/.oxen-harness/models/` by default),
//! each as `{id}.gguf` with a `{id}.json` sidecar holding its [`ModelRef`]
//! metadata. The sidecar lets arbitrary Hugging Face / Oxen models keep their
//! display name, quant, and origin across restarts — not just the curated few.

use std::path::{Path, PathBuf};

use crate::source::ModelRef;
use crate::LocalError;

/// Progress for an in-flight download.
#[derive(Debug, Clone, Copy)]
pub struct DownloadProgress {
    /// Bytes written so far.
    pub downloaded: u64,
    /// Total bytes, if the server reported a content length.
    pub total: Option<u64>,
}

impl DownloadProgress {
    /// Fraction complete in `0.0..=1.0`, if the total size is known.
    pub fn fraction(&self) -> Option<f64> {
        self.total
            .filter(|t| *t > 0)
            .map(|t| (self.downloaded as f64 / t as f64).clamp(0.0, 1.0))
    }
}

/// Manages the directory of downloaded models.
#[derive(Debug, Clone)]
pub struct ModelStore {
    dir: PathBuf,
}

impl ModelStore {
    /// Open the default store at `~/.oxen-harness/models/`, creating it.
    pub fn open() -> Result<Self, LocalError> {
        let dir = dirs::home_dir()
            .ok_or_else(|| LocalError::Download("could not determine home directory".to_string()))?
            .join(".oxen-harness")
            .join("models");
        Self::with_dir(dir)
    }

    /// Open a store rooted at `dir`, creating it if needed.
    pub fn with_dir(dir: impl Into<PathBuf>) -> Result<Self, LocalError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// The directory models are stored in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Local path a model's GGUF occupies.
    pub fn path_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.gguf"))
    }

    /// Local path a model's metadata sidecar occupies.
    fn meta_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Whether a model's GGUF is fully present.
    pub fn is_installed(&self, id: &str) -> bool {
        self.path_for(id).is_file()
    }

    /// Actual on-disk size of a model, if installed.
    pub fn installed_size(&self, id: &str) -> Option<u64> {
        std::fs::metadata(self.path_for(id))
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len())
    }

    /// Total bytes used by all `.gguf` files in the store directory.
    pub fn total_disk_used(&self) -> u64 {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let is_gguf = path.extension().is_some_and(|x| x == "gguf");
                is_gguf
                    .then(|| e.metadata().ok().map(|m| m.len()))
                    .flatten()
            })
            .sum()
    }

    /// Every installed model, read from its sidecar. A `.gguf` without a sidecar
    /// (e.g. placed manually) still appears as a minimal entry so it's usable.
    pub fn installed(&self) -> Vec<ModelRef> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "gguf") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            match self.read_meta(id) {
                Some(mut m) => {
                    m.size_bytes = size; // reflect the real on-disk size
                    out.push(m);
                }
                None => out.push(orphan_ref(id, size)),
            }
        }
        out.sort_by(|a, b| a.display.to_lowercase().cmp(&b.display.to_lowercase()));
        out
    }

    /// The metadata for one installed model, if a sidecar exists.
    pub fn read_meta(&self, id: &str) -> Option<ModelRef> {
        let raw = std::fs::read_to_string(self.meta_path(id)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// The model's native (training) context window in tokens, or 0 if unknown.
    ///
    /// Read straight from the GGUF header — authoritative regardless of how the
    /// model was added (a Hugging Face search leaves the sidecar's `context` at
    /// 0). Falls back to the sidecar value if the file can't be parsed.
    pub fn native_context(&self, id: &str) -> u32 {
        crate::gguf::context_length(&self.path_for(id))
            .or_else(|| self.read_meta(id).map(|m| m.context).filter(|&c| c > 0))
            .unwrap_or(0)
    }

    /// Remove a downloaded model (GGUF + sidecar), returning `true` if it existed.
    pub fn remove(&self, id: &str) -> Result<bool, LocalError> {
        let gguf = self.path_for(id);
        let existed = gguf.is_file();
        if existed {
            std::fs::remove_file(&gguf)?;
        }
        let _ = std::fs::remove_file(self.meta_path(id));
        Ok(existed)
    }

    /// Download a model's GGUF, writing its sidecar on success. `token` is an
    /// optional bearer token (Hugging Face / Oxen) for gated or private repos.
    /// Streams to a `.part` file and atomically renames, so an interrupted
    /// download never looks installed. A no-op if already present.
    pub async fn download<F>(
        &self,
        model: &ModelRef,
        token: Option<&str>,
        on_progress: F,
    ) -> Result<PathBuf, LocalError>
    where
        F: FnMut(DownloadProgress),
    {
        let final_path = self.path_for(&model.id);
        if final_path.is_file() {
            // Ensure the sidecar exists even for a previously-downloaded file.
            self.write_meta(model)?;
            return Ok(final_path);
        }
        let client = reqwest::Client::new();
        stream_to_file(
            &client,
            &model.download_url(),
            token,
            &final_path,
            on_progress,
        )
        .await?;
        self.write_meta(model)?;
        Ok(final_path)
    }

    fn write_meta(&self, model: &ModelRef) -> Result<(), LocalError> {
        let json = serde_json::to_string_pretty(model)
            .map_err(|e| LocalError::Download(format!("could not serialize metadata: {e}")))?;
        std::fs::write(self.meta_path(&model.id), json)?;
        Ok(())
    }
}

/// Total and available bytes on the filesystem holding `path`, so the UI can
/// show free space and warn before a download won't fit. `None` if it can't be
/// determined (or on platforms without `statvfs`).
#[cfg(unix)]
#[allow(unsafe_code)] // the one audited FFI call in the workspace; see SAFETY below
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `cpath` is a valid NUL-terminated path; `statvfs` fully initializes
    // `stat` on success (return 0) and we read it only then.
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(cpath.as_ptr(), &mut stat) != 0 {
            return None;
        }
        let unit = if stat.f_frsize > 0 {
            stat.f_frsize
        } else {
            stat.f_bsize
        } as u64;
        let total = (stat.f_blocks as u64).saturating_mul(unit);
        let available = (stat.f_bavail as u64).saturating_mul(unit);
        Some((total, available))
    }
}

#[cfg(not(unix))]
pub fn disk_space(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// A minimal [`ModelRef`] for a GGUF found on disk without a sidecar.
fn orphan_ref(id: &str, size: u64) -> ModelRef {
    ModelRef {
        id: id.to_string(),
        display: id.to_string(),
        params: String::new(),
        quant: crate::source::parse_quant(id).unwrap_or_default(),
        context: 0,
        size_bytes: size,
        origin: crate::source::Origin::HuggingFace {
            repo: String::new(),
            file: format!("{id}.gguf"),
            revision: "main".to_string(),
        },
    }
}

/// Stream `url` to `dest` (via a sibling `.part` file, atomically renamed on
/// success), optionally with a bearer `token`, reporting progress.
async fn stream_to_file<F>(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    dest: &Path,
    mut on_progress: F,
) -> Result<(), LocalError>
where
    F: FnMut(DownloadProgress),
{
    let part_path = dest.with_extension("gguf.part");
    crate::download::fetch_to_file(
        client,
        url,
        &part_path,
        crate::download::FetchOpts {
            token,
            user_agent: Some("oxen-harness"),
            gated_message: Some("access denied — this model may be gated or private; add a token"),
        },
        |downloaded, total| on_progress(DownloadProgress { downloaded, total }),
    )
    .await?;
    tokio::fs::rename(&part_path, dest).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{ModelRef, Origin};

    fn store() -> (tempfile::TempDir, ModelStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::with_dir(dir.path().join("models")).unwrap();
        (dir, store)
    }

    fn sample_ref(id: &str) -> ModelRef {
        ModelRef {
            id: id.to_string(),
            display: "Sample".to_string(),
            params: "8B".to_string(),
            quant: "Q4_K_M".to_string(),
            context: 40960,
            size_bytes: 123,
            origin: Origin::HuggingFace {
                repo: "owner/name".to_string(),
                file: "model-Q4_K_M.gguf".to_string(),
                revision: "main".to_string(),
            },
        }
    }

    #[test]
    fn fresh_store_is_empty() {
        let (_d, store) = store();
        assert!(!store.is_installed("x"));
        assert_eq!(store.total_disk_used(), 0);
        assert!(store.installed().is_empty());
    }

    #[test]
    fn installed_reads_sidecar_and_reflects_disk_size() {
        let (_d, store) = store();
        let m = sample_ref("sample-q4-k-m");
        std::fs::write(store.path_for(&m.id), b"0123456789").unwrap();
        store.write_meta(&m).unwrap();

        let installed = store.installed();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].display, "Sample");
        assert_eq!(installed[0].quant, "Q4_K_M");
        assert_eq!(installed[0].size_bytes, 10); // real on-disk size, not the 123
        assert_eq!(store.total_disk_used(), 10);
    }

    #[test]
    fn orphan_gguf_without_sidecar_still_lists() {
        let (_d, store) = store();
        std::fs::write(store.path_for("loose-Q5_K_M"), b"x").unwrap();
        let installed = store.installed();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].id, "loose-Q5_K_M");
        assert_eq!(installed[0].quant, "Q5_K_M");
    }

    #[test]
    fn remove_deletes_gguf_and_sidecar() {
        let (_d, store) = store();
        let m = sample_ref("gone");
        std::fs::write(store.path_for(&m.id), b"x").unwrap();
        store.write_meta(&m).unwrap();
        assert!(store.remove(&m.id).unwrap());
        assert!(!store.is_installed(&m.id));
        assert!(!store.meta_path(&m.id).is_file());
        assert!(!store.remove(&m.id).unwrap());
    }

    #[tokio::test]
    async fn download_streams_and_writes_sidecar() {
        let mut server = mockito::Server::new_async().await;
        let body = vec![7u8; 2048];
        let mock = server
            .mock("GET", "/m.gguf")
            .with_status(200)
            .with_header("content-length", &body.len().to_string())
            .with_body(body)
            .create_async()
            .await;

        let (_d, store) = store();
        let m = ModelRef {
            id: "dl".to_string(),
            display: "DL".to_string(),
            params: String::new(),
            quant: "Q4_K_M".to_string(),
            context: 0,
            size_bytes: 0,
            origin: Origin::HuggingFace {
                repo: "o/n".to_string(),
                // Point the download at the mock server via a full URL override
                // isn't possible through ModelRef; instead we hit stream_to_file.
                file: "m.gguf".to_string(),
                revision: "main".to_string(),
            },
        };
        // Exercise the streaming helper directly against the mock.
        let dest = store.path_for(&m.id);
        super::stream_to_file(
            &reqwest::Client::new(),
            &format!("{}/m.gguf", server.url()),
            None,
            &dest,
            |_| {},
        )
        .await
        .unwrap();
        store.write_meta(&m).unwrap();

        assert!(store.is_installed("dl"));
        assert_eq!(store.installed_size("dl"), Some(2048));
        assert_eq!(store.read_meta("dl").unwrap().quant, "Q4_K_M");
        assert!(!dest.with_extension("gguf.part").exists());
        mock.assert_async().await;
    }
}
