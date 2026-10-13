//! The stable-diffusion.cpp engine: a managed `sd-server` child process
//! driven over its native async job API.
//!
//! - **Runtime.** A pinned prebuilt release unpacked into
//!   `~/.oxen-harness/runtime/stable-diffusion.cpp/` (the same shape as the
//!   managed llama.cpp runtime); `SD_SERVER` points at another binary.
//! - **Lifecycle.** The server starts on the first job — never at boot —
//!   with one model's weights, stays up while jobs keep coming (a warm model
//!   skips loading and reuses cached prompt encodings), restarts when a job
//!   wants a different model, and stops after [`IDLE_UNLOAD`] without work.
//!   Every spawn is recorded in the shared pid registry so a host that dies
//!   without stopping it gets its server reaped at the next boot.
//! - **Memory.** Weights are memory-mapped (`--mmap`): the pages are
//!   file-backed, so macOS can drop and re-read them under pressure instead
//!   of swapping the user's apps out. Measured on a 36 GB M4 Max already
//!   deep in swap: ~0.7 GB process footprint instead of ~7.9 GB, with no
//!   change in speed. A load is refused outright under critical pressure.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use harness_media::{LocalMediaError, LocalOutput, LocalProgress};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::engine::{EngineRequest, EngineStatus, MediaEngine};
use crate::LocalError;

/// The engine id catalog entries name.
pub const ENGINE_ID: &str = "sd.cpp";

/// The stable-diffusion.cpp release we pin to — benchmarked with every
/// built-in model (Iris-3B support landed in master-950). Bump deliberately.
pub const PINNED_VERSION: &str = "master-959-d637cfb";
const PINNED_COMMIT: &str = "d637cfb";

/// How long a loaded model stays resident without a job.
pub const IDLE_UNLOAD: Duration = Duration::from_secs(5 * 60);

/// How long a server gets to come up and answer.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

/// How long one output may take before it is abandoned: generous, since a
/// slow model at a large size on a busy machine can take minutes.
const JOB_TIMEOUT: Duration = Duration::from_secs(20 * 60);

const POLL: Duration = Duration::from_millis(400);

/// How long the server may fail to answer polls before the job fails.
const UNANSWERED_LIMIT: Duration = Duration::from_secs(30);

/// The pid registry file `sd-server`s are recorded in.
pub const PID_FILE: &str = harness_local::SD_PIDS;

/// What an image job adds to memory: compute buffers plus the slice of the
/// memory-mapped weights in use (measured: 0.6–1.0 GB of footprint at
/// 1024²; the rest of the weights are file pages the OS can drop).
const WORKING_SET: u64 = 3 * 1024 * 1024 * 1024;

/// Where managed runtimes live: `~/.oxen-harness/runtime/stable-diffusion.cpp/`.
fn runtime_root() -> Option<PathBuf> {
    Some(
        harness_config::paths::base_dir_unchecked()?
            .join("runtime")
            .join("stable-diffusion.cpp"),
    )
}

/// The release asset for this platform, or `None` where no prebuilt
/// Metal build is published (only Apple Silicon is wired up).
pub fn asset_name() -> Option<String> {
    (cfg!(target_os = "macos") && cfg!(target_arch = "aarch64"))
        .then(|| format!("sd-master-{PINNED_COMMIT}-bin-Darwin-macOS-26.6.2-arm64.zip"))
}

/// The managed `sd-server`, if the pinned version is unpacked.
pub fn managed_binary() -> Option<PathBuf> {
    let bin = runtime_root()?
        .join(format!("sd-{PINNED_VERSION}"))
        .join("sd-server");
    bin.is_file().then_some(bin)
}

/// The `sd-server` to run: `SD_SERVER` when set, else the managed one.
pub fn binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SD_SERVER").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    managed_binary()
}

