//! Reference media as hub uploads.
//!
//! A generation request wants URLs the hub's workers can fetch, so every
//! reference the user dropped (an image, a clip, a soundtrack) is uploaded
//! to the user's `playground` repository on the hub — the same place the
//! hub's own workbench keeps uploads — through the workbench context
//! endpoint, then presigned into a public, expiring URL:
//!
//! 1. `GET /api/users/me` → the account's namespace (once per session).
//! 2. `GET /api/repos/{ns}/playground` → exists, or `POST /api/repos/`
//!    creates it (private, `workbench: true`).
//! 3. `POST /api/repos/{ns}/{repo}/workbench/context` multipart `file`
//!    (+ `directory`) → `asset_path`.
//! 4. `GET /api/repos/{ns}/{repo}/file/presigned_url/main/{asset_path}`
//!    → `{url}`, fetchable without auth for about an hour.
//!
//! Uploads stream with progress (the hosts show a bar per file), and a
//! content-hash cache remembers each file's `asset_path` so re-using a
//! reference costs one presign call, not another upload.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest;

use crate::refs::{mime_for, RefKind};

/// Default repository name for uploads (the hub creates it on demand).
pub const DEFAULT_UPLOAD_REPO: &str = "playground";
/// Folder inside the repo the harness uploads under.
pub const UPLOAD_DIRECTORY: &str = "oxen-harness";
/// Progress is reported at least every this many bytes.
const PROGRESS_STEP: u64 = 128 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("the hub rejected the API key (401): check Settings → Connection")]
    Unauthorized,
    #[error("hub error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("network error: {0}")]
    Http(String),
    #[error("{0}")]
    Io(String),
    #[error("unexpected response: {0}")]
    Json(String),
}

/// A reference after upload: where it lives and the URL to hand the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedRef {
    pub asset_path: String,
    pub url: String,
    pub bytes: u64,
    /// Whether the bytes were already on the hub (cache hit).
    pub reused: bool,
}

/// One remembered upload, keyed by content hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedUpload {
    namespace: String,
    repo: String,
    asset_path: String,
    bytes: u64,
    uploaded_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UploadCache {
    #[serde(default)]
    entries: std::collections::HashMap<String, CachedUpload>,
}

/// Uploads references to the hub for one session.
pub struct HubUploader {
    http: reqwest::Client,
    api_root: String,
    api_key: String,
    repos: crate::hub::HubRepos,
    /// `namespace/repo` from the user's settings, else the account's
    /// `playground`.
    override_repo: Option<(String, String)>,
    target: tokio::sync::OnceCell<(String, String)>,
    cache_path: Option<PathBuf>,
    cache: Mutex<UploadCache>,
}

impl HubUploader {
    /// `api_root` is the hub's `/api` root (see
    /// [`crate::queue::api_root_from_base`]).
    pub fn new(
        http: reqwest::Client,
        api_root: impl Into<String>,
        api_key: impl Into<String>,
        override_repo: Option<(String, String)>,
        cache_path: Option<PathBuf>,
    ) -> Self {
        let cache = cache_path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let api_root = api_root.into();
        let api_key = api_key.into();
        Self {
            repos: crate::hub::HubRepos::new(http.clone(), api_root.clone(), api_key.clone()),
            http,
            api_root,
            api_key,
            override_repo,
            target: tokio::sync::OnceCell::new(),
            cache_path,
            cache: Mutex::new(cache),
        }
    }

    /// The `(namespace, repo)` uploads go to, resolved once: the override,
    /// else the account's playground. The hub has no "my namespaces" call
    /// and a user's playground can live under an organization, so the
    /// namespace is tried in order — the username from `users/me`, then the
    /// namespace the queue last billed to (`target_namespace` of a recent
    /// generation) — and the first one whose playground answers wins; with
    /// none, the playground is created under the username.
    pub async fn target(&self) -> Result<(String, String), UploadError> {
        self.target
            .get_or_try_init(|| async {
                if let Some(target) = &self.override_repo {
                    return Ok(target.clone());
                }
                let username = self.whoami().await?;
                let mut candidates = vec![username.clone()];
                if let Some(ns) = self.queue_namespace().await {
                    if !candidates.contains(&ns) {
                        candidates.push(ns);
                    }
                }
                for ns in &candidates {
                    if self.repo_exists(ns, DEFAULT_UPLOAD_REPO).await? {
                        return Ok((ns.clone(), DEFAULT_UPLOAD_REPO.to_string()));
                    }
                }
                self.create_repo(&username, DEFAULT_UPLOAD_REPO).await?;
                Ok((username, DEFAULT_UPLOAD_REPO.to_string()))
            })
            .await
            .cloned()
    }

