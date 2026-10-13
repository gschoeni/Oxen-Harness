//! On-device models for clients: the memory meter's reading, the local
//! image models and their downloads, and memory notices forwarded as
//! events. Free functions over process-wide state (the GPU, the weight
//! store, and the memory guard are machine resources, not per-session ones),
//! shared by the desktop and HTTP transports.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use harness_local::memory::{self, MemoryNotice, NoticeLevel};
use harness_protocol::{
    LocalMediaModel, LocalMediaStatus, LocalModelProcess, MemoryStatus, MemorySystem, ProtocolEvent,
};
use tokio_util::sync::CancellationToken;

use crate::EventSink;

/// System memory, this host's model servers, and the last memory notice.
pub fn memory_status() -> MemoryStatus {
    MemoryStatus {
        system: memory::snapshot().map(|s| MemorySystem {
            total_bytes: s.total_bytes,
            used_bytes: s.used_bytes,
            available_bytes: s.available_bytes,
            compressed_bytes: s.compressed_bytes,
            swap_used_bytes: s.swap_used_bytes,
            free_percent: s.free_percent,
            pressure: pressure_name(s.pressure).to_string(),
        }),
        processes: memory::local_processes()
            .into_iter()
            .map(|p| LocalModelProcess {
                kind: p.kind.to_string(),
                model: p.model,
                pid: p.pid,
                footprint_bytes: p.footprint_bytes,
            })
            .collect(),
        notice: memory::last_notice().map(notice),
    }
}

fn pressure_name(p: harness_local::MemoryPressure) -> &'static str {
    match p {
        harness_local::MemoryPressure::Normal => "normal",
        harness_local::MemoryPressure::Warn => "warn",
        harness_local::MemoryPressure::Critical => "critical",
        harness_local::MemoryPressure::Unknown => "unknown",
    }
}

fn level_name(level: NoticeLevel) -> &'static str {
    match level {
        NoticeLevel::Warn => "warn",
        NoticeLevel::Refused => "refused",
        NoticeLevel::Stopped => "stopped",
    }
}

fn notice(n: MemoryNotice) -> harness_protocol::MemoryNotice {
    harness_protocol::MemoryNotice {
        level: level_name(n.level).to_string(),
        model: n.model,
        message: n.message,
        at: n.at,
    }
}

