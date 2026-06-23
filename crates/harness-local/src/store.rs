//! On-disk store of downloaded GGUF models.
//!
//! Models live in a single directory (`~/.oxen-harness/models/` by default).
//! The store knows which catalog entries are present, how much disk each uses,
//! and how to download (with progress) or remove them.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::Serialize;

use crate::catalog::{self, ModelSpec};
use crate::{download_url, LocalError};

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

/// A catalog model paired with its local on-disk status (for listing/UIs).
#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub display: String,
    pub params: String,
    pub quant: String,
    pub context: u32,
    pub note: String,
    /// Whether the GGUF is present locally.
    pub installed: bool,
    /// Actual on-disk size when installed, else the catalog estimate.
    pub size_bytes: u64,
    /// True when `size_bytes` is the actual on-disk size (vs an estimate).
    pub size_is_actual: bool,
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

    /// Local path a model's GGUF would occupy.
    pub fn path_for(&self, spec: &ModelSpec) -> PathBuf {
        self.dir.join(spec.file)
    }

    /// Whether a model's GGUF is fully present (no leftover `.part`).
    pub fn is_installed(&self, spec: &ModelSpec) -> bool {
        self.path_for(spec).is_file()
    }

    /// Actual on-disk size of a model, if installed.
    pub fn installed_size(&self, spec: &ModelSpec) -> Option<u64> {
        std::fs::metadata(self.path_for(spec))
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

    /// Catalog entries decorated with their local status (for `models list`).
    pub fn statuses(&self) -> Vec<ModelStatus> {
        catalog::catalog()
            .iter()
            .map(|spec| {
                let installed_size = self.installed_size(spec);
                ModelStatus {
                    id: spec.id.to_string(),
                    display: spec.display.to_string(),
                    params: spec.params.to_string(),
                    quant: spec.quant.to_string(),
                    context: spec.context,
                    note: spec.note.to_string(),
                    installed: installed_size.is_some(),
                    size_bytes: installed_size.unwrap_or(spec.approx_bytes),
                    size_is_actual: installed_size.is_some(),
                }
            })
            .collect()
    }

    /// Remove a downloaded model, returning `true` if a file was deleted.
    pub fn remove(&self, spec: &ModelSpec) -> Result<bool, LocalError> {
        let path = self.path_for(spec);
        if path.is_file() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Download a model using a fresh HTTP client (convenience over
    /// [`ModelStore::download`] for callers that don't manage their own).
    pub async fn pull<F>(&self, spec: &ModelSpec, on_progress: F) -> Result<PathBuf, LocalError>
    where
        F: FnMut(DownloadProgress),
    {
        let client = reqwest::Client::new();
        self.download(spec, &client, on_progress).await
    }

    /// Download a model's GGUF, invoking `on_progress` as bytes arrive.
    ///
    /// The download streams to a `.part` file and is atomically renamed into
    /// place on success, so an interrupted download never looks "installed".
    /// Returns the final path (a no-op returning early if already present).
    pub async fn download<F>(
        &self,
        spec: &ModelSpec,
        client: &reqwest::Client,
        on_progress: F,
    ) -> Result<PathBuf, LocalError>
    where
        F: FnMut(DownloadProgress),
    {
        let final_path = self.path_for(spec);
        if final_path.is_file() {
            return Ok(final_path);
        }
        stream_to_file(client, &download_url(spec), &final_path, on_progress).await?;
        Ok(final_path)
    }
}

/// Stream `url` to `dest`, reporting progress, via a sibling `.part` file that
/// is atomically renamed into place only on success.
async fn stream_to_file<F>(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    mut on_progress: F,
) -> Result<(), LocalError>
where
    F: FnMut(DownloadProgress),
{
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| LocalError::Download(format!("request failed: {e}")))?;
    if !response.status().is_success() {
        return Err(LocalError::Download(format!(
            "HTTP {} fetching {}",
            response.status().as_u16(),
            url
        )));
    }
    let total = response.content_length();

    let part_path = dest.with_extension("gguf.part");
    let mut file = tokio::fs::File::create(&part_path).await?;
    let mut downloaded: u64 = 0;
    on_progress(DownloadProgress { downloaded, total });

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LocalError::Download(format!("stream error: {e}")))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        downloaded += chunk.len() as u64;
        on_progress(DownloadProgress { downloaded, total });
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);

    tokio::fs::rename(&part_path, dest).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ModelStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ModelStore::with_dir(dir.path().join("models")).unwrap();
        (dir, store)
    }

    #[test]
    fn fresh_store_has_nothing_installed() {
        let (_dir, store) = store();
        let spec = catalog::find("qwen3-0.6b").unwrap();
        assert!(!store.is_installed(spec));
        assert_eq!(store.installed_size(spec), None);
        assert_eq!(store.total_disk_used(), 0);

        let status = store
            .statuses()
            .iter()
            .find(|s| s.id == spec.id)
            .cloned()
            .unwrap();
        assert!(!status.installed);
        assert!(!status.size_is_actual);
        assert_eq!(status.size_bytes, spec.approx_bytes);
    }

    #[test]
    fn reports_disk_usage_and_status_for_present_files() {
        let (_dir, store) = store();
        let spec = catalog::find("qwen3-0.6b").unwrap();
        std::fs::write(store.path_for(spec), b"0123456789").unwrap();

        assert!(store.is_installed(spec));
        assert_eq!(store.installed_size(spec), Some(10));
        assert_eq!(store.total_disk_used(), 10);

        let status = store
            .statuses()
            .iter()
            .find(|s| s.id == spec.id)
            .cloned()
            .unwrap();
        assert!(status.installed);
        assert!(status.size_is_actual);
        assert_eq!(status.size_bytes, 10);
    }

    #[test]
    fn remove_deletes_only_when_present() {
        let (_dir, store) = store();
        let spec = catalog::find("qwen3-0.6b").unwrap();
        assert!(!store.remove(spec).unwrap());
        std::fs::write(store.path_for(spec), b"x").unwrap();
        assert!(store.remove(spec).unwrap());
        assert!(!store.is_installed(spec));
    }

    #[test]
    fn total_disk_used_ignores_non_gguf() {
        let (_dir, store) = store();
        std::fs::write(store.dir().join("notes.txt"), b"hello").unwrap();
        std::fs::write(store.dir().join("a.gguf"), b"1234").unwrap();
        assert_eq!(store.total_disk_used(), 4);
    }

    #[tokio::test]
    async fn stream_to_file_writes_and_reports_progress() {
        let mut server = mockito::Server::new_async().await;
        let body = vec![7u8; 4096];
        let mock = server
            .mock("GET", "/model.gguf")
            .with_status(200)
            .with_header("content-length", &body.len().to_string())
            .with_body(body)
            .create_async()
            .await;

        let (_dir, store) = store();
        let dest = store.dir().join("model.gguf");
        let client = reqwest::Client::new();

        let mut last = DownloadProgress {
            downloaded: 0,
            total: None,
        };
        super::stream_to_file(
            &client,
            &format!("{}/model.gguf", server.url()),
            &dest,
            |p| {
                last = p;
            },
        )
        .await
        .unwrap();

        assert_eq!(last.downloaded, 4096);
        assert_eq!(last.total, Some(4096));
        assert_eq!(last.fraction(), Some(1.0));
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 4096);
        // The temporary part file is gone after the atomic rename.
        assert!(!dest.with_extension("gguf.part").exists());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn stream_to_file_surfaces_http_errors() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/missing.gguf")
            .with_status(404)
            .create_async()
            .await;
        let (_dir, store) = store();
        let dest = store.dir().join("missing.gguf");
        let err = super::stream_to_file(
            &reqwest::Client::new(),
            &format!("{}/missing.gguf", server.url()),
            &dest,
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LocalError::Download(msg) if msg.contains("404")));
        assert!(!dest.exists());
    }
}
