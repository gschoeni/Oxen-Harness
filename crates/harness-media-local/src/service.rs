//! [`LocalMediaService`]: the [`LocalMedia`] provider hosts install.
//!
//! It owns the catalog, the weight store, and the engines, and does the
//! work every engine would otherwise repeat: find the model, check it is
//! installed, resolve settings, queue jobs for the GPU one at a time, and
//! keep a cancel token per job so `media_status` can stop a queued or
//! running one.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use harness_media::{
    LocalJob, LocalMedia, LocalMediaError, LocalOutput, LocalProgress, MediaModel,
};
use tokio_util::sync::CancellationToken;

use crate::catalog::ModelSpec;
use crate::engine::{EngineRequest, MediaEngine};
use crate::store::WeightStore;

pub struct LocalMediaService {
    specs: Vec<ModelSpec>,
    store: WeightStore,
    engines: HashMap<&'static str, Arc<dyn MediaEngine>>,
    /// One job on the GPU at a time, in arrival order.
    gpu: tokio::sync::Mutex<()>,
    jobs: Mutex<HashMap<String, CancellationToken>>,
}

impl LocalMediaService {
    pub fn new(specs: Vec<ModelSpec>, store: WeightStore) -> Self {
        Self {
            specs,
            store,
            engines: HashMap::new(),
            gpu: tokio::sync::Mutex::new(()),
            jobs: Mutex::new(HashMap::new()),
        }
    }

    /// Register an engine; catalog entries naming its id run on it.
    pub fn with_engine(mut self, engine: Arc<dyn MediaEngine>) -> Self {
        self.engines.insert(engine.id(), engine);
        self
    }

    /// Every engine this build ships.
    pub fn with_default_engines(self) -> Self {
        self.with_engine(Arc::new(crate::sdcpp::SdCppEngine::default()))
    }

    pub fn specs(&self) -> &[ModelSpec] {
        &self.specs
    }

    pub fn store(&self) -> &WeightStore {
        &self.store
    }

    pub fn spec(&self, id: &str) -> Option<&ModelSpec> {
        let id = id.trim();
        self.specs.iter().find(|s| s.id == id).or_else(|| {
            // `flux-2-klein-4b` finds `local/flux-2-klein-4b`.
            self.specs
                .iter()
                .find(|s| s.id.strip_prefix(harness_media::local::LOCAL_PREFIX) == Some(id))
        })
    }

    pub fn engine(&self, id: &str) -> Option<&Arc<dyn MediaEngine>> {
        self.engines.get(id)
    }

    /// Stop every engine's server (app quit, or freeing memory on demand).
    pub async fn unload_all(&self) {
        for engine in self.engines.values() {
            engine.unload().await;
        }
    }

    fn jobs(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, CancellationToken>>, LocalMediaError>
    {
        self.jobs
            .lock()
            .map_err(|_| LocalMediaError::Failed("the local job table is poisoned".into()))
    }

    async fn run(
        &self,
        job: &LocalJob,
        progress: &(dyn Fn(LocalProgress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<LocalOutput, LocalMediaError> {
        let spec = self
            .spec(&job.model)
            .ok_or_else(|| LocalMediaError::Failed(format!("no local model `{}`", job.model)))?;
        let missing = self.store.missing_bytes(spec);
        if missing > 0 {
            return Err(LocalMediaError::NotInstalled(
                spec.id.clone(),
                format!(
                    "{:.1} GB to download; run `oxen-harness media pull {}`",
                    missing as f64 / 1e9,
                    spec.id
                ),
            ));
        }
        let engine = self.engines.get(spec.engine.as_str()).ok_or_else(|| {
            LocalMediaError::Failed(format!(
                "{} needs the `{}` engine, which this build doesn't have",
                spec.id, spec.engine
            ))
        })?;
        if job.refs.len() > spec.max_refs as usize {
            return Err(LocalMediaError::Failed(format!(
                "{} takes at most {} reference image(s); {} given",
                spec.id,
                spec.max_refs,
                job.refs.len()
            )));
        }
        let settings = spec
            .settings(&job.params, random_seed())
            .map_err(|e| LocalMediaError::Failed(e.to_string()))?;
        let weights: BTreeMap<String, PathBuf> = spec
            .files
            .iter()
            .map(|(role, f)| (role.clone(), self.store.path(f)))
            .collect();

        progress(LocalProgress::Queued);
        let _gpu = tokio::select! {
            turn = self.gpu.lock() => turn,
            _ = cancel.cancelled() => return Err(LocalMediaError::Cancelled),
        };
        engine
            .render(
                EngineRequest {
                    spec,
                    weights: &weights,
                    prompt: &job.prompt,
                    settings: &settings,
                    refs: &job.refs,
                },
                progress,
                cancel,
            )
            .await
    }
}

/// A job's entry in the cancel table, removed however the job ends —
/// including its future being dropped when a turn is interrupted, when the
/// token also fires so a queued job never starts.
struct Registered<'a> {
    jobs: &'a Mutex<HashMap<String, CancellationToken>>,
    id: String,
    cancel: CancellationToken,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.cancel.cancel();
        // A poisoned table only costs the stale entry; the job's own result
        // (often a finished image) must not be thrown away for it.
        match self.jobs.lock() {
            Ok(mut jobs) => {
                jobs.remove(&self.id);
            }
            Err(_) => tracing::warn!("local job table poisoned; {} stays listed", self.id),
        }
    }
}

/// A seed for a call that fixed none: varied per call, recorded on the row
/// so the output can be reproduced. Kept under 2^53 so it survives JSON.
fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // splitmix64: neighbouring calls land far apart.
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) & ((1 << 53) - 1)
}

