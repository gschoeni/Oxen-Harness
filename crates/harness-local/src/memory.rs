//! What local models cost the machine's memory, and the guard that stops a
//! model before it drags the user's computer into swap.
//!
//! Three pieces, all macOS-first and FFI-free (the same `sysctl`/`vm_stat`
//! probes [`crate::hardware`] uses):
//!
//! - [`snapshot`] — system memory as Activity Monitor counts it (used =
//!   app memory + wired + compressed), swap, the kernel's pressure level.
//! - [`assess`] — before a load: go, warn ("this will push you into swap"),
//!   or refuse (the OS is already at critical pressure).
//! - [`Watchdog`] — while a model server loads and runs: samples memory and
//!   stops the server when pressure turns critical, free memory runs out,
//!   or swap grows fast — so the model dies, not the user's session.
//!
//! [`local_processes`] lists this host's model servers (from the spawned-
//! server registries) with their footprints, for the memory meters.
//! [`memory_hogs`] names the user's own apps holding the most memory, so a
//! notice says *what* to close — and [`quit_app`] closes one on request.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

use crate::hardware::MemoryPressure;

const GIB: u64 = 1024 * 1024 * 1024;

/// System memory right now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MemorySnapshot {
    pub total_bytes: u64,
    /// App memory + wired + compressed — what Activity Monitor calls
    /// "Memory Used". File cache is not counted; the OS drops it on demand.
    pub used_bytes: u64,
    /// `total - used`: what can be had without swapping.
    pub available_bytes: u64,
    /// Bytes the compressor holds (a squeezed machine compresses first).
    pub compressed_bytes: u64,
    pub swap_used_bytes: u64,
    /// The kernel's "memory free" percentage (`kern.memorystatus_level`).
    pub free_percent: u8,
    pub pressure: MemoryPressure,
}

impl MemorySnapshot {
    /// `used / total` as a fraction.
    pub fn used_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        self.used_bytes as f64 / self.total_bytes as f64
    }
}

/// Read system memory, or `None` where the platform gives no reading.
pub fn snapshot() -> Option<MemorySnapshot> {
    #[cfg(target_os = "macos")]
    {
        macos::snapshot()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// A process's memory footprint (what counts toward pressure: anonymous
/// and compressed memory, not memory-mapped files it could drop), falling
/// back to its resident size.
pub fn footprint(pid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        macos::footprint(pid).or_else(|| rss(pid))
    }
    #[cfg(not(target_os = "macos"))]
    {
        rss(pid)
    }
}

fn rss(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let kb: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kb * 1024)
}

/// One app's share of memory, summed over its processes (a browser's
/// renderers, an editor's helpers), for "close this to make room" advice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppMemory {
    pub name: String,
    /// Resident memory across the app's processes.
    pub bytes: u64,
    /// A macOS app bundle, quit like ⌘Q (it can still ask to save); else a
    /// bare process, stopped with SIGTERM.
    pub bundle: bool,
}

/// An app as [`rank_hogs`] found it, with the processes to signal.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RankedApp {
    app: AppMemory,
    pids: Vec<u32>,
}

/// Apps below this aren't worth naming: closing them frees little.
const HOG_MIN_BYTES: u64 = 512 * 1024 * 1024;

/// How many apps a memory message names.
const HOGS_NAMED: usize = 3;

/// The user's apps holding the most memory, biggest first, at most
/// `limit`. One `ps` call (~30ms); empty where it can't be read.
pub fn memory_hogs(limit: usize) -> Vec<AppMemory> {
    ranked_apps(HOG_MIN_BYTES)
        .into_iter()
        .take(limit)
        .map(|r| r.app)
        .collect()
}

fn ranked_apps(min_bytes: u64) -> Vec<RankedApp> {
    // A failed probe only drops the specifics from advice that reads fine
    // without them, so it is logged rather than surfaced.
    match std::process::Command::new("ps")
        .args(["-axo", "pid=,rss=,comm="])
        .output()
    {
        Ok(out) => rank_hogs(&String::from_utf8_lossy(&out.stdout), min_bytes),
        Err(e) => {
            tracing::warn!("couldn't list processes for memory advice: {e}");
            Vec::new()
        }
    }
}

