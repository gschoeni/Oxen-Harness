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

/// The pure decision behind a pre-load check.
pub fn assess(
    snap: &MemorySnapshot,
    label: &str,
    need_bytes: u64,
    residency: Residency,
) -> LoadAdvice {
    let gb = |b: u64| b as f64 / 1e9;
    if snap.pressure == MemoryPressure::Critical {
        return LoadAdvice::Refuse(format!(
            "macOS is at critical memory pressure ({:.1} of {:.1} GB used, {:.1} GB in swap); \
             loading {label} would freeze apps. Close something heavy and try again.",
            gb(snap.used_bytes),
            gb(snap.total_bytes),
            gb(snap.swap_used_bytes)
        ));
    }
    // A resident load bigger than everything not already in use cannot fit
    // without the OS swapping the user's apps out under it.
    if residency == Residency::Resident
        && need_bytes > snap.available_bytes + snap.compressed_bytes / 2
    {
        return LoadAdvice::Refuse(format!(
            "{label} needs about {:.1} GB and only {:.1} GB is free ({:.1} of {:.1} GB used). \
             Close some apps or pick a smaller model.",
            gb(need_bytes),
            gb(snap.available_bytes),
            gb(snap.used_bytes),
            gb(snap.total_bytes)
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
    let advice = match snapshot() {
        Some(snap) => assess(&snap, label, need_bytes, residency),
        None => LoadAdvice::Go,
    };
    match &advice {
        LoadAdvice::Go => {}
        LoadAdvice::Warn(m) => publish(NoticeLevel::Warn, label, m),
        LoadAdvice::Refuse(m) => publish(NoticeLevel::Refused, label, m),
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
}

fn notices() -> &'static tokio::sync::broadcast::Sender<MemoryNotice> {
    static TX: OnceLock<tokio::sync::broadcast::Sender<MemoryNotice>> = OnceLock::new();
    TX.get_or_init(|| tokio::sync::broadcast::channel(16).0)
}

fn last_slot() -> &'static Mutex<Option<MemoryNotice>> {
    static LAST: OnceLock<Mutex<Option<MemoryNotice>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

fn publish(level: NoticeLevel, model: &str, message: &str) {
    let notice = MemoryNotice {
        level,
        model: model.to_string(),
        message: message.to_string(),
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default(),
    };
    if let Ok(mut last) = last_slot().lock() {
        *last = Some(notice.clone());
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
    /// Stop below this kernel "memory free" percentage.
    pub min_free_percent: u8,
    /// Stop when swap has grown this much since the server started.
    pub max_swap_growth: u64,
    /// Consecutive bad samples before stopping (one blip is not a trend).
    pub strikes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            min_free_percent: 10,
            max_swap_growth: 3 * GIB,
            strikes: 2,
        }
    }
}

/// Why a sample is bad, if it is.
pub fn breach(snap: &MemorySnapshot, swap_at_start: u64, limits: &Limits) -> Option<String> {
    let gb = |b: u64| b as f64 / 1e9;
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
    None
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
        let task = tokio::spawn(async move {
            let Some(start) = snapshot() else { return };
            let mut strikes = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if !crate::server::process_alive(pid) {
                    return;
                }
                let Some(snap) = snapshot() else { continue };
                match breach(&snap, start.swap_used_bytes, &limits) {
                    Some(why) => {
                        strikes += 1;
                        if strikes < limits.strikes {
                            continue;
                        }
                        let reason = format!("Stopped {label} to protect your computer: {why}.");
                        crate::server::stop_process(pid).await;
                        // A poisoned slot only loses the reason; the stop
                        // itself already happened and on_trip still hears it.
                        if let Ok(mut slot) = record.lock() {
                            *slot = Some(reason.clone());
                        }
                        publish(NoticeLevel::Stopped, &label, &reason);
                        on_trip(&reason);
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

/// The page counts [`snapshot`] needs from `vm_stat`.
#[derive(Debug, Default, PartialEq)]
struct VmStat {
    page_size: u64,
    anonymous: u64,
    purgeable: u64,
    wired: u64,
    compressor: u64,
}

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
    fn critical_pressure_refuses_any_load() {
        let advice = assess(
            &snap(20, MemoryPressure::Critical),
            "gemma",
            GIB,
            Residency::Mapped,
        );
        assert!(matches!(advice, LoadAdvice::Refuse(m) if m.contains("critical")));
    }

    #[test]
    fn a_resident_model_that_cannot_fit_is_refused_and_a_tight_one_warned() {
        // 30 GB used: 6 GB free + 2 GB of compressible slack.
        let s = snap(30, MemoryPressure::Normal);
        assert!(matches!(
            assess(&s, "a 30B model", 17 * GIB, Residency::Resident),
            LoadAdvice::Refuse(m) if m.contains("a 30B model")
        ));
        assert!(matches!(
            assess(&s, "gemma", 7 * GIB, Residency::Resident),
            LoadAdvice::Warn(_)
        ));
        // Mapped weights only warn: the OS pages them instead of swapping.
        assert!(matches!(
            assess(&s, "iris", 9 * GIB, Residency::Mapped),
            LoadAdvice::Warn(_)
        ));
        assert_eq!(
            assess(&s, "small", 2 * GIB, Residency::Resident),
            LoadAdvice::Go
        );
    }

    #[test]
    fn warn_pressure_warns_even_when_it_fits() {
        assert!(matches!(
            assess(&snap(10, MemoryPressure::Warn), "klein", GIB, Residency::Mapped),
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
            free_percent: 6,
            ..ok.clone()
        };
        assert!(breach(&low, 10 * GIB, &limits)
            .unwrap_or_default()
            .contains("6%"));
        let swapping = MemorySnapshot {
            swap_used_bytes: 14 * GIB,
            ..ok
        };
        assert!(breach(&swapping, 10 * GIB, &limits)
            .unwrap_or_default()
            .contains("swap"));
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
