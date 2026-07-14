//! Best-effort audit log of gate decisions.
//!
//! Every deny, every approval prompt's resolution, and every allow of a
//! non-trivial shell command appends one JSON line to
//! `~/.oxen-harness/permissions.jsonl`, recording *which rule or decision*
//! permitted or refused it — so "why did that run?" stays answerable after the
//! screen scrolled. Mirrors the developer error log (`harness-agent::errlog`):
//! strictly best-effort, a full disk must never break a turn.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Append one audit entry. `None` (no log path resolvable) and IO failures are
/// traced and dropped — the log is diagnostics, not state.
pub(crate) fn record(path: Option<&Path>, mut entry: serde_json::Value) {
    let Some(path) = path else { return };
    let (epoch_ms, ts) = now();
    if let Some(map) = entry.as_object_mut() {
        map.insert("ts".into(), ts.into());
        map.insert("epoch_ms".into(), epoch_ms.into());
    }
    if let Err(e) = append_line(path, &entry) {
        tracing::debug!("permissions audit write failed ({}): {e}", path.display());
    }
}

fn append_line(path: &Path, entry: &serde_json::Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{entry}")
}

/// The wall-clock now as (epoch milliseconds, ISO-8601 UTC string). Same
/// civil-from-days rendering as the error log, kept dependency-free.
fn now() -> (u64, String) {
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    (epoch_ms, iso8601_utc(epoch_ms / 1000))
}

fn iso8601_utc(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86_400) as i64;
    let secs = epoch_secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i64 + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3_600,
        (secs / 60) % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_appends_json_lines_with_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("permissions.jsonl");
        record(
            Some(&path),
            serde_json::json!({"decision": "deny", "command": "rm -rf /"}),
        );
        let body = std::fs::read_to_string(&path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        assert_eq!(entry["decision"], "deny");
        assert!(entry["ts"].as_str().unwrap().ends_with('Z'));
    }
}