#[async_trait]
impl LocalMedia for LocalMediaService {
    fn models(&self) -> Vec<MediaModel> {
        self.specs
            .iter()
            .map(|s| {
                let missing = self.store.missing_bytes(s);
                s.to_media_model(missing == 0, missing)
            })
            .collect()
    }

    async fn generate(
        &self,
        job: LocalJob,
        progress: Arc<dyn Fn(LocalProgress) + Send + Sync>,
    ) -> Result<LocalOutput, LocalMediaError> {
        let cancel = CancellationToken::new();
        self.jobs()?.insert(job.id.clone(), cancel.clone());
        let _registered = Registered {
            jobs: &self.jobs,
            id: job.id.clone(),
            cancel: cancel.clone(),
        };
        self.run(&job, progress.as_ref(), &cancel).await
    }

    async fn cancel(&self, id: &str) -> bool {
        match self.jobs() {
            Ok(jobs) => jobs.get(id).map(CancellationToken::cancel).is_some(),
            Err(e) => {
                tracing::warn!("could not cancel local job {id}: {e}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::load_from;
    use crate::engine::EngineStatus;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// What the fake engine was asked: prompt, width, height, seed, refs.
    type Seen = (String, u32, u32, u64, usize);

    /// An engine that records what it was asked and returns a 1×1 PNG, or
    /// waits for cancel when told to.
    #[derive(Default)]
    struct FakeEngine {
        calls: AtomicUsize,
        block: bool,
        seen: Mutex<Vec<Seen>>,
    }

    #[async_trait]
    impl MediaEngine for FakeEngine {
        fn id(&self) -> &'static str {
            "sd.cpp"
        }
        fn status(&self) -> EngineStatus {
            EngineStatus::Ready("test".into())
        }
        async fn render(
            &self,
            req: EngineRequest<'_>,
            progress: &(dyn Fn(LocalProgress) + Send + Sync),
            cancel: &CancellationToken,
        ) -> Result<LocalOutput, LocalMediaError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().expect("seen").push((
                req.prompt.to_string(),
                req.settings.width,
                req.settings.height,
                req.settings.seed,
                req.refs.len(),
            ));
            progress(LocalProgress::Step { step: 1, total: 4 });
            if self.block {
                cancel.cancelled().await;
                return Err(LocalMediaError::Cancelled);
            }
            Ok(LocalOutput {
                bytes: b"\x89PNG fake".to_vec(),
                seed: Some(Value::from(req.settings.seed)),
                record: json!({"engine": "fake"}),
            })
        }
        async fn unload(&self) {}
    }

    fn installed_service(engine: Arc<FakeEngine>) -> (tempfile::TempDir, LocalMediaService) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = WeightStore::at(dir.path());
        let specs = load_from(None).expect("catalog");
        for spec in &specs {
            for f in spec.files.values() {
                let p = store.path(f);
                std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
                std::fs::File::create(&p)
                    .and_then(|file| file.set_len(f.bytes))
                    .expect("fake weight");
            }
        }
        (
            dir,
            LocalMediaService::new(specs, store).with_engine(engine),
        )
    }

    fn job(model: &str, params: Value) -> LocalJob {
        LocalJob {
            id: "local-test-1".into(),
            model: model.into(),
            kind: harness_media::MediaKind::Image,
            prompt: "a fox".into(),
            params: params.as_object().cloned().unwrap_or_default(),
            refs: vec![],
        }
    }

    #[tokio::test]
    async fn models_report_install_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let service = LocalMediaService::new(
            load_from(None).expect("catalog"),
            WeightStore::at(dir.path()),
        );
        let models = service.models();
        assert_eq!(models.len(), 3);
        assert!(models.iter().all(|m| m
            .local
            .as_ref()
            .is_some_and(|l| !l.installed && l.download_bytes > 0)));
    }

