//! The `🧠` memory readout: what local models are costing the machine,
//! shown on the pinned meter (and under rendering generations) only while
//! this CLI has a local model server running, or just after a memory notice.
//!
//! A reading spawns a few probes (`sysctl`, `vm_stat`, `footprint`), so it
//! is cached and refreshed at most every [`SAMPLE_EVERY`]; the pinned area
//! repaints far more often than that.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use harness_local::memory::{self, LocalProcess, MemoryNotice, MemorySnapshot, NoticeLevel};
use harness_local::MemoryPressure;

use crate::theme::Ui;

const SAMPLE_EVERY: Duration = Duration::from_secs(2);

/// How long a warning or stop stays on the meter after it happened.
const NOTICE_FOR: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
struct Reading {
    at: Instant,
    system: Option<MemorySnapshot>,
    processes: Vec<LocalProcess>,
    notice: Option<MemoryNotice>,
}

fn cache() -> &'static Mutex<Option<Reading>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<Reading>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// The cached reading, refreshed when stale. Cheap when nothing local ever
/// ran: the registry files are read but no process is probed.
fn reading() -> Reading {
    let Ok(mut slot) = cache().lock() else {
        // Poisoned only if a sampler panicked; read fresh, uncached.
        return sample();
    };
    match slot.as_ref() {
        Some(r) if r.at.elapsed() < SAMPLE_EVERY => r.clone(),
        _ => {
            let fresh = sample();
            *slot = Some(fresh.clone());
            fresh
        }
    }
}

fn sample() -> Reading {
    let processes = memory::local_processes();
    let notice = memory::last_notice().filter(|n| age_secs(n.at) < NOTICE_FOR.as_secs() as i64);
    // System memory only matters while something local is (or was) running.
    let system = (!processes.is_empty() || notice.is_some())
        .then(memory::snapshot)
        .flatten();
    Reading {
        at: Instant::now(),
        system,
        processes,
        notice,
    }
}

/// The guard's most recent stop of a local model, if it was in the last
/// few minutes — what a failed turn should be explained by.
pub(crate) fn recent_stop() -> Option<MemoryNotice> {
    memory::last_notice().filter(|n| n.level == NoticeLevel::Stopped && age_secs(n.at) < 180)
}

fn age_secs(at: i64) -> i64 {
    harness_media::library::now_unix() - at
}

/// Whether the readout has anything to show (and so the idle prompt should
/// wake to refresh it).
pub(crate) fn active() -> bool {
    let r = reading();
    !r.processes.is_empty() || r.notice.is_some()
}

/// Memory in gigabytes as Apple counts them (2^30), so a 36 GB Mac reads 36.0.
fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1u64 << 30) as f64)
}

/// `🧠 30.1/36.0 GB · local 13.2 GB` (`plain`, `styled`), colored by the
/// kernel's pressure, plus `⚠ <notice>` while one is fresh. `None` when no
/// local model is running and nothing happened lately.
pub(crate) fn segment(ui: &Ui) -> Option<(String, String)> {
    let r = reading();
    let system = r.system.as_ref()?;
    let local: u64 = r.processes.iter().map(|p| p.footprint_bytes).sum();
    let mut plain = format!("🧠 {}/{} GB", gb(system.used_bytes), gb(system.total_bytes));
    if !r.processes.is_empty() {
        plain.push_str(&format!(" · local {} GB", gb(local)));
    }
    let mut styled = paint(ui, system.pressure, &plain);
    if system.pressure != MemoryPressure::Normal && system.pressure != MemoryPressure::Unknown {
        let label = format!(" · pressure {}", pressure_word(system.pressure));
        plain.push_str(&label);
        styled.push_str(&paint(ui, system.pressure, &label));
    }
    if let Some(n) = &r.notice {
        let text = format!(" · ⚠ {}", notice_short(n));
        plain.push_str(&text);
        styled.push_str(&match n.level {
            NoticeLevel::Warn => ui.brown(&text),
            NoticeLevel::Refused | NoticeLevel::Stopped => ui.red(&text),
        });
    }
    Some((plain, styled))
}

/// One line for the generations block: the image model's own footprint and
/// the machine, so a long render shows what it costs.
pub(crate) fn media_line(ui: &Ui) -> Option<String> {
    let r = reading();
    let system = r.system.as_ref()?;
    let image: u64 = r
        .processes
        .iter()
        .filter(|p| p.kind == "image")
        .map(|p| p.footprint_bytes)
        .sum();
    let text = format!(
        "    🧠 image model {} GB · machine {}/{} GB · pressure {}",
        gb(image),
        gb(system.used_bytes),
        gb(system.total_bytes),
        pressure_word(system.pressure)
    );
    Some(paint(ui, system.pressure, &text))
}

fn pressure_word(p: MemoryPressure) -> &'static str {
    match p {
        MemoryPressure::Normal => "normal",
        MemoryPressure::Warn => "high",
        MemoryPressure::Critical => "critical",
        MemoryPressure::Unknown => "unknown",
    }
}

fn paint(ui: &Ui, p: MemoryPressure, text: &str) -> String {
    match p {
        MemoryPressure::Critical => ui.red(text),
        MemoryPressure::Warn => ui.brown(text),
        MemoryPressure::Normal | MemoryPressure::Unknown => ui.dim(text),
    }
}

/// `stopped Iris-3B Turbo (memory)` / `memory tight for gemma`.
fn notice_short(n: &MemoryNotice) -> String {
    match n.level {
        NoticeLevel::Stopped => format!("stopped {} to free memory", n.model),
        NoticeLevel::Refused => format!("didn't load {}: not enough memory", n.model),
        NoticeLevel::Warn => format!("memory tight for {}", n.model),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_read_short() {
        let n = MemoryNotice {
            level: NoticeLevel::Stopped,
            model: "Iris-3B Turbo".into(),
            message: "long reason".into(),
            at: 0,
        };
        assert_eq!(notice_short(&n), "stopped Iris-3B Turbo to free memory");
    }

    #[test]
    fn nothing_shows_without_a_local_model_or_notice() {
        // This test process never started a model server, so unless another
        // test in the binary raised a notice the gauge stays quiet.
        let r = sample();
        if r.notice.is_none() {
            assert!(r.processes.is_empty());
            assert!(r.system.is_none());
        }
    }
}