/// Forward every memory notice (a load warned about or refused, a model
/// the watchdog stopped) to `sink` as `memory.notice`, for as long as the
/// process runs. Each host spawns this once at boot on its own runtime.
pub async fn forward_memory_notices(sink: Arc<dyn EventSink>) {
    let mut rx = memory::subscribe();
    loop {
        match rx.recv().await {
            Ok(n) => sink.emit(ProtocolEvent::MemoryNotice {
                level: level_name(n.level).to_string(),
                model: n.model,
                message: n.message,
                at: n.at,
            }),
            // A burst overflowed the buffer: the newest notices still come
            // through, which are the ones that matter.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// The on-device image models, their engine, and the disk they live on.
/// A host whose local media failed to load reports why in `error`.
pub fn local_media_status() -> LocalMediaStatus {
    let service = match harness_media_local::service() {
        Ok(s) => s,
        Err(e) => {
            return LocalMediaStatus {
                models: Vec::new(),
                engine_version: None,
                store_path: String::new(),
                disk_available_bytes: None,
                error: Some(e.to_string()),
            }
        }
    };
    let store = service.store();
    let engine_version = service
        .engine(harness_media_local::sdcpp::ENGINE_ID)
        .and_then(|e| match e.status() {
            harness_media_local::EngineStatus::Ready(v) => Some(v),
            harness_media_local::EngineStatus::NotInstalled(_) => None,
        });
    LocalMediaStatus {
        models: service
            .specs()
            .iter()
            .map(|s| {
                let missing = store.missing_bytes(s);
                LocalMediaModel {
                    id: s.id.clone(),
                    display_name: s.display_name.clone(),
                    developer: s.developer.clone(),
                    license: s.license.clone(),
                    summary: s.summary.clone(),
                    total_bytes: s.total_bytes(),
                    missing_bytes: missing,
                    installed: missing == 0,
                    typical_secs: s.typical_secs,
                    takes_references: s.max_refs > 0,
                }
            })
            .collect(),
        engine_version,
        store_path: store.root().display().to_string(),
        disk_available_bytes: disk_available(store.root()),
        error: None,
    }
}

fn disk_available(root: &std::path::Path) -> Option<u64> {
    // The store may not exist yet; measure the disk it will live on.
    let probe = root.ancestors().find(|p| p.exists())?;
    harness_local::disk_space(probe).map(|(_, available)| available)
}

fn pulls() -> &'static Mutex<HashMap<String, CancellationToken>> {
    static PULLS: OnceLock<Mutex<HashMap<String, CancellationToken>>> = OnceLock::new();
    PULLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A pull's slot in the table, freed however the pull ends.
struct PullSlot(String);

impl Drop for PullSlot {
    fn drop(&mut self) {
        match pulls().lock() {
            Ok(mut table) => {
                table.remove(&self.0);
            }
            Err(_) => tracing::warn!("pull table poisoned; {} stays listed", self.0),
        }
    }
}

/// Emits progress at most a few times a second, and always the first and
/// last.
#[derive(Default)]
struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    fn ready(&mut self, done: u64, total: u64) -> bool {
        let due = self
            .last
            .is_none_or(|at| at.elapsed() >= Duration::from_millis(150));
        if done >= total || due {
            self.last = Some(Instant::now());
            return true;
        }
        false
    }
}

/// Download an image model's weights (and the engine runtime if missing),
/// streaming `local_media.progress`. One pull per model at a time.
pub async fn pull_local_media(sink: Arc<dyn EventSink>, id: &str) -> Result<(), String> {
    let service = harness_media_local::service().map_err(|e| e.to_string())?;
    let spec = service
        .spec(id)
        .cloned()
        .ok_or_else(|| format!("no local image model `{id}`"))?;
    let cancel = CancellationToken::new();
    {
        let mut table = pulls()
            .lock()
            .map_err(|_| "pull table poisoned".to_string())?;
        if table.contains_key(&spec.id) {
            return Err(format!("{} is already downloading", spec.display_name));
        }
        table.insert(spec.id.clone(), cancel.clone());
    }
    let _slot = PullSlot(spec.id.clone());
    let report = Reporter {
        sink,
        id: spec.id.clone(),
    };
    let result = pull_inner(&service, &spec, &cancel, &report).await;
    match &result {
        Ok(()) => report.emit("done", None, spec.total_bytes(), spec.total_bytes(), None),
        Err(_) if cancel.is_cancelled() => report.emit("cancelled", None, 0, 0, None),
        Err(e) => report.emit("failed", None, 0, 0, Some(e.clone())),
    }
    result
}

/// Streams one pull's `local_media.progress` events.
struct Reporter {
    sink: Arc<dyn EventSink>,
    id: String,
}

impl Reporter {
    fn emit(
        &self,
        phase: &str,
        file: Option<String>,
        done: u64,
        total: u64,
        error: Option<String>,
    ) {
        self.sink.emit(ProtocolEvent::LocalMediaProgress {
            id: self.id.clone(),
            phase: phase.to_string(),
            file,
            done_bytes: done,
            total_bytes: total,
            error,
        });
    }
}

async fn pull_inner(
    service: &harness_media_local::LocalMediaService,
    spec: &harness_media_local::ModelSpec,
    cancel: &CancellationToken,
    report: &Reporter,
) -> Result<(), String> {
    if spec.engine == harness_media_local::sdcpp::ENGINE_ID
        && harness_media_local::sdcpp::binary().is_none()
    {
        let mut throttle = Throttle::default();
        harness_media_local::sdcpp::install_runtime(cancel, |done, total| {
            let total = total.unwrap_or(0);
            if throttle.ready(done, total) {
                report.emit("runtime", Some("sd-server".into()), done, total, None);
            }
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    let store = service.store();
    let missing = store.missing_bytes(spec);
    if missing == 0 {
        return Ok(());
    }
    if let Some(available) = disk_available(store.root()) {
        if available < missing {
            return Err(format!(
                "{} needs {:.1} GB more on disk and only {:.1} GB is free",
                spec.display_name,
                missing as f64 / 1e9,
                available as f64 / 1e9
            ));
        }
    }
    // Progress is over the whole model, not file by file.
    let already = spec.total_bytes() - missing;
    let mut finished_files = 0u64;
    let mut last_total = 0u64;
    let mut current = String::new();
    let mut throttle = Throttle::default();
    store
        .pull(spec, cancel, |p| {
            if p.file != current {
                // The file just finished counts in full from here on.
                finished_files += last_total;
                current = p.file.clone();
            }
            last_total = p.total_bytes;
            let done = already + finished_files + p.done_bytes;
            if throttle.ready(done, spec.total_bytes()) {
                report.emit(
                    "weights",
                    Some(p.file.clone()),
                    done,
                    spec.total_bytes(),
                    None,
                );
            }
        })
        .await
        .map_err(|e| e.to_string())
}

/// Stop a running pull; its partial file is removed.
pub fn cancel_local_media_pull(id: &str) {
    match pulls().lock() {
        Ok(table) => {
            if let Some(token) = table.get(id) {
                token.cancel();
            }
        }
        Err(_) => tracing::warn!("pull table poisoned; could not cancel {id}"),
    }
}

/// Delete an image model's files (ones another installed model shares
/// stay). Returns the bytes freed.
pub fn remove_local_media(id: &str) -> Result<u64, String> {
    let service = harness_media_local::service().map_err(|e| e.to_string())?;
    let spec = service
        .spec(id)
        .cloned()
        .ok_or_else(|| format!("no local image model `{id}`"))?;
    let others: Vec<_> = service
        .specs()
        .iter()
        .filter(|o| o.id != spec.id && service.store().is_installed(o))
        .cloned()
        .collect();
    service
        .store()
        .remove(&spec, &others)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_status_lists_the_system_on_macos() {
        let status = memory_status();
        if cfg!(target_os = "macos") {
            let system = status.system.expect("macOS reports memory");
            assert!(system.total_bytes > system.used_bytes);
            assert!(["normal", "warn", "critical", "unknown"].contains(&system.pressure.as_str()));
        }
    }

    #[test]
    fn the_throttle_lets_the_last_update_through() {
        let mut t = Throttle::default();
        assert!(t.ready(1, 10), "the first update goes out");
        assert!(!t.ready(2, 10), "too soon");
        assert!(t.ready(10, 10), "the final update always goes out");
    }
}