/// Group `ps -o pid=,rss=,comm=` lines by app, biggest first. A process
/// inside an `.app` bundle counts toward the outermost bundle ("Google
/// Chrome Helper (Renderer)" is Google Chrome); a bare executable by its
/// file name. macOS itself (`/System`, `/usr/libexec`, …) and this
/// harness's own processes are left out: neither is the user's to close.
fn rank_hogs(ps: &str, min_bytes: u64) -> Vec<RankedApp> {
    const SYSTEM: [&str; 4] = ["/System/", "/usr/libexec/", "/usr/sbin/", "/sbin/"];
    const OURS: [&str; 4] = [
        "oxen-harness",
        "oxen-harness-app",
        "llama-server",
        "sd-server",
    ];
    let mut apps: Vec<RankedApp> = Vec::new();
    for line in ps.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(kb)) = (fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(pid), Ok(kb)) = (pid.parse::<u32>(), kb.parse::<u64>()) else {
            continue;
        };
        let command = fields.collect::<Vec<_>>().join(" ");
        if command.is_empty() || SYSTEM.iter().any(|p| command.starts_with(p)) {
            continue;
        }
        let (name, bundle) = match command.find(".app/") {
            Some(end) => (command[..end].rsplit('/').next().unwrap_or(""), true),
            // `ps` can append arguments to a bare name (`claude bg-spare`).
            None => {
                let program = command.split(' ').next().unwrap_or("");
                (program.rsplit('/').next().unwrap_or(""), false)
            }
        };
        if name.is_empty() || OURS.contains(&name) {
            continue;
        }
        match apps.iter_mut().find(|r| r.app.name == name) {
            Some(r) => {
                r.app.bytes += kb * 1024;
                r.pids.push(pid);
            }
            None => apps.push(RankedApp {
                app: AppMemory {
                    name: name.to_string(),
                    bytes: kb * 1024,
                    bundle,
                },
                pids: vec![pid],
            }),
        }
    }
    apps.retain(|r| r.app.bytes >= min_bytes);
    apps.sort_by_key(|a| std::cmp::Reverse(a.app.bytes));
    apps
}

/// What to close to make room: the named apps when there are any, else a
/// plain suggestion.
fn close_advice(hogs: &[AppMemory]) -> String {
    let gb = |b: u64| b as f64 / GIB as f64;
    match hogs {
        [] => "Closing other apps frees memory.".to_string(),
        [only] => format!(
            "Quitting {} ({:.1} GB) frees the most.",
            only.name,
            gb(only.bytes)
        ),
        [rest @ .., last] => {
            let named: Vec<String> = rest
                .iter()
                .map(|a| format!("{} ({:.1} GB)", a.name, gb(a.bytes)))
                .collect();
            format!(
                "Using the most: {} and {} ({:.1} GB).",
                named.join(", "),
                last.name,
                gb(last.bytes)
            )
        }
    }
}

/// The "what to close" sentence for right now, for a host's own report.
pub fn close_advice_now() -> String {
    close_advice(&memory_hogs(HOGS_NAMED))
}

/// Why [`quit_app`] couldn't close an app.
#[derive(Debug, thiserror::Error)]
pub enum QuitError {
    #[error("{0} isn't running (or isn't one of your apps)")]
    NotRunning(String),
    #[error("{name} didn't quit: {detail}")]
    Refused { name: String, detail: String },
    #[error("couldn't ask {name} to quit: {source}")]
    Io {
        name: String,
        #[source]
        source: std::io::Error,
    },
}

/// How long a bundled app gets to quit before [`quit_app`] reports back
/// (it may be waiting on the user to save something).
const QUIT_WAIT: Duration = Duration::from_secs(10);