    /// The namespace a recent generation was billed to, if the queue lists
    /// one — the hub's own default for this account.
    async fn queue_namespace(&self) -> Option<String> {
        let res = crate::hub::send(
            self.http
                .get(format!("{}/ai/queue", self.api_root))
                .bearer_auth(&self.api_key)
                .timeout(Duration::from_secs(20)),
            true,
        )
        .await
        .ok()?;
        if !res.status().is_success() {
            return None;
        }
        let v: Value = res.json().await.ok()?;
        v.get("generations")?
            .as_array()?
            .iter()
            .find_map(|g| g.get("target_namespace").and_then(Value::as_str))
            .map(str::to_string)
    }

    async fn whoami(&self) -> Result<String, UploadError> {
        self.repos.whoami().await
    }

    /// Whether `namespace/repo` exists and this key can see it (a 403 reads
    /// as "not for this key", not as an error — another namespace may be).
    async fn repo_exists(&self, namespace: &str, repo: &str) -> Result<bool, UploadError> {
        self.repos.repo_exists(namespace, repo).await
    }

    /// Create the playground under `namespace`; when the hub refuses (this
    /// key can't create, or the name is taken elsewhere), say what to set.
    async fn create_repo(&self, namespace: &str, repo: &str) -> Result<(), UploadError> {
        let new = crate::hub::NewRepo {
            name: repo,
            namespace: Some(namespace),
            description: "Uploads and generations from oxen-harness.",
            public: false,
            workbench: true,
        };
        match self.repos.create_repo(new).await {
            Ok(()) => Ok(()),
            Err(UploadError::Api { status, message }) => Err(UploadError::Api {
                status,
                message: format!(
                    "could not find or create `{namespace}/{repo}` for this API key \
                     ({status}: {message}). Set Settings → Media → Hub repo to a repository \
                     this key can write to (for example `ox/playground`)."
                ),
            }),
            Err(UploadError::Unauthorized) => Err(UploadError::Api {
                status: 401,
                message: format!(
                    "this API key isn't allowed to create repositories, and no `{repo}` \
                     repository was found for it. Set Settings → Media → Hub repo to a \
                     repository this key can write to (for example `ox/playground`)."
                ),
            }),
            Err(e) => Err(e),
        }
    }