    #[tokio::test]
    async fn generates_with_resolved_settings_and_progress() {
        let engine = Arc::new(FakeEngine::default());
        let (_dir, service) = installed_service(engine.clone());
        let heard = Arc::new(Mutex::new(Vec::new()));
        let h = heard.clone();
        let out = service
            .generate(
                job(
                    "local/iris-3b-turbo",
                    json!({"aspect_ratio": "16:9", "seed": 9}),
                ),
                Arc::new(move |p| h.lock().expect("heard").push(p)),
            )
            .await
            .expect("generates");
        assert!(out.bytes.starts_with(b"\x89PNG"));
        assert_eq!(out.seed, Some(Value::from(9u64)));
        let seen = engine.seen.lock().expect("seen").clone();
        assert_eq!(seen, vec![("a fox".to_string(), 1360, 768, 9, 0)]);
        let heard = heard.lock().expect("heard").clone();
        assert_eq!(heard.first(), Some(&LocalProgress::Queued));
        assert!(heard.contains(&LocalProgress::Step { step: 1, total: 4 }));
    }

    #[tokio::test]
    async fn uninstalled_models_say_how_to_install() {
        let dir = tempfile::tempdir().expect("tempdir");
        let service = LocalMediaService::new(
            load_from(None).expect("catalog"),
            WeightStore::at(dir.path()),
        )
        .with_engine(Arc::new(FakeEngine::default()));
        let err = service
            .generate(job("local/flux-2-klein-4b", json!({})), Arc::new(|_| {}))
            .await
            .expect_err("not installed");
        assert!(
            err.to_string()
                .contains("oxen-harness media pull local/flux-2-klein-4b"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn too_many_references_are_refused_before_the_engine_runs() {
        let engine = Arc::new(FakeEngine::default());
        let (_dir, service) = installed_service(engine.clone());
        let mut j = job("local/iris-3b-turbo", json!({}));
        j.refs = vec![PathBuf::from("a.png")];
        let err = service
            .generate(j, Arc::new(|_| {}))
            .await
            .expect_err("iris takes no refs");
        assert!(err.to_string().contains("at most 0"), "{err}");
        assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_stops_a_running_job() {
        let engine = Arc::new(FakeEngine {
            block: true,
            ..Default::default()
        });
        let (_dir, service) = installed_service(engine);
        let service = Arc::new(service);
        let s2 = service.clone();
        let running = tokio::spawn(async move {
            s2.generate(job("local/flux-2-klein-4b", json!({})), Arc::new(|_| {}))
                .await
        });
        // Wait until the job is registered, then cancel it by id.
        for _ in 0..100 {
            if service.cancel("local-test-1").await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let result = running.await.expect("task");
        assert!(matches!(result, Err(LocalMediaError::Cancelled)));
        assert!(
            !service.cancel("local-test-1").await,
            "finished jobs are forgotten"
        );
    }

    #[test]
    fn short_ids_resolve() {
        let dir = tempfile::tempdir().expect("tempdir");
        let service = LocalMediaService::new(
            load_from(None).expect("catalog"),
            WeightStore::at(dir.path()),
        );
        assert_eq!(
            service.spec("flux-2-klein-4b").map(|s| s.id.as_str()),
            Some("local/flux-2-klein-4b")
        );
        assert!(random_seed() < 1 << 53);
    }
}