/// Close one of the user's apps to free memory, on their say-so. The app is
/// looked up afresh by name — never by a pid that may have been reused since
/// it was shown — and must be one [`memory_hogs`] would name (never macOS or
/// this harness). A bundled app is asked to quit like ⌘Q, so it can still
/// prompt to save; a bare process gets SIGTERM. Returns the memory it held.
pub fn quit_app(name: &str) -> Result<u64, QuitError> {
    let Some(target) = ranked_apps(0).into_iter().find(|r| r.app.name == name) else {
        return Err(QuitError::NotRunning(name.to_string()));
    };
    let io = |source| QuitError::Io {
        name: name.to_string(),
        source,
    };
    if target.app.bundle {
        let script = format!(
            "tell application \"{}\" to quit",
            name.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let mut child = std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(io)?;
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().map_err(io)? {
                if status.success() {
                    break;
                }
                let mut detail = String::new();
                if let Some(mut err) = child.stderr.take() {
                    std::io::Read::read_to_string(&mut err, &mut detail).map_err(io)?;
                }
                return Err(QuitError::Refused {
                    name: name.to_string(),
                    detail: detail.trim().to_string(),
                });
            }
            if started.elapsed() > QUIT_WAIT {
                return Err(QuitError::Refused {
                    name: name.to_string(),
                    detail: "it may be waiting for you to save something".to_string(),
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    } else {
        let pids: Vec<String> = target.pids.iter().map(u32::to_string).collect();
        let out = std::process::Command::new("kill")
            .arg("-TERM")
            .args(&pids)
            .output()
            .map_err(io)?;
        if !out.status.success() {
            return Err(QuitError::Refused {
                name: name.to_string(),
                detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
    }
    Ok(target.app.bytes)
}

/// What to do about loading a model that needs `need_bytes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", content = "message", rename_all = "lowercase")]
pub enum LoadAdvice {
    Go,
    /// Load, but tell the user why their machine may slow down.
    Warn(String),
    /// Don't load: it would push an already-struggling machine over.
    Refuse(String),
}

/// How a model holds its memory, which decides how hard a shortfall bites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Residency {
    /// Weights copied into memory (llama-server): a shortfall is swapping.
    Resident,
    /// Weights memory-mapped from disk (sd-server): a shortfall re-reads
    /// pages and runs slower, but only the working buffers are new memory.
    Mapped,
}

/// The pure decision behind a pre-load check. `hogs` names what to close
/// when the advice is to make room.
pub fn assess(
    snap: &MemorySnapshot,
    label: &str,
    need_bytes: u64,
    residency: Residency,
    hogs: &[AppMemory],
) -> LoadAdvice {
    let gb = |b: u64| b as f64 / GIB as f64;
    if snap.pressure == MemoryPressure::Critical {
        return LoadAdvice::Refuse(format!(
            "macOS is at critical memory pressure ({:.1} of {:.1} GB used, {:.1} GB in swap); \
             loading {label} would freeze apps. {} Then try again.",
            gb(snap.used_bytes),
            gb(snap.total_bytes),
            gb(snap.swap_used_bytes),
            close_advice(hogs)
        ));
    }
    // A resident model bigger than most of the machine can never fit, apps
    // closed or not.
    if residency == Residency::Resident && need_bytes > snap.total_bytes / 100 * 85 {
        return LoadAdvice::Refuse(format!(
            "{label} needs about {:.1} GB and this machine has {:.1} GB in all; pick a smaller \
             model.",
            gb(need_bytes),
            gb(snap.total_bytes)
        ));
    }
    // Bigger than what's free: macOS will compress or swap other apps to
    // make room. That can work (idle apps page out), so it is the user's
    // call — said plainly — and the watchdog stops the model if it bites.
    if residency == Residency::Resident && need_bytes > snap.available_bytes {
        return LoadAdvice::Warn(format!(
            "memory is tight: {label} needs about {:.1} GB and {:.1} GB is free, so macOS will \
             push about {:.1} GB of your other apps into compressed memory or swap. {} It is \
             stopped automatically before your computer bogs down.",
            gb(need_bytes),
            gb(snap.available_bytes),
            gb(need_bytes - snap.available_bytes),
            close_advice(hogs)
        ));
    }
    if need_bytes > snap.available_bytes {
        return LoadAdvice::Warn(format!(
            "memory is tight: {label} wants about {:.1} GB and {:.1} GB is free, so macOS will \
             compress or swap and things may slow down. It is stopped automatically if memory \
             runs out.",
            gb(need_bytes),
            gb(snap.available_bytes)
        ));
    }
    if snap.pressure == MemoryPressure::Warn {
        return LoadAdvice::Warn(format!(
            "macOS already reports memory pressure ({:.1} of {:.1} GB used); {label} may run \
             slowly. It is stopped automatically if memory runs out.",
            gb(snap.used_bytes),
            gb(snap.total_bytes)
        ));
    }
    LoadAdvice::Go
}

/// Check the machine before loading `label`. No reading (other platforms)
/// means no opinion. A warning or refusal is also published as a
/// [`MemoryNotice`] so hosts can show it however the load was started.
pub fn preflight(label: &str, need_bytes: u64, residency: Residency) -> LoadAdvice {
    let hogs = memory_hogs(HOGS_NAMED);
    let advice = match snapshot() {
        Some(snap) => assess(&snap, label, need_bytes, residency, &hogs),
        None => LoadAdvice::Go,
    };
    match &advice {
        LoadAdvice::Go => {}
        LoadAdvice::Warn(m) => publish(NoticeLevel::Warn, label, m, hogs),
        LoadAdvice::Refuse(m) => publish(NoticeLevel::Refused, label, m, hogs),
    }
    advice
}

/// How serious a memory notice is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeLevel {
    /// A model loaded, but the machine is tight.
    Warn,
    /// A model was not loaded.
    Refused,
    /// A running model was stopped by the watchdog.
    Stopped,
}

/// Something a user should hear about local models and memory.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MemoryNotice {
    pub level: NoticeLevel,
    pub model: String,
    pub message: String,
    /// Unix seconds.
    pub at: i64,
    /// The apps holding the most memory when it was raised: what a host can
    /// offer to quit ([`quit_app`]).
    pub hogs: Vec<AppMemory>,
}

fn notices() -> &'static tokio::sync::broadcast::Sender<MemoryNotice> {
    static TX: OnceLock<tokio::sync::broadcast::Sender<MemoryNotice>> = OnceLock::new();
    TX.get_or_init(|| tokio::sync::broadcast::channel(16).0)
}

fn last_slot() -> &'static Mutex<Option<MemoryNotice>> {
    static LAST: OnceLock<Mutex<Option<MemoryNotice>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

fn publish(level: NoticeLevel, model: &str, message: &str, hogs: Vec<AppMemory>) {
    let notice = MemoryNotice {
        level,
        model: model.to_string(),
        message: message.to_string(),
        hogs,
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default(),
    };
    match last_slot().lock() {
        Ok(mut last) => *last = Some(notice.clone()),
        Err(_) => tracing::warn!("memory notice slot poisoned; the meter misses: {message}"),
    }
    // No subscribers is normal (a host that shows notices another way).
    let _ = notices().send(notice);
}

/// Hear every memory notice from here on (each host forwards them to its
/// user: the desktop as an event, the CLI as a printed line).
pub fn subscribe() -> tokio::sync::broadcast::Receiver<MemoryNotice> {
    notices().subscribe()
}

/// The most recent notice this process raised, for polling meters.
pub fn last_notice() -> Option<MemoryNotice> {
    last_slot().lock().ok().and_then(|n| n.clone())
}

/// When the watchdog stops a server.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Stop below this kernel "memory free" percentage. Kept low: the
    /// kernel counts a model's memory-mapped weights as in use although it
    /// can drop them, so loading an image model alone moves this a lot.
    pub min_free_percent: u8,
    /// Stop when swap has grown this much since the server started.
    pub max_swap_growth: u64,
    /// Stop when the kernel reports memory pressure *and* swap has grown
    /// this much: the machine is paging the user's apps out to keep up.
    pub warn_swap_growth: u64,
    /// Consecutive bad samples before stopping (one blip is not a trend).
    pub strikes: u32,
}

impl Default for Limits {
    /// The shipped limits, adjustable per machine with
    /// `OXEN_HARNESS_MEMORY_MIN_FREE` (percent) and
    /// `OXEN_HARNESS_MEMORY_MAX_SWAP_GB`. A value that doesn't parse is
    /// ignored, never an error: the guard must always be on.
    fn default() -> Self {
        let env = |key: &str| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
        };
        Self {
            min_free_percent: env("OXEN_HARNESS_MEMORY_MIN_FREE")
                .map(|p| p.clamp(0.0, 101.0) as u8)
                .unwrap_or(5),
            // Loading a model that is bigger than what's free pushes a few
            // GB of idle apps out by design (the pre-load warning says so);
            // growth well past that is the machine starting to thrash.
            max_swap_growth: env("OXEN_HARNESS_MEMORY_MAX_SWAP_GB")
                .map(|g| (g.max(0.0) * GIB as f64) as u64)
                .unwrap_or(6 * GIB),
            warn_swap_growth: 2 * GIB,
            strikes: 2,
        }
    }
}

/// Why a sample is bad, if it is.
pub fn breach(snap: &MemorySnapshot, swap_at_start: u64, limits: &Limits) -> Option<String> {
    let gb = |b: u64| b as f64 / GIB as f64;
    if snap.pressure == MemoryPressure::Critical {
        return Some(format!(
            "macOS hit critical memory pressure ({:.1} of {:.1} GB used)",
            gb(snap.used_bytes),
            gb(snap.total_bytes)
        ));
    }
    if snap.free_percent < limits.min_free_percent {
        return Some(format!(
            "free memory fell to {}% ({:.1} of {:.1} GB used)",
            snap.free_percent,
            gb(snap.used_bytes),
            gb(snap.total_bytes)
        ));
    }
    let grown = snap.swap_used_bytes.saturating_sub(swap_at_start);
    if grown > limits.max_swap_growth {
        return Some(format!(
            "it pushed {:.1} GB of your apps into swap",
            gb(grown)
        ));
    }
    if snap.pressure == MemoryPressure::Warn && grown > limits.warn_swap_growth {
        return Some(format!(
            "macOS is under memory pressure and has pushed {:.1} GB of your apps into swap",
            gb(grown)
        ));
    }
    None
}

/// The live watchdogs by start order, so a squeeze stops the newest model
/// (the load that tipped the machine over) rather than all of them.
fn watchers() -> &'static Mutex<Vec<u64>> {
    static LIVE: OnceLock<Mutex<Vec<u64>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(Vec::new()))
}