    /// Upload `path` (or reuse a previous upload of the same bytes) and
    /// presign it. `on_progress(sent, total)` fires as bytes go out.
    pub async fn upload(
        &self,
        path: &Path,
        kind: RefKind,
        on_progress: Arc<dyn Fn(u64, u64) + Send + Sync>,
    ) -> Result<UploadedRef, UploadError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| UploadError::Io(format!("{}: {e}", path.display())))?;
        let total = bytes.len() as u64;
        let hash = hex(&sha2::Sha256::digest(&bytes));
        let (namespace, repo) = self.target().await?;

        if let Some(cached) = self.cached(&hash, &namespace, &repo) {
            on_progress(total, total);
            match self.presign(&namespace, &repo, &cached.asset_path).await {
                Ok(url) => {
                    return Ok(UploadedRef {
                        asset_path: cached.asset_path,
                        url,
                        bytes: total,
                        reused: true,
                    })
                }
                // The file may have been deleted on the hub; upload again.
                Err(UploadError::Api { status: 404, .. }) => self.forget(&hash),
                Err(e) => return Err(e),
            }
        }

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("reference")
            .to_string();
        let mime = mime_for(path).to_string();
        let progress = on_progress.clone();
        // Stream the bytes in chunks so progress moves while the request is
        // in flight (a single buffered body would jump 0 → 100).
        let chunks: Vec<Vec<u8>> = bytes
            .chunks(PROGRESS_STEP as usize)
            .map(<[u8]>::to_vec)
            .collect();
        let sent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let stream = futures_util::stream::iter(chunks.into_iter().map({
            let sent = sent.clone();
            move |chunk| {
                let n = sent.fetch_add(chunk.len() as u64, std::sync::atomic::Ordering::Relaxed)
                    + chunk.len() as u64;
                progress(n.min(total), total);
                Ok::<Vec<u8>, std::io::Error>(chunk)
            }
        }));
        let part =
            reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), total)
                .file_name(filename)
                .mime_str(&mime)
                .map_err(|e| UploadError::Io(e.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("directory", UPLOAD_DIRECTORY);
        // A multipart body streams, so this is sent exactly once (the retry
        // helper can't clone it); the routing goes through it for one
        // consistent error mapping.
        let res = crate::hub::send(
            self.http
                .post(format!(
                    "{}/repos/{namespace}/{repo}/workbench/context",
                    self.api_root
                ))
                .bearer_auth(&self.api_key)
                .multipart(form)
                .timeout(Duration::from_secs(20 * 60)),
            false,
        )
        .await?;
        let v: Value = crate::hub::body(res).await.map_err(|e| match e {
            UploadError::Api { status, message } => UploadError::Api {
                status,
                message: format!("uploading to {namespace}/{repo}: {message}"),
            },
            other => other,
        })?;
        let asset_path = v
            .get("asset_path")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| UploadError::Json("upload returned no asset_path".into()))?;
        on_progress(total, total);
        self.remember(
            &hash,
            CachedUpload {
                namespace: namespace.clone(),
                repo: repo.clone(),
                asset_path: asset_path.clone(),
                bytes: total,
                uploaded_at: crate::library::now_unix(),
            },
        );
        let _ = kind;
        let url = self.presign(&namespace, &repo, &asset_path).await?;
        Ok(UploadedRef {
            asset_path,
            url,
            bytes: total,
            reused: false,
        })
    }

    /// A public, expiring URL for a file in the repo.
    pub async fn presign(
        &self,
        namespace: &str,
        repo: &str,
        asset_path: &str,
    ) -> Result<String, UploadError> {
        let res = crate::hub::send(
            self.http
                .get(format!(
                    "{}/repos/{namespace}/{repo}/file/presigned_url/main/{asset_path}",
                    self.api_root
                ))
                .bearer_auth(&self.api_key)
                .timeout(Duration::from_secs(30)),
            true,
        )
        .await?;
        let v: Value = crate::hub::body(res).await.map_err(|e| match e {
            UploadError::Api { status, message } => UploadError::Api {
                status,
                message: format!("presigning {asset_path} in {namespace}/{repo}: {message}"),
            },
            other => other,
        })?;
        let url = v
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| UploadError::Json("presign returned no url".into()))?;
        if !url.starts_with("http") {
            return Err(UploadError::Json(format!(
                "presign returned a relative url: {url}"
            )));
        }
        Ok(url.to_string())
    }

    fn cached(&self, hash: &str, namespace: &str, repo: &str) -> Option<CachedUpload> {
        self.cache
            .lock()
            .expect("upload cache poisoned")
            .entries
            .get(hash)
            .filter(|c| c.namespace == namespace && c.repo == repo)
            .cloned()
    }

    fn remember(&self, hash: &str, entry: CachedUpload) {
        let mut cache = self.cache.lock().expect("upload cache poisoned");
        cache.entries.insert(hash.to_string(), entry);
        self.persist(&cache);
    }

    fn forget(&self, hash: &str) {
        let mut cache = self.cache.lock().expect("upload cache poisoned");
        cache.entries.remove(hash);
        self.persist(&cache);
    }

    fn persist(&self, cache: &UploadCache) {
        let Some(path) = &self.cache_path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(cache) {
            let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `~/.oxen-harness/cache/media-uploads-<host>.json`.
pub fn upload_cache_path(api_root: &str) -> Option<PathBuf> {
    let host: String = api_root
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("hub")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    harness_config::paths::cache_dir()
        .ok()
        .map(|d| d.join(format!("media-uploads-{host}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(3, 2, image::Rgb([1, 2, 3]))
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    #[tokio::test]
    async fn uploads_presigns_reports_progress_and_reuses() {
        let mut server = mockito::Server::new_async().await;
        let me = server
            .mock("GET", "/api/users/me")
            .with_body(json!({"user": {"username": "greg"}}).to_string())
            .expect(1)
            .create_async()
            .await;
        let missing = server
            .mock("GET", "/api/repos/greg/playground")
            .with_status(404)
            .with_body(r#"{"error":{"type":"resource_not_found"}}"#)
            .expect(1)
            .create_async()
            .await;
        let create = server
            .mock("POST", "/api/repos/")
            .match_body(mockito::Matcher::PartialJson(
                json!({"name": "playground", "workbench": true}),
            ))
            .with_body(
                json!({"repository": {"name": "playground", "namespace": "greg"}}).to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let upload = server
            .mock("POST", "/api/repos/greg/playground/workbench/context")
            .match_header(
                "content-type",
                mockito::Matcher::Regex("multipart/form-data.*".into()),
            )
            .with_body(
                json!({"status": "success", "asset_path": "oxen/context/x.png", "uuid": "u"})
                    .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let presign = server
            .mock("GET", "/api/repos/greg/playground/file/presigned_url/main/oxen/context/x.png")
            .with_body(json!({"url": "https://hub.test/api/repos/greg/playground/file/main/oxen/context/x.png?oxen_signature=s"}).to_string())
            .expect(2)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ref.png");
        std::fs::write(&file, png()).unwrap();
        let cache = dir.path().join("cache/uploads.json");
        let uploader = HubUploader::new(
            reqwest::Client::new(),
            format!("{}/api", server.url()),
            "k",
            None,
            Some(cache.clone()),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let first = uploader
            .upload(
                &file,
                RefKind::Image,
                Arc::new(move |s, t| seen2.lock().unwrap().push((s, t))),
            )
            .await
            .unwrap();
        assert!(!first.reused);
        assert_eq!(first.asset_path, "oxen/context/x.png");
        assert!(first.url.contains("oxen_signature"));
        let progress = seen.lock().unwrap().clone();
        assert!(!progress.is_empty());
        assert_eq!(progress.last().unwrap().0, progress.last().unwrap().1);
        assert!(cache.exists());

        // Same bytes again: no second upload, one more presign.
        let again = uploader
            .upload(&file, RefKind::Image, Arc::new(|_, _| {}))
            .await
            .unwrap();
        assert!(again.reused);
        me.assert_async().await;
        missing.assert_async().await;
        create.assert_async().await;
        upload.assert_async().await;
        presign.assert_async().await;

        // A fresh uploader reads the persisted cache.
        let fresh = HubUploader::new(
            reqwest::Client::new(),
            format!("{}/api", server.url()),
            "k",
            Some(("greg".into(), "playground".into())),
            Some(cache),
        );
        assert!(fresh
            .cached(&hex(&sha2::Sha256::digest(png())), "greg", "playground")
            .is_some());
    }

    #[tokio::test]
    async fn falls_back_to_the_namespace_the_queue_bills_to() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/users/me")
            .with_body(json!({"user": {"username": "ME"}}).to_string())
            .create_async()
            .await;
        server
            .mock("GET", "/api/repos/ME/playground")
            .with_status(404)
            .with_body(r#"{"error":{"type":"resource_not_found"}}"#)
            .create_async()
            .await;
        server
            .mock("GET", "/api/ai/queue")
            .with_body(
                json!({"generations": [{"generation_id": "g", "target_namespace": "ox"}]})
                    .to_string(),
            )
            .create_async()
            .await;
        let seen = server
            .mock("GET", "/api/repos/ox/playground")
            .with_body(json!({"repository": {"name": "playground", "namespace": "ox"}}).to_string())
            .expect(1)
            .create_async()
            .await;
        let never_created = server
            .mock("POST", "/api/repos/")
            .expect(0)
            .create_async()
            .await;
        let uploader = HubUploader::new(
            reqwest::Client::new(),
            format!("{}/api", server.url()),
            "k",
            None,
            None,
        );
        assert_eq!(
            uploader.target().await.unwrap(),
            ("ox".to_string(), "playground".to_string())
        );
        seen.assert_async().await;
        never_created.assert_async().await;
    }

    #[tokio::test]
    async fn a_key_that_cannot_create_gets_a_settings_hint() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/users/me")
            .with_body(json!({"user": {"username": "ME"}}).to_string())
            .create_async()
            .await;
        server
            .mock("GET", "/api/repos/ME/playground")
            .with_status(404)
            .with_body(r#"{"error":{"type":"resource_not_found"}}"#)
            .create_async()
            .await;
        server
            .mock("GET", "/api/ai/queue")
            .with_status(401)
            .with_body(r#"{"error":{"type":"unauthenticated"}}"#)
            .create_async()
            .await;
        server
            .mock("POST", "/api/repos/")
            .with_status(401)
            .with_body(
                r#"{"error":{"type":"unauthenticated","title":"You must be authenticated"}}"#,
            )
            .create_async()
            .await;
        let uploader = HubUploader::new(
            reqwest::Client::new(),
            format!("{}/api", server.url()),
            "k",
            None,
            None,
        );
        let err = uploader.target().await.unwrap_err();
        assert!(
            err.to_string().contains("Settings → Media → Hub repo"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn unauthorized_is_named_and_override_skips_whoami() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/api/repos/ox/art/workbench/context")
            .with_status(401)
            .with_body(r#"{"error":{"type":"unauthenticated"}}"#)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        std::fs::write(&file, png()).unwrap();
        let uploader = HubUploader::new(
            reqwest::Client::new(),
            format!("{}/api", server.url()),
            "bad",
            Some(("ox".into(), "art".into())),
            None,
        );
        let err = uploader
            .upload(&file, RefKind::Image, Arc::new(|_, _| {}))
            .await
            .unwrap_err();
        assert!(matches!(err, UploadError::Unauthorized), "{err}");
    }
}
