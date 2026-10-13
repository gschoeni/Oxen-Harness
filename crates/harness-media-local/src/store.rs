//! Weight files on disk: `~/.oxen-harness/media/weights/<org>--<repo>/<path>`.
//!
//! Files are keyed by repo and path, not by model, so models that share a
//! text encoder (FLUX.2 klein and Z-Image both use Qwen3-4B) share one copy.
//! A file counts as present when its size matches the catalog; a download
//! lands in `<file>.part` and is checksummed before it is renamed into place,
//! so an interrupted or corrupt download never reads as installed.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::catalog::{ModelSpec, WeightFile};
use crate::LocalError;

/// Progress through one model's downloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullProgress {
    /// The file being fetched (`repo/path`).
    pub file: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
}

/// The weight store.
#[derive(Debug, Clone)]
pub struct WeightStore {
    root: PathBuf,
}

impl WeightStore {
    /// The store under the harness directory.
    pub fn open() -> Result<Self, LocalError> {
        let base = harness_config::paths::base_dir_unchecked().ok_or_else(|| {
            LocalError::Store("could not locate the ~/.oxen-harness directory".into())
        })?;
        Ok(Self::at(base.join("media").join("weights")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where `file` lives (whether or not it is there yet).
    pub fn path(&self, file: &WeightFile) -> PathBuf {
        self.root
            .join(file.repo.replace('/', "--"))
            .join(&file.path)
    }

    fn present(&self, file: &WeightFile) -> bool {
        std::fs::metadata(self.path(file)).is_ok_and(|m| m.len() == file.bytes)
    }

    /// Bytes still to download before `spec` can run (0 = installed).
    pub fn missing_bytes(&self, spec: &ModelSpec) -> u64 {
        spec.files
            .values()
            .filter(|f| !self.present(f))
            .map(|f| f.bytes)
            .sum()
    }

    pub fn is_installed(&self, spec: &ModelSpec) -> bool {
        self.missing_bytes(spec) == 0
    }

    /// Download every missing file of `spec` from Hugging Face.
    pub async fn pull(
        &self,
        spec: &ModelSpec,
        cancel: &CancellationToken,
        mut on_progress: impl FnMut(PullProgress),
    ) -> Result<(), LocalError> {
        let http = reqwest::Client::new();
        for file in spec.files.values().filter(|f| !self.present(f)) {
            let dest = self.path(file);
            if let Some(dir) = dest.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            let part = part_path(&dest);
            let label = format!("{}/{}", file.repo, file.path);
            let url = format!(
                "https://huggingface.co/{}/resolve/main/{}",
                file.repo, file.path
            );
            let token = std::env::var("HF_TOKEN").ok();
            let opts = harness_local::FetchOpts {
                token: token.as_deref(),
                gated_message: Some("Hugging Face refused the download (gated or private repo)"),
                cancel: Some(cancel),
                ..Default::default()
            };
            let fetched = harness_local::fetch_to_file(&http, &url, &part, opts, |done, _| {
                on_progress(PullProgress {
                    file: label.clone(),
                    done_bytes: done,
                    total_bytes: file.bytes,
                });
            })
            .await;
            if let Err(e) = fetched {
                remove_quietly(&part).await;
                return Err(LocalError::Store(format!("downloading {label}: {e}")));
            }
            if let Err(e) = verify(&part, file).await {
                remove_quietly(&part).await;
                return Err(e);
            }
            tokio::fs::rename(&part, &dest).await?;
        }
        Ok(())
    }

    /// Delete `spec`'s files that no model in `keep` (the other installed
    /// models) still uses. Returns the bytes freed.
    pub fn remove(&self, spec: &ModelSpec, keep: &[ModelSpec]) -> Result<u64, LocalError> {
        let mut freed = 0;
        for file in spec.files.values() {
            let shared = keep
                .iter()
                .filter(|other| other.id != spec.id)
                .any(|other| other.files.values().any(|f| f == file));
            let path = self.path(file);
            if shared || !path.exists() {
                continue;
            }
            freed += std::fs::metadata(&path).map(|m| m.len())?;
            std::fs::remove_file(&path)?;
        }
        Ok(freed)
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Size, then (when the catalog has one) the sha256 — hashed off the async
/// runtime, since a weight file is gigabytes.
async fn verify(path: &Path, file: &WeightFile) -> Result<(), LocalError> {
    let len = tokio::fs::metadata(path).await?.len();
    if len != file.bytes {
        return Err(LocalError::Store(format!(
            "{}/{} downloaded {len} bytes, expected {}",
            file.repo, file.path, file.bytes
        )));
    }
    let Some(want) = file.sha256.clone() else {
        return Ok(());
    };
    let owned = path.to_path_buf();
    let got = tokio::task::spawn_blocking(move || sha256_file(&owned))
        .await
        .map_err(|e| LocalError::Store(format!("checksum task failed: {e}")))??;
    if !got.eq_ignore_ascii_case(&want) {
        return Err(LocalError::Store(format!(
            "{}/{} failed its checksum (got {got}, expected {want})",
            file.repo, file.path
        )));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, LocalError> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Cleanup after a failed download: the failure being reported is the
/// download's, so a leftover `.part` is only logged.
async fn remove_quietly(path: &Path) {
    if let Err(e) = tokio::fs::remove_file(path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("could not remove {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::load_from;

    fn klein() -> ModelSpec {
        load_from(None)
            .expect("catalog")
            .into_iter()
            .find(|m| m.id == "local/flux-2-klein-4b")
            .expect("klein")
    }

    fn fake(store: &WeightStore, file: &WeightFile, len: u64) {
        let path = store.path(file);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let f = std::fs::File::create(&path).expect("create");
        f.set_len(len).expect("set_len");
    }

    #[test]
    fn installed_means_every_file_at_its_catalog_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = WeightStore::at(dir.path());
        let spec = klein();
        assert_eq!(store.missing_bytes(&spec), spec.total_bytes());
        for f in spec.files.values() {
            fake(&store, f, f.bytes);
        }
        assert!(store.is_installed(&spec));
        // A truncated file is not installed.
        let vae = &spec.files["vae"];
        fake(&store, vae, vae.bytes - 1);
        assert_eq!(store.missing_bytes(&spec), vae.bytes);
    }

    #[test]
    fn remove_keeps_files_another_model_shares() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = WeightStore::at(dir.path());
        let klein = klein();
        let zimage = load_from(None)
            .expect("catalog")
            .into_iter()
            .find(|m| m.id == "local/z-image-turbo")
            .expect("z-image");
        for f in klein.files.values().chain(zimage.files.values()) {
            fake(&store, f, 1);
        }
        let freed = store
            .remove(&klein, std::slice::from_ref(&zimage))
            .expect("remove");
        assert_eq!(freed, 2, "the diffusion model and VAE go");
        assert!(
            store.path(&klein.files["llm"]).exists(),
            "shared text encoder stays"
        );
        assert!(!store.path(&klein.files["diffusion_model"]).exists());
    }

    #[tokio::test]
    async fn a_checksum_mismatch_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("w.bin");
        std::fs::write(&path, b"hello").expect("write");
        let file = WeightFile {
            repo: "a/b".into(),
            path: "w.bin".into(),
            bytes: 5,
            sha256: Some("00".repeat(32)),
        };
        assert!(verify(&path, &file).await.is_err());
        let good = WeightFile {
            sha256: Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into()),
            ..file
        };
        verify(&path, &good).await.expect("matching checksum");
    }
}