/// When any watchdog last stopped a model; the others give memory a moment
/// to recover before judging it.
fn last_stop() -> &'static Mutex<Option<std::time::Instant>> {
    static LAST: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

/// Swap in use when the most recent model started loading (or was last
/// stopped). Growth is measured from here, so it is charged to the load
/// that caused it rather than to every model that happens to be running.
fn swap_baseline() -> &'static Mutex<Option<u64>> {
    static BASE: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
    BASE.get_or_init(|| Mutex::new(None))
}

fn reset_swap_baseline(swap: u64) {
    if let Ok(mut b) = swap_baseline().lock() {
        *b = Some(swap);
    }
}

/// Extra samples an older model waits for the newest one to be stopped.
const DEFER_STRIKES: u32 = 6;
/// How long after any stop the remaining watchdogs hold off.
const SETTLE: Duration = Duration::from_secs(5);

/// Whether `id` should act on a breach now: the newest live watchdog acts
/// at once; an older one only once the newest had its chance and memory is
/// still bad. Nobody acts while memory settles after a stop.
fn may_stop(id: u64, strikes: u32, limits: &Limits) -> bool {
    // Poisoned coordination state only means a watchdog panicked mid-update;
    // falling back to acting alone keeps the machine protected.
    let settling = match last_stop().lock() {
        Ok(l) => l.is_some_and(|at| at.elapsed() < SETTLE),
        Err(_) => {
            tracing::warn!("watchdog stop clock poisoned; acting without settling");
            false
        }
    };
    if settling {
        return false;
    }
    let newest = match watchers().lock() {
        Ok(w) => w.last().copied(),
        Err(_) => {
            tracing::warn!("watchdog list poisoned; acting without deferring");
            None
        }
    };
    match newest {
        Some(n) if n != id => strikes >= limits.strikes + DEFER_STRIKES,
        _ => strikes >= limits.strikes,
    }
}

