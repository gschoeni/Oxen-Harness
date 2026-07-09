//! Best-effort developer error log.
//!
//! Every retry attempt and every terminal turn failure is appended as one JSON
//! line to the path in [`AgentConfig::error_log`](crate::AgentConfig), so a
//! developer can reconstruct what went wrong (when, which session, which
//! model/endpoint, what the provider said) long after the terminal scrolled
//! away. Logging is strictly best-effort: a full disk or bad path must never
//! break a turn, so failures to write are traced and dropped.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Append one entry to the error log at `path`. `None` (no log configured)
/// and any IO failure are silently ignored — the log is diagnostics, not
/// state.
pub(crate) fn record(path: Option<&Path>, event: &str, mut entry: serde_json::Value) {
    let Some(path) = path else { return };
    let (epoch_ms, ts) = now();
    if let Some(map) = entry.as_object_mut() {
        map.insert("ts".into(), ts.into());
        map.insert("epoch_ms".into(), epoch_ms.into());
        map.insert("event".into(), event.into());
    }
    if let Err(e) = append_line(path, &entry) {
        tracing::debug!("error log write failed ({}): {e}", path.display());
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

/// The wall-clock now as (epoch milliseconds, ISO-8601 UTC string).
fn now() -> (u64, String) {
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    (epoch_ms, iso8601_utc(epoch_ms / 1000))
}

/// Format epoch seconds as `YYYY-MM-DDTHH:MM:SSZ` without a date-time
/// dependency (civil-from-days, Howard Hinnant's algorithm).
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
    fn iso8601_matches_known_instants() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        // `date -u -r 1783977338` → Jul 13 2026 21:15:38 UTC.
        assert_eq!(iso8601_utc(1_783_977_338), "2026-07-13T21:15:38Z");
        // A leap-year February 29th.
        assert_eq!(iso8601_utc(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn record_appends_one_json_line_per_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/errors.jsonl");

        record(
            Some(&path),
            "retrying",
            serde_json::json!({"session": "s1", "error": "boom"}),
        );
        record(
            Some(&path),
            "turn_failed",
            serde_json::json!({"session": "s1", "error": "still down"}),
        );

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> = body
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["event"], "retrying");
        assert_eq!(lines[0]["session"], "s1");
        assert!(lines[0]["ts"].as_str().unwrap().ends_with('Z'));
        assert!(lines[0]["epoch_ms"].as_u64().unwrap() > 0);
        assert_eq!(lines[1]["event"], "turn_failed");
    }

    #[test]
    fn record_without_a_path_is_a_no_op() {
        record(None, "retrying", serde_json::json!({"error": "boom"}));
    }
}