/// Download and unpack the pinned release. A no-op when it is present.
pub async fn install_runtime(
    cancel: &CancellationToken,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, LocalError> {
    if let Some(bin) = managed_binary() {
        return Ok(bin);
    }
    let asset = asset_name().ok_or_else(|| {
        LocalError::Runtime(
            "no prebuilt stable-diffusion.cpp for this platform; build sd-server and set \
             SD_SERVER to its path"
                .into(),
        )
    })?;
    let root = runtime_root()
        .ok_or_else(|| LocalError::Runtime("could not locate ~/.oxen-harness".into()))?;
    tokio::fs::create_dir_all(&root).await?;
    let zip = root.join(format!("{asset}.part"));
    let url = format!(
        "https://github.com/leejet/stable-diffusion.cpp/releases/download/{PINNED_VERSION}/{asset}"
    );
    let opts = harness_local::FetchOpts {
        cancel: Some(cancel),
        ..Default::default()
    };
    harness_local::fetch_to_file(&reqwest::Client::new(), &url, &zip, opts, on_progress)
        .await
        .map_err(|e| LocalError::Runtime(format!("downloading {url}: {e}")))?;
    let dest = root.join(format!("sd-{PINNED_VERSION}"));
    // Unpack beside the destination and rename once complete, so an
    // interrupted unpack never leaves an `sd-server` without its dylib that
    // reads as installed. `ditto` ships with macOS (the only platform with
    // an asset) and keeps the executable bits.
    let unpacking = root.join(format!("sd-{PINNED_VERSION}.unpacking"));
    if unpacking.exists() {
        tokio::fs::remove_dir_all(&unpacking).await?;
    }
    let status = tokio::process::Command::new("ditto")
        .args(["-x", "-k"])
        .arg(&zip)
        .arg(&unpacking)
        .status()
        .await?;
    if let Err(e) = tokio::fs::remove_file(&zip).await {
        tracing::warn!("could not remove {}: {e}", zip.display());
    }
    if !status.success() || !unpacking.join("sd-server").is_file() {
        return Err(LocalError::Runtime(format!(
            "unpacking {asset} failed ({status}) or held no sd-server"
        )));
    }
    if dest.exists() {
        tokio::fs::remove_dir_all(&dest).await?;
    }
    tokio::fs::rename(&unpacking, &dest).await?;
    managed_binary()
        .ok_or_else(|| LocalError::Runtime(format!("{asset} unpacked without an sd-server binary")))
}

/// A running server and the model it holds.
struct Server {
    child: tokio::process::Child,
    pid: u32,
    base: String,
    model: String,
    /// Whether the server offers cheap latent previews, which is how it
    /// reports step progress.
    previews: bool,
    /// Stops the server if memory runs out while it loads or renders.
    watchdog: Option<harness_local::memory::Watchdog>,
}

impl Server {
    async fn stop(mut self) {
        if let Err(e) = self.child.start_kill() {
            tracing::warn!("could not stop sd-server {}: {e}", self.pid);
        }
        if tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .is_err()
        {
            tracing::warn!("sd-server {} did not exit within 10s", self.pid);
        }
        harness_local::unregister_spawned(PID_FILE, self.pid);
    }

    fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Why the memory guard stopped it, if it did.
    fn memory_trip(&self) -> Option<String> {
        self.watchdog.as_ref().and_then(|w| w.tripped())
    }

    /// The error for a server that died: the guard's reason when it was
    /// the guard, else what the log says.
    fn death(&self, what: &str, tail: String) -> LocalMediaError {
        match self.memory_trip() {
            Some(reason) => LocalMediaError::Memory {
                model: self.model.clone(),
                detail: reason,
            },
            None => LocalMediaError::Failed(format!("sd-server {what}{tail}")),
        }
    }
}

/// The engine. One server at a time: jobs are serialized on `server`.
pub struct SdCppEngine {
    http: reqwest::Client,
    server: Arc<Mutex<Option<Server>>>,
    /// Bumped by every job; an idle timer only unloads if it is unchanged.
    jobs: Arc<AtomicU64>,
    idle: Duration,
    log: Option<PathBuf>,
}

impl Default for SdCppEngine {
    fn default() -> Self {
        Self::new(IDLE_UNLOAD)
    }
}

impl SdCppEngine {
    pub fn new(idle: Duration) -> Self {
        Self {
            http: reqwest::Client::new(),
            server: Arc::new(Mutex::new(None)),
            jobs: Arc::new(AtomicU64::new(0)),
            idle,
            log: runtime_root().map(|r| r.join("sd-server.log")),
        }
    }

    /// The running server for `req`'s model, starting (or swapping to) it.
    async fn ensure_server<'a>(
        &self,
        slot: &'a mut Option<Server>,
        req: &EngineRequest<'_>,
        progress: &(dyn Fn(LocalProgress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<&'a Server, LocalMediaError> {
        if let Some(s) = slot.as_mut() {
            if s.model != req.spec.id || s.exited() {
                if let Some(old) = slot.take() {
                    old.stop().await;
                }
            }
        }
        if slot.is_none() {
            *slot = Some(self.spawn(req, progress, cancel).await?);
        }
        slot.as_ref()
            .ok_or_else(|| LocalMediaError::Failed("sd-server did not start".into()))
    }

    async fn spawn(
        &self,
        req: &EngineRequest<'_>,
        progress: &(dyn Fn(LocalProgress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<Server, LocalMediaError> {
        let bin = binary().ok_or_else(|| {
            LocalMediaError::NotInstalled(
                "the stable-diffusion.cpp runtime".into(),
                format!("run `oxen-harness media pull {}`", req.spec.id),
            )
        })?;
        use harness_local::memory::{preflight, LoadAdvice, Residency};
        match preflight(&req.spec.display_name, WORKING_SET, Residency::Mapped) {
            LoadAdvice::Go => {}
            LoadAdvice::Warn(message) => progress(LocalProgress::Warning(message)),
            LoadAdvice::Refuse(detail) => {
                return Err(LocalMediaError::Memory {
                    model: req.spec.id.clone(),
                    detail,
                })
            }
        }
        let port = free_port().map_err(|e| LocalMediaError::Failed(e.to_string()))?;
        let mut cmd = tokio::process::Command::new(&bin);
        cmd.args(["--listen-ip", "127.0.0.1", "--listen-port"])
            .arg(port.to_string())
            .args(server_args(req.spec, req.weights)?)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        match self.log.as_deref().map(std::fs::File::create) {
            Some(Ok(log)) => {
                let err = log
                    .try_clone()
                    .map_err(|e| LocalMediaError::Failed(format!("sd-server log: {e}")))?;
                cmd.stdout(log).stderr(err);
            }
            Some(Err(e)) => {
                tracing::warn!("could not open the sd-server log: {e}");
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
            None => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        let child = cmd.spawn().map_err(|e| {
            LocalMediaError::Failed(format!("could not start {}: {e}", bin.display()))
        })?;
        let pid = child.id().ok_or_else(|| {
            LocalMediaError::Failed(format!(
                "sd-server exited as soon as it started{}",
                self.log_tail()
            ))
        })?;
        harness_local::register_spawned(PID_FILE, pid, port, &req.spec.id);
        // The engine is a never-dropped static, so `kill_on_drop` alone
        // would leave this server behind after every CLI run.
        harness_local::spawn_lifeline(pid);
        let mut server = Server {
            child,
            pid,
            base: format!("http://127.0.0.1:{port}"),
            model: req.spec.id.clone(),
            previews: false,
            watchdog: Some(harness_local::memory::Watchdog::spawn(
                pid,
                &req.spec.display_name,
                harness_local::memory::Limits::default(),
                |_| {},
            )),
        };
        let started = Instant::now();
        loop {
            if cancel.is_cancelled() {
                server.stop().await;
                return Err(LocalMediaError::Cancelled);
            }
            if server.exited() {
                let err = server.death(
                    &format!("exited while loading {}", req.spec.id),
                    self.log_tail(),
                );
                server.stop().await;
                return Err(err);
            }
            if let Ok(caps) = self.get(&server.base, "/sdcpp/v1/capabilities").await {
                let modes = caps.get("preview_modes").and_then(Value::as_array);
                server.previews = modes.is_some_and(|m| m.iter().any(|v| v == "proj"));
                return Ok(server);
            }
            if started.elapsed() > STARTUP_TIMEOUT {
                server.stop().await;
                return Err(LocalMediaError::Failed(format!(
                    "sd-server did not answer within {}s",
                    STARTUP_TIMEOUT.as_secs()
                )));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn get(&self, base: &str, path: &str) -> Result<Value, String> {
        let res = self
            .http
            .get(format!("{base}{path}"))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("HTTP {}", res.status()));
        }
        res.json().await.map_err(|e| e.to_string())
    }

    /// The last lines of the server's log, for an error message.
    fn log_tail(&self) -> String {
        let Some(text) = self
            .log
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
        else {
            return String::new();
        };
        let lines: Vec<&str> = text
            .split(['\n', '\r'])
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('|'))
            .collect();
        let tail = lines[lines.len().saturating_sub(6)..].join("\n  ");
        if tail.is_empty() {
            String::new()
        } else {
            format!(":\n  {tail}")
        }
    }

    /// Unload after [`Self::idle`] unless another job comes first.
    fn schedule_unload(&self) {
        let ticket = self.jobs.fetch_add(1, Ordering::SeqCst) + 1;
        let jobs = self.jobs.clone();
        let server = self.server.clone();
        let idle = self.idle;
        tokio::spawn(async move {
            tokio::time::sleep(idle).await;
            if jobs.load(Ordering::SeqCst) != ticket {
                return;
            }
            let mut slot = server.lock().await;
            if jobs.load(Ordering::SeqCst) == ticket {
                if let Some(s) = slot.take() {
                    tracing::info!("unloading idle local image model {}", s.model);
                    s.stop().await;
                }
            }
        });
    }
}

/// Command-line flags for a model's weights. Unknown roles are an error:
/// a catalog entry written for another engine must not half-load here.
fn server_args(
    spec: &crate::catalog::ModelSpec,
    weights: &std::collections::BTreeMap<String, PathBuf>,
) -> Result<Vec<String>, LocalMediaError> {
    let mut args = Vec::new();
    for (role, path) in weights {
        let flag = match role.as_str() {
            "model" => "--model",
            "diffusion_model" => "--diffusion-model",
            "vae" => "--vae",
            "llm" => "--llm",
            "t5xxl" => "--t5xxl",
            "clip_l" => "--clip_l",
            "clip_g" => "--clip_g",
            other => {
                return Err(LocalMediaError::Failed(format!(
                    "{}: sd.cpp has no weight role `{other}`",
                    spec.id
                )))
            }
        };
        args.push(flag.to_string());
        args.push(path.to_string_lossy().into_owned());
    }
    args.extend(["--mmap".to_string(), "--diffusion-fa".to_string()]);
    args.extend(spec.engine_args.iter().cloned());
    Ok(args)
}

/// The job body for `/sdcpp/v1/img_gen`.
fn job_body(req: &EngineRequest<'_>, previews: bool) -> Result<Value, LocalMediaError> {
    let s = req.settings;
    let mut refs = Vec::new();
    for path in req.refs {
        let bytes = std::fs::read(path).map_err(|e| {
            LocalMediaError::Failed(format!("reading reference {}: {e}", path.display()))
        })?;
        refs.push(Value::String(
            base64::engine::general_purpose::STANDARD.encode(bytes),
        ));
    }
    Ok(json!({
        "prompt": req.prompt,
        "negative_prompt": s.negative_prompt.clone().unwrap_or_default(),
        "width": s.width,
        "height": s.height,
        "seed": s.seed,
        "batch_count": 1,
        "ref_images": refs,
        "sample_params": {
            "sample_method": s.sampler,
            "sample_steps": s.steps,
            "guidance": { "txt_cfg": s.cfg_scale },
        },
        "preview": if previews { "proj" } else { "none" },
        "preview_interval": 1,
        "output_format": "png",
    }))
}

fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

#[async_trait]
impl MediaEngine for SdCppEngine {
    fn id(&self) -> &'static str {
        ENGINE_ID
    }

    fn status(&self) -> EngineStatus {
        match binary() {
            Some(_) => EngineStatus::Ready(PINNED_VERSION.to_string()),
            None => EngineStatus::NotInstalled(
                "the stable-diffusion.cpp runtime downloads with `oxen-harness media pull`".into(),
            ),
        }
    }

    async fn render(
        &self,
        req: EngineRequest<'_>,
        progress: &(dyn Fn(LocalProgress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<LocalOutput, LocalMediaError> {
        let mut slot = tokio::select! {
            slot = self.server.lock() => slot,
            _ = cancel.cancelled() => return Err(LocalMediaError::Cancelled),
        };
        // Holding the slot keeps the idle timer off this job.
        self.jobs.fetch_add(1, Ordering::SeqCst);
        let warm = slot.as_ref().is_some_and(|s| s.model == req.spec.id);
        if !warm {
            progress(LocalProgress::Loading);
        }
        let started = Instant::now();
        let result = self.run_job(&mut slot, &req, progress, cancel).await;
        drop(slot);
        self.schedule_unload();
        let (bytes, job) = result?;
        let s = req.settings;
        Ok(LocalOutput {
            bytes,
            seed: Some(Value::from(s.seed)),
            record: json!({
                "engine": ENGINE_ID,
                "engine_version": PINNED_VERSION,
                "model": req.spec.id,
                "license": req.spec.license,
                "weights": req.spec.files.iter()
                    .map(|(role, f)| (role.clone(), json!(format!("{}/{}", f.repo, f.path))))
                    .collect::<serde_json::Map<_, _>>(),
                "width": s.width,
                "height": s.height,
                "steps": s.steps,
                "cfg_scale": s.cfg_scale,
                "sampler": s.sampler,
                "seed": s.seed,
                "references": req.refs.len(),
                "warm": warm,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "job": job,
            }),
        })
    }

    async fn unload(&self) {
        self.jobs.fetch_add(1, Ordering::SeqCst);
        if let Some(s) = self.server.lock().await.take() {
            s.stop().await;
        }
    }
}

impl SdCppEngine {
    /// Submit and follow one job on the (started) server. Returns the PNG
    /// bytes and the server's job id.
    async fn run_job(
        &self,
        slot: &mut Option<Server>,
        req: &EngineRequest<'_>,
        progress: &(dyn Fn(LocalProgress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<(Vec<u8>, String), LocalMediaError> {
        let server = self.ensure_server(slot, req, progress, cancel).await?;
        let base = server.base.clone();
        let body = job_body(req, server.previews)?;
        let res = self
            .http
            .post(format!("{base}/sdcpp/v1/img_gen"))
            .json(&body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| LocalMediaError::Failed(format!("submitting to sd-server: {e}")))?;
        let status = res.status();
        let submitted: Value = res
            .json()
            .await
            .map_err(|e| LocalMediaError::Failed(format!("sd-server reply: {e}")))?;
        if !status.is_success() {
            return Err(LocalMediaError::Failed(format!(
                "sd-server rejected the job ({status}): {}",
                error_text(&submitted)
            )));
        }
        let id = submitted
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| LocalMediaError::Failed("sd-server returned no job id".into()))?
            .to_string();
        // Until the server settles the job, any exit — cancel, timeout, an
        // error, or this future being dropped with an interrupted turn —
        // tells the server to stop rendering it.
        let mut guard = AbandonGuard {
            http: self.http.clone(),
            url: format!("{base}/sdcpp/v1/jobs/{id}/cancel"),
            armed: true,
        };
        let started = Instant::now();
        let mut last_step = None;
        let mut unanswered_since: Option<Instant> = None;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Err(LocalMediaError::Cancelled),
                _ = tokio::time::sleep(POLL) => {}
            }
            if slot.as_mut().is_some_and(Server::exited) {
                // A dead server has nothing to cancel.
                guard.armed = false;
                let tail = self.log_tail();
                let Some(dead) = slot.take() else { continue };
                let err = dead.death("stopped while generating", tail);
                dead.stop().await;
                return Err(err);
            }
            let job = match self.get(&base, &format!("/sdcpp/v1/jobs/{id}")).await {
                Ok(job) => {
                    unanswered_since = None;
                    job
                }
                // A busy server can miss a poll; one that stays silent is
                // wedged, and waiting out the job timeout would hide that.
                Err(e) => {
                    let since = *unanswered_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > UNANSWERED_LIMIT {
                        return Err(LocalMediaError::Failed(format!(
                            "sd-server stopped answering ({e}){}",
                            self.log_tail()
                        )));
                    }
                    continue;
                }
            };
            let settled = matches!(
                job.get("status").and_then(Value::as_str),
                Some("completed" | "failed" | "cancelled")
            );
            if settled {
                guard.armed = false;
            }
            match job.get("status").and_then(Value::as_str).unwrap_or("") {
                "completed" => {
                    let b64 = job
                        .pointer("/result/images/0/b64_json")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            LocalMediaError::Failed("sd-server finished without an image".into())
                        })?;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| {
                            LocalMediaError::Failed(format!("sd-server image is not base64: {e}"))
                        })?;
                    return Ok((bytes, id));
                }
                "failed" => {
                    return Err(LocalMediaError::Failed(format!(
                        "sd-server: {}",
                        error_text(job.get("error").unwrap_or(&Value::Null))
                    )))
                }
                "cancelled" => return Err(LocalMediaError::Cancelled),
                _ => {}
            }
            let step = job
                .pointer("/preview/step")
                .and_then(Value::as_u64)
                .zip(job.pointer("/preview/total_steps").and_then(Value::as_u64));
            if let Some((step, total)) = step {
                if last_step != Some(step) {
                    last_step = Some(step);
                    progress(if step >= total {
                        LocalProgress::Finishing
                    } else {
                        LocalProgress::Step {
                            step: step as u32,
                            total: total as u32,
                        }
                    });
                }
            }
            if started.elapsed() > JOB_TIMEOUT {
                return Err(LocalMediaError::Failed(format!(
                    "gave up after {} minutes",
                    JOB_TIMEOUT.as_secs() / 60
                )));
            }
        }
    }
}

/// Cancels a submitted job on the server unless disarmed — the one place a
/// job that won't be followed to the end is stopped.
struct AbandonGuard {
    http: reqwest::Client,
    url: String,
    armed: bool,
}

impl Drop for AbandonGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Drop can't await; the cancel rides the runtime the job ran on.
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("no runtime to cancel {}; the server finishes it", self.url);
            return;
        };
        let request = self.http.post(&self.url).timeout(Duration::from_secs(5));
        let url = self.url.clone();
        rt.spawn(async move {
            // A server that went away (stopped, crashed) can't be told;
            // nothing is rendering there anymore.
            if let Err(e) = request.send().await {
                tracing::debug!("could not cancel sd-server job ({url}): {e}");
            }
        });
    }
}

/// The message out of an sd-server error object (or the whole value).
fn error_text(v: &Value) -> String {
    v.get("message")
        .or_else(|| v.pointer("/error/message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string())
}

/// Reap servers a dead host left running (call once at boot).
pub fn reap_stale() -> Vec<u32> {
    harness_local::reap_stale(PID_FILE, "sd-server")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{load_from, ModelSpec, Settings};
    use std::collections::BTreeMap;

    fn klein() -> ModelSpec {
        load_from(None)
            .expect("catalog")
            .into_iter()
            .find(|m| m.id == "local/flux-2-klein-4b")
            .expect("klein")
    }

    fn weights(spec: &ModelSpec) -> BTreeMap<String, PathBuf> {
        spec.files
            .keys()
            .map(|role| (role.clone(), PathBuf::from(format!("/w/{role}"))))
            .collect()
    }

    fn settings() -> Settings {
        Settings {
            width: 768,
            height: 512,
            steps: 4,
            cfg_scale: 1.0,
            sampler: "euler".into(),
            seed: 11,
            negative_prompt: None,
        }
    }

    #[test]
    fn weights_map_to_flags_and_memory_mapping_is_always_on() {
        let spec = klein();
        let args = server_args(&spec, &weights(&spec)).expect("args");
        let joined = args.join(" ");
        assert!(
            joined.contains("--diffusion-model /w/diffusion_model"),
            "{joined}"
        );
        assert!(joined.contains("--vae /w/vae"), "{joined}");
        assert!(joined.contains("--llm /w/llm"), "{joined}");
        assert!(args.contains(&"--mmap".to_string()));
        let mut odd = weights(&spec);
        odd.insert("unet".into(), PathBuf::from("/w/unet"));
        assert!(
            server_args(&spec, &odd).is_err(),
            "unknown roles never half-load"
        );
    }

    #[test]
    fn the_job_body_carries_settings_and_references() {
        let spec = klein();
        let w = weights(&spec);
        let s = settings();
        let dir = tempfile::tempdir().expect("tempdir");
        let r = dir.path().join("ref.png");
        std::fs::write(&r, b"img").expect("write");
        let refs = [r];
        let body = job_body(
            &EngineRequest {
                spec: &spec,
                weights: &w,
                prompt: "make it night",
                settings: &s,
                refs: &refs,
            },
            true,
        )
        .expect("body");
        assert_eq!(body["width"], 768);
        assert_eq!(body["seed"], 11);
        assert_eq!(body["sample_params"]["sample_steps"], 4);
        assert_eq!(body["ref_images"][0], "aW1n");
        assert_eq!(body["preview"], "proj");
    }

    /// A server already "running" against a mock: the job loop submits,
    /// reports steps, and decodes the image.
    #[tokio::test]
    async fn a_job_reports_steps_and_returns_the_image() {
        let mut mock = mockito::Server::new_async().await;
        let submit = mock
            .mock("POST", "/sdcpp/v1/img_gen")
            .with_status(202)
            .with_body(r#"{"id":"job_1","status":"queued"}"#)
            .create_async()
            .await;
        let generating = mock
            .mock("GET", "/sdcpp/v1/jobs/job_1")
            .with_body(
                r#"{"id":"job_1","status":"generating","preview":{"step":2,"total_steps":4}}"#,
            )
            .expect(1)
            .create_async()
            .await;
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG data");
        let done = mock
            .mock("GET", "/sdcpp/v1/jobs/job_1")
            .with_body(format!(
                r#"{{"id":"job_1","status":"completed","result":{{"images":[{{"index":0,"b64_json":"{png}"}}]}}}}"#
            ))
            .create_async()
            .await;

        let spec = klein();
        let w = weights(&spec);
        let s = settings();
        let engine = SdCppEngine::new(Duration::from_secs(60));
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .expect("stand-in child");
        let mut slot = Some(Server {
            pid: child.id().unwrap_or(0),
            child,
            base: mock.url(),
            model: spec.id.clone(),
            previews: true,
            watchdog: None,
        });
        let heard = std::sync::Mutex::new(Vec::new());
        let req = EngineRequest {
            spec: &spec,
            weights: &w,
            prompt: "a fox",
            settings: &s,
            refs: &[],
        };
        let (bytes, id) = engine
            .run_job(
                &mut slot,
                &req,
                &|p| heard.lock().expect("heard").push(p),
                &CancellationToken::new(),
            )
            .await
            .expect("job completes");
        assert_eq!(bytes, b"\x89PNG data");
        assert_eq!(id, "job_1");
        assert_eq!(
            heard.lock().expect("heard").clone(),
            vec![LocalProgress::Step { step: 2, total: 4 }]
        );
        submit.assert_async().await;
        generating.assert_async().await;
        done.assert_async().await;
    }

    #[tokio::test]
    async fn cancel_tells_the_server_and_reports_cancelled() {
        let mut mock = mockito::Server::new_async().await;
        mock.mock("POST", "/sdcpp/v1/img_gen")
            .with_status(202)
            .with_body(r#"{"id":"job_2"}"#)
            .create_async()
            .await;
        mock.mock("GET", "/sdcpp/v1/jobs/job_2")
            .with_body(r#"{"id":"job_2","status":"generating"}"#)
            .create_async()
            .await;
        let cancel_call = mock
            .mock("POST", "/sdcpp/v1/jobs/job_2/cancel")
            .with_body("{}")
            .create_async()
            .await;
        let spec = klein();
        let w = weights(&spec);
        let s = settings();
        let engine = SdCppEngine::new(Duration::from_secs(60));
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .expect("stand-in child");
        let mut slot = Some(Server {
            pid: child.id().unwrap_or(0),
            child,
            base: mock.url(),
            model: spec.id.clone(),
            previews: false,
            watchdog: None,
        });
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(900)).await;
            trigger.cancel();
        });
        let req = EngineRequest {
            spec: &spec,
            weights: &w,
            prompt: "a fox",
            settings: &s,
            refs: &[],
        };
        let result = engine.run_job(&mut slot, &req, &|_| {}, &cancel).await;
        assert!(matches!(result, Err(LocalMediaError::Cancelled)));
        // The cancel request rides a spawned task; give it a moment.
        for _ in 0..50 {
            if cancel_call.matched_async().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        cancel_call.assert_async().await;
    }

    /// An interrupted turn drops the job future; the server must hear a
    /// cancel instead of rendering on for minutes.
    #[tokio::test]
    async fn dropping_a_job_cancels_it_on_the_server() {
        let mut mock = mockito::Server::new_async().await;
        mock.mock("POST", "/sdcpp/v1/img_gen")
            .with_status(202)
            .with_body(r#"{"id":"job_3"}"#)
            .create_async()
            .await;
        mock.mock("GET", "/sdcpp/v1/jobs/job_3")
            .with_body(r#"{"id":"job_3","status":"generating"}"#)
            .create_async()
            .await;
        let cancel_call = mock
            .mock("POST", "/sdcpp/v1/jobs/job_3/cancel")
            .with_body("{}")
            .create_async()
            .await;
        let spec = klein();
        let w = weights(&spec);
        let s = settings();
        let engine = SdCppEngine::new(Duration::from_secs(60));
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .expect("stand-in child");
        let mut slot = Some(Server {
            pid: child.id().unwrap_or(0),
            child,
            base: mock.url(),
            model: spec.id.clone(),
            previews: false,
            watchdog: None,
        });
        let req = EngineRequest {
            spec: &spec,
            weights: &w,
            prompt: "a fox",
            settings: &s,
            refs: &[],
        };
        let never = CancellationToken::new();
        let job = engine.run_job(&mut slot, &req, &|_| {}, &never);
        let timed_out = tokio::time::timeout(Duration::from_millis(900), job).await;
        assert!(timed_out.is_err(), "the job was still running when dropped");
        // The guard's cancel rides a spawned task; give it a moment.
        for _ in 0..50 {
            if cancel_call.matched_async().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        cancel_call.assert_async().await;
    }
}