/// One shared reading for every watchdog: a sample spawns a few probe
/// processes, so it is taken off the async runtime at most every
/// [`SAMPLE_EVERY`] however many models are being watched.
async fn sampled() -> Option<MemorySnapshot> {
    static CACHE: OnceLock<tokio::sync::Mutex<Option<(std::time::Instant, MemorySnapshot)>>> =
        OnceLock::new();
    let mut slot = CACHE
        .get_or_init(|| tokio::sync::Mutex::new(None))
        .lock()
        .await;
    if let Some((at, snap)) = slot.as_ref() {
        if at.elapsed() < SAMPLE_EVERY {
            return Some(snap.clone());
        }
    }
    let fresh = match tokio::task::spawn_blocking(snapshot).await {
        Ok(s) => s?,
        Err(e) => {
            tracing::warn!("memory sample failed: {e}");
            return None;
        }
    };
    *slot = Some((std::time::Instant::now(), fresh.clone()));
    Some(fresh)
}

const SAMPLE_EVERY: Duration = Duration::from_millis(400);

/// Removes a watchdog from the live list however its task ends.
struct Live(u64);

impl Drop for Live {
    fn drop(&mut self) {
        if let Ok(mut w) = watchers().lock() {
            w.retain(|x| *x != self.0);
        }
    }
}

/// Watches memory while a model server runs and stops it — SIGTERM, then
/// SIGKILL — before the machine thrashes. Dropping it stops the watch.
pub struct Watchdog {
    task: tokio::task::JoinHandle<()>,
    tripped: Arc<Mutex<Option<String>>>,
}

impl Watchdog {
    /// Start watching `pid` (labelled `label` in the reason). `on_trip`
    /// hears the reason once, after the server has been signalled.
    pub fn spawn(
        pid: u32,
        label: &str,
        limits: Limits,
        on_trip: impl FnOnce(&str) + Send + 'static,
    ) -> Self {
        let tripped = Arc::new(Mutex::new(None));
        let record = tripped.clone();
        let label = label.to_string();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut w) = watchers().lock() {
            w.push(id);
        }
        // Built before the task so a watchdog dropped before its first poll
        // still leaves the live list (else it would count as newest forever).
        let live = Live(id);
        let task = tokio::spawn(async move {
            let _live = live;
            let Some(start) = sampled().await else { return };
            reset_swap_baseline(start.swap_used_bytes);
            let mut strikes = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if !crate::server::process_alive(pid) {
                    return;
                }
                let Some(snap) = sampled().await else {
                    continue;
                };
                let baseline = swap_baseline()
                    .lock()
                    .ok()
                    .and_then(|b| *b)
                    .unwrap_or(start.swap_used_bytes);
                match breach(&snap, baseline, &limits) {
                    Some(why) => {
                        strikes += 1;
                        if !may_stop(id, strikes, &limits) {
                            continue;
                        }
                        if let Ok(mut last) = last_stop().lock() {
                            *last = Some(std::time::Instant::now());
                        }
                        // What the survivors are judged by next starts now.
                        reset_swap_baseline(snap.swap_used_bytes);
                        let hogs = memory_hogs(HOGS_NAMED);
                        let reason = format!(
                            "Stopped {label} to protect your computer: {why}. {}",
                            close_advice(&hogs)
                        );
                        // Everything is reported before the signal: whoever
                        // sees the server die must be able to read why, and
                        // the owner dropping this watchdog on that death
                        // aborts this task mid-stop. (A poisoned slot only
                        // loses the stored reason; the notice still goes.)
                        if let Ok(mut slot) = record.lock() {
                            *slot = Some(reason.clone());
                        }
                        publish(NoticeLevel::Stopped, &label, &reason, hogs);
                        on_trip(&reason);
                        crate::server::stop_process(pid).await;
                        return;
                    }
                    None => strikes = 0,
                }
            }
        });
        Self { task, tripped }
    }

    /// Why the server was stopped, if it was.
    pub fn tripped(&self) -> Option<String> {
        self.tripped.lock().ok().and_then(|r| r.clone())
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One of this host's model servers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocalProcess {
    /// `chat` (llama-server) or `image` (sd-server).
    pub kind: &'static str,
    pub model: String,
    pub pid: u32,
    pub footprint_bytes: u64,
}

/// The model servers this process started that are still running, with
/// their current footprints.
pub fn local_processes() -> Vec<LocalProcess> {
    let mut out = Vec::new();
    for (file, kind) in [
        (crate::server::LLAMA_PIDS, "chat"),
        (crate::server::SD_PIDS, "image"),
    ] {
        for entry in crate::server::owned_entries(file) {
            if !crate::server::process_alive(entry.pid) {
                continue;
            }
            out.push(LocalProcess {
                kind,
                model: entry.name,
                pid: entry.pid,
                footprint_bytes: footprint(entry.pid).unwrap_or(0),
            });
        }
    }
    out
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    fn sysctl(key: &str) -> Option<String> {
        let out = std::process::Command::new("sysctl")
            .args(["-n", key])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub(super) fn snapshot() -> Option<MemorySnapshot> {
        let total: u64 = sysctl("hw.memsize")?.parse().ok()?;
        let free_percent: u8 = sysctl("kern.memorystatus_level")?.parse().ok()?;
        let pressure = crate::hardware::memory_pressure();
        let swap_used = sysctl("vm.swapusage")
            .and_then(|s| parse_swap_used(&s))
            .unwrap_or(0);
        let out = std::process::Command::new("vm_stat").output().ok()?;
        let vm = parse_vm_stat(&String::from_utf8_lossy(&out.stdout))?;
        Some(vm.snapshot(total, swap_used, free_percent, pressure))
    }

    pub(super) fn footprint(pid: u32) -> Option<u64> {
        let out = std::process::Command::new("footprint")
            .args(["-p", &pid.to_string()])
            .output()
            .ok()?;
        parse_footprint(&String::from_utf8_lossy(&out.stdout))
    }
}

// The `vm_stat` / `sysctl` / `footprint` parsers below only run on macOS,
// but stay compiled (and tested) everywhere: they're pure text parsing.

/// The page counts [`snapshot`] needs from `vm_stat`.
#[derive(Debug, Default, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct VmStat {
    page_size: u64,
    anonymous: u64,
    purgeable: u64,
    wired: u64,
    compressor: u64,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl VmStat {
    fn snapshot(
        &self,
        total: u64,
        swap_used: u64,
        free_percent: u8,
        pressure: MemoryPressure,
    ) -> MemorySnapshot {
        let pages = self.anonymous.saturating_sub(self.purgeable) + self.wired + self.compressor;
        let used = (pages * self.page_size).min(total);
        MemorySnapshot {
            total_bytes: total,
            used_bytes: used,
            available_bytes: total - used,
            compressed_bytes: self.compressor * self.page_size,
            swap_used_bytes: swap_used,
            free_percent,
            pressure,
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_vm_stat(text: &str) -> Option<VmStat> {
    let mut vm = VmStat::default();
    for line in text.lines() {
        if let Some(rest) = line.split("page size of ").nth(1) {
            vm.page_size = rest.split_whitespace().next()?.parse().ok()?;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let Ok(n) = value.trim().trim_end_matches('.').parse::<u64>() else {
            continue;
        };
        match key.trim() {
            "Anonymous pages" => vm.anonymous = n,
            "Pages purgeable" => vm.purgeable = n,
            "Pages wired down" => vm.wired = n,
            "Pages occupied by compressor" => vm.compressor = n,
            _ => {}
        }
    }
    (vm.page_size > 0).then_some(vm)
}

/// `total = 23552.00M  used = 22389.25M  free = …` → used bytes.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_swap_used(text: &str) -> Option<u64> {
    let used = text.split("used =").nth(1)?.split_whitespace().next()?;
    let (num, unit) = used.split_at(used.len().checked_sub(1)?);
    let n: f64 = num.parse().ok()?;
    let scale = match unit {
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => GIB as f64,
        _ => return None,
    };
    Some((n * scale) as u64)
}

/// `zsh [28785]: 64-bit    Footprint: 2192 KB (16384 bytes per page)` → bytes.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_footprint(text: &str) -> Option<u64> {
    let mut words = text.split_whitespace();
    words.find(|w| *w == "Footprint:")?;
    let n: f64 = words.next()?.parse().ok()?;
    let scale = match words.next()? {
        "B" => 1.0,
        "KB" => 1024.0,
        "MB" => 1024.0 * 1024.0,
        "GB" => GIB as f64,
        _ => return None,
    };
    Some((n * scale) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(used_gb: u64, pressure: MemoryPressure) -> MemorySnapshot {
        MemorySnapshot {
            total_bytes: 36 * GIB,
            used_bytes: used_gb * GIB,
            available_bytes: (36 - used_gb) * GIB,
            compressed_bytes: 4 * GIB,
            swap_used_bytes: 10 * GIB,
            free_percent: 40,
            pressure,
        }
    }

    #[test]
    fn parses_the_macos_probes() {
        let vm = parse_vm_stat(
            "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
             Pages free:                                    39608.\n\
             Pages wired down:                             414018.\n\
             Pages purgeable:                               20695.\n\
             Anonymous pages:                             1115996.\n\
             Pages occupied by compressor:                 325454.\n",
        )
        .expect("vm_stat parses");
        let s = vm.snapshot(38_654_705_664, 0, 67, MemoryPressure::Normal);
        // (1115996 - 20695 + 414018 + 325454) pages × 16 KiB.
        assert_eq!(s.used_bytes, 1_834_773 * 16_384);
        assert_eq!(s.available_bytes, 38_654_705_664 - s.used_bytes);
        assert_eq!(
            parse_swap_used("total = 23552.00M  used = 22389.25M  free = 1162.75M  (encrypted)"),
            Some((22389.25 * 1024.0 * 1024.0) as u64)
        );
        assert_eq!(
            parse_footprint("zsh [28785]: 64-bit    Footprint: 2192 KB (16384 bytes per page)"),
            Some(2192 * 1024)
        );
        assert_eq!(parse_footprint("no such process"), None);
    }

    #[test]
    fn hogs_group_by_app_and_skip_macos_and_our_own_processes() {
        let ps = "\
  101 1561440 /Applications/Docker.app/Contents/MacOS/com.docker.backend
  102  440560 claude
  103  270944 claude bg-spare
  104  307648 /Applications/Slack.app/Contents/Frameworks/Slack Helper (Renderer).app/Contents/MacOS/Slack Helper (Renderer)
  105  700000 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
  106  600000 /Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)
  107 9000000 /System/Library/PrivateFrameworks/SkyLight.framework/Resources/WindowServer
  108  900000 /usr/libexec/syspolicyd
  109 9000000 /Users/me/.oxen-harness/runtime/llama-server
garbage line
";
        let ranked = rank_hogs(ps, HOG_MIN_BYTES);
        let names: Vec<&str> = ranked.iter().map(|r| r.app.name.as_str()).collect();
        // Chrome's renderer counts toward Chrome; Slack (0.3 GB) is too
        // small to name; macOS and our own model server never show.
        assert_eq!(names, ["Docker", "Google Chrome", "claude"]);
        assert_eq!(ranked[1].app.bytes, 1_300_000 * 1024);
        assert_eq!(ranked[1].pids, [105, 106]);
        assert!(ranked[1].app.bundle);
        assert_eq!(ranked[2].app.bytes, (440_560 + 270_944) * 1024);
        assert_eq!(ranked[2].pids, [102, 103]);
        assert!(
            !ranked[2].app.bundle,
            "a bare process is signalled, not quit"
        );
    }

    #[test]
    fn close_advice_names_the_biggest_apps() {
        let app = |name: &str, gb: u64| AppMemory {
            name: name.into(),
            bytes: gb * GIB,
            bundle: true,
        };
        assert_eq!(close_advice(&[]), "Closing other apps frees memory.");
        assert_eq!(
            close_advice(&[app("Docker", 4)]),
            "Quitting Docker (4.0 GB) frees the most."
        );
        assert_eq!(
            close_advice(&[app("Docker", 4), app("Google Chrome", 3), app("Slack", 1)]),
            "Using the most: Docker (4.0 GB), Google Chrome (3.0 GB) and Slack (1.0 GB)."
        );
        // …and a tight-memory warning carries it.
        let warn = assess(
            &snap(30, MemoryPressure::Normal),
            "gemma",
            13 * GIB,
            Residency::Resident,
            &[app("Docker", 4)],
        );
        assert!(matches!(warn, LoadAdvice::Warn(m) if m.contains("Quitting Docker (4.0 GB)")));
    }

    #[test]
    fn quitting_an_app_that_is_not_running_says_so() {
        let err = quit_app("no-such-app-anywhere-0451").expect_err("not running");
        assert!(matches!(err, QuitError::NotRunning(_)), "{err}");
    }

    #[test]
    fn critical_pressure_refuses_any_load() {
        let advice = assess(
            &snap(20, MemoryPressure::Critical),
            "gemma",
            GIB,
            Residency::Mapped,
            &[],
        );
        assert!(matches!(advice, LoadAdvice::Refuse(m) if m.contains("critical")));
    }

    #[test]
    fn a_resident_model_that_cannot_fit_is_refused_and_a_tight_one_warned() {
        // 30 of 36 GB used: 6 GB free.
        let s = snap(30, MemoryPressure::Normal);
        assert!(matches!(
            assess(&s, "a 70B model", 34 * GIB, Residency::Resident, &[]),
            LoadAdvice::Refuse(m) if m.contains("a 70B model")
        ));
        // Bigger than what's free but able to fit: warned, with the cost.
        assert!(matches!(
            assess(&s, "gemma", 13 * GIB, Residency::Resident, &[]),
            LoadAdvice::Warn(m) if m.contains("7.0 GB of your other apps")
        ));
        // Mapped weights only warn: the OS pages them instead of swapping.
        assert!(matches!(
            assess(&s, "iris", 9 * GIB, Residency::Mapped, &[]),
            LoadAdvice::Warn(_)
        ));
        assert_eq!(
            assess(&s, "small", 2 * GIB, Residency::Resident, &[]),
            LoadAdvice::Go
        );
    }

    #[test]
    fn warn_pressure_warns_even_when_it_fits() {
        assert!(matches!(
            assess(&snap(10, MemoryPressure::Warn), "klein", GIB, Residency::Mapped, &[]),
            LoadAdvice::Warn(m) if m.contains("pressure")
        ));
    }

    #[test]
    fn breaches_are_critical_pressure_low_free_or_swap_growth() {
        let limits = Limits::default();
        let ok = snap(20, MemoryPressure::Normal);
        assert_eq!(breach(&ok, 10 * GIB, &limits), None);
        assert!(breach(&snap(20, MemoryPressure::Critical), 10 * GIB, &limits).is_some());
        let low = MemorySnapshot {
            free_percent: 4,
            ..ok.clone()
        };
        assert!(breach(&low, 10 * GIB, &limits)
            .unwrap_or_default()
            .contains("4%"));
        // Pressure alone is not a breach; pressure while apps page out is.
        let squeezed = MemorySnapshot {
            pressure: MemoryPressure::Warn,
            swap_used_bytes: 13 * GIB,
            ..ok.clone()
        };
        assert!(breach(&squeezed, 10 * GIB, &limits)
            .unwrap_or_default()
            .contains("under memory pressure"));
        let calm_warn = MemorySnapshot {
            pressure: MemoryPressure::Warn,
            swap_used_bytes: 11 * GIB,
            ..ok.clone()
        };
        assert_eq!(breach(&calm_warn, 10 * GIB, &limits), None);
        let swapping = MemorySnapshot {
            swap_used_bytes: 17 * GIB,
            ..ok
        };
        assert!(breach(&swapping, 10 * GIB, &limits)
            .unwrap_or_default()
            .contains("swap"));
    }

    #[test]
    fn the_newest_model_is_stopped_first_and_older_ones_wait() {
        let limits = Limits::default();
        // Ids far from the live counter so this can't collide with a real one.
        let (older, newer) = (900_001, 900_002);
        watchers().lock().expect("watchers").extend([older, newer]);
        assert!(
            may_stop(newer, limits.strikes, &limits),
            "the newest acts at once"
        );
        assert!(
            !may_stop(older, limits.strikes, &limits),
            "an older one waits"
        );
        assert!(
            may_stop(older, limits.strikes + DEFER_STRIKES, &limits),
            "…but not forever"
        );
        *last_stop().lock().expect("last") = Some(std::time::Instant::now());
        assert!(
            !may_stop(newer, 99, &limits),
            "nobody acts while memory settles"
        );
        *last_stop().lock().expect("last") = None;
        watchers()
            .lock()
            .expect("watchers")
            .retain(|x| *x != older && *x != newer);
    }

    #[tokio::test]
    async fn the_watchdog_stops_a_process_only_when_memory_runs_out() {
        // A tight limit that the real machine always breaches: free memory
        // can never be under 101%.
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .expect("child");
        let pid = child.id().expect("pid");
        if snapshot().is_none() {
            return; // no probes on this platform
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let dog = Watchdog::spawn(
            pid,
            "the test model",
            Limits {
                min_free_percent: 101,
                max_swap_growth: u64::MAX,
                warn_swap_growth: u64::MAX,
                strikes: 1,
            },
            move |why| {
                let _ = tx.send(why.to_string());
            },
        );
        let why = tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .expect("tripped in time")
            .expect("reason");
        assert!(why.contains("the test model"), "{why}");
        assert_eq!(dog.tripped().as_deref(), Some(why.as_str()));
        let last = last_notice().expect("published");
        assert_eq!(last.level, NoticeLevel::Stopped);
        assert_eq!(last.message, why);
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("child exits")
            .expect("wait");
        assert!(!status.success());
    }
}
