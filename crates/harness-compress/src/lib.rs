//! Reversible context compression for tool output.
//!
//! The agent resends the whole transcript on every model call, and most of its
//! bulk is stale tool output the model has already acted on. This crate
//! shrinks that output *before it goes on the wire* — the persisted transcript
//! is never touched — using two compressors picked by content shape:
//!
//! - [`crush`]: statistical sampling of large JSON arrays (keep boundaries,
//!   errors, anomalies, one exemplar per duplicate group).
//! - [`lines`]: head/tail/error-line extraction for long plain text (shell
//!   output, logs, file dumps), with consecutive-duplicate collapsing.
//!
//! Everything removed is stashed in a [`ccr::CcrStore`] keyed by the hash in
//! the inline `<<ccr:HASH>>` marker, and the `retrieve_original` tool serves
//! it back on demand — so compression is a cheaper *view* of the data, not a
//! loss of it. The techniques are a lean native port of the ideas in
//! [headroom](https://github.com/headroomlabs-ai/headroom).

use serde::{Deserialize, Serialize};

pub mod ccr;
pub mod crush;
pub mod lines;

pub use ccr::CcrStore;

/// The user-facing compression setting.
///
/// `Audit` runs the full pipeline and reports what it *would* save, but the
/// original request is sent untouched — a risk-free way to measure the
/// difference before turning compression on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionMode {
    #[default]
    Off,
    Audit,
    On,
}

impl CompressionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressionMode::Off => "off",
            CompressionMode::Audit => "audit",
            CompressionMode::On => "on",
        }
    }

    pub fn from_str_or_off(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "audit" => CompressionMode::Audit,
            "on" => CompressionMode::On,
            _ => CompressionMode::Off,
        }
    }
}

/// Tunables for the compressors. The defaults follow headroom's, scaled to
/// this harness; every threshold errs toward *not* compressing.
#[derive(Debug, Clone)]
pub struct CompressConfig {
    /// Content shorter than this is never touched (~200 tokens).
    pub min_chars: usize,
    /// A compression must shrink the text by at least this fraction to be
    /// accepted; marginal wins aren't worth the indirection.
    pub min_savings_ratio: f64,
    /// The most recent N tool results are always sent verbatim — the model is
    /// likely still working with them. (Applied by the caller, which sees the
    /// whole transcript; mirrors `compact::prune_tool_results`.)
    pub keep_recent_tools: usize,
    /// Error output up to this size is protected verbatim (beyond it, the
    /// line crusher still keeps every error line).
    pub protect_error_chars: usize,

    // JSON crusher
    /// Object arrays smaller than this are never analyzed.
    pub min_items_to_analyze: usize,
    /// Target row count for a crushed array (signal rows may exceed it).
    pub max_items_after_crush: usize,
    /// Rows kept verbatim from the head of an array.
    pub first_keep: usize,
    /// Rows kept verbatim from the tail of an array.
    pub last_keep: usize,

    // Line crusher
    /// Lines kept verbatim from the head of long text.
    pub head_lines: usize,
    /// Lines kept verbatim from the tail of long text.
    pub tail_lines: usize,
    /// Cap on error lines preserved from the elided middle.
    pub max_error_lines: usize,
}

impl Default for CompressConfig {
    fn default() -> Self {
        Self {
            min_chars: 800,
            min_savings_ratio: 0.15,
            keep_recent_tools: 2,
            protect_error_chars: 8_000,
            min_items_to_analyze: 5,
            max_items_after_crush: 15,
            first_keep: 5,
            last_keep: 3,
            head_lines: 20,
            tail_lines: 15,
            max_error_lines: 40,
        }
    }
}

/// The 12 keywords (headroom's list) that mark content as too important to
/// drop: rows/lines containing one are always preserved, and small outputs
/// containing one near the start are left entirely verbatim.
const ERROR_KEYWORDS: &[&str] = &[
    "error",
    "exception",
    "failed",
    "failure",
    "critical",
    "fatal",
    "crash",
    "panic",
    "abort",
    "timeout",
    "denied",
    "rejected",
];

/// Whether `text` mentions any error keyword (case-insensitive).
pub fn contains_error_keyword(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ERROR_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// A successful compression of one piece of tool output.
#[derive(Debug, Clone)]
pub struct Compressed {
    pub text: String,
    /// A short human-readable note on what was done (for events/logs).
    pub strategy: String,
    pub chars_before: usize,
    pub chars_after: usize,
}

/// Compress one tool result. Returns `None` when the content is protected,
/// too small, unshrinkable, or the savings don't clear the bar.
///
/// `store` is where dropped originals go; pass `None` in audit mode to run
/// the identical pipeline without stashing anything.
pub fn compress_tool_result(
    text: &str,
    cfg: &CompressConfig,
    store: Option<&CcrStore>,
) -> Option<Compressed> {
    if text.len() < cfg.min_chars {
        return None;
    }
    // Never re-compress: a marker inside compressed content would either be
    // lost or double-indirected, and `retrieve_original` results would loop.
    if text.contains(ccr::MARKER_PREFIX) {
        return None;
    }
    // Small-to-medium error output is sacred — the model needs it verbatim to
    // debug. (Huge error dumps still get the line treatment below, which
    // preserves every error-bearing line.)
    if text.len() <= cfg.protect_error_chars {
        let head = &text[..text.len().min(400)];
        if contains_error_keyword(head) {
            return None;
        }
    }

    let trimmed = text.trim_start();
    let (out, strategy) = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        // Structured data either crushes safely or passes through whole — the
        // line crusher would corrupt JSON, so no fallback.
        let mut value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let note = crush::crush_json(&mut value, cfg, store)?;
        (value.to_string(), format!("json: {note}"))
    } else {
        let (out, note) = lines::crush_lines(text, cfg, store)?;
        (out, format!("text: {note}"))
    };

    // Only accept a real win.
    if (out.len() as f64) > (text.len() as f64) * (1.0 - cfg.min_savings_ratio) {
        return None;
    }
    Some(Compressed {
        chars_before: text.len(),
        chars_after: out.len(),
        text: out,
        strategy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CompressConfig {
        CompressConfig::default()
    }

    fn repetitive_json(n: usize) -> String {
        let rows: Vec<serde_json::Value> = (0..n)
            .map(|i| serde_json::json!({"id": i, "level": "info", "message": "heartbeat ok"}))
            .collect();
        serde_json::Value::Array(rows).to_string()
    }

    #[test]
    fn mode_round_trips_through_strings_and_serde() {
        for mode in [
            CompressionMode::Off,
            CompressionMode::Audit,
            CompressionMode::On,
        ] {
            assert_eq!(CompressionMode::from_str_or_off(mode.as_str()), mode);
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.as_str()));
            assert_eq!(
                serde_json::from_str::<CompressionMode>(&json).unwrap(),
                mode
            );
        }
        assert_eq!(
            CompressionMode::from_str_or_off("garbage"),
            CompressionMode::Off
        );
    }

    #[test]
    fn compresses_repetitive_json_and_stores_original() {
        let text = repetitive_json(200);
        let store = CcrStore::default();
        let out = compress_tool_result(&text, &cfg(), Some(&store)).expect("should compress");
        assert!(out.chars_after < out.chars_before / 2);
        assert!(out.text.contains("<<ccr:"));
        assert!(out.strategy.starts_with("json:"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn small_content_and_errors_are_protected() {
        assert!(compress_tool_result("short output", &cfg(), None).is_none());

        let mut error_text = String::from("error: build failed\n");
        error_text.push_str(&"context line about the failure\n".repeat(100));
        assert!(
            compress_tool_result(&error_text, &cfg(), None).is_none(),
            "medium error output stays verbatim"
        );
    }

    #[test]
    fn already_compressed_content_is_skipped() {
        let mut text = repetitive_json(200);
        text.push_str(" <<ccr:aabbccddeeff>>");
        assert!(compress_tool_result(&text, &cfg(), None).is_none());
    }

    #[test]
    fn unshrinkable_json_passes_through() {
        // A big object with no large arrays — nothing to crush.
        let text = format!("{{\"blob\": \"{}\"}}", "x".repeat(2000));
        assert!(compress_tool_result(&text, &cfg(), None).is_none());
    }

    #[test]
    fn long_shell_output_compresses_via_lines() {
        let text = (0..500)
            .map(|i| format!("Compiling crate number {i} of the workspace build"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = compress_tool_result(&text, &cfg(), None).expect("should compress");
        assert!(out.strategy.starts_with("text:"));
        assert!(out.chars_after < out.chars_before / 2);
    }

    #[test]
    fn audit_mode_stores_nothing_but_reports_the_same_savings() {
        let text = repetitive_json(200);
        let store = CcrStore::default();
        let applied = compress_tool_result(&text, &cfg(), Some(&store)).unwrap();
        let audited = compress_tool_result(&text, &cfg(), None).unwrap();
        assert_eq!(applied.chars_after, audited.chars_after);
        assert_eq!(applied.text, audited.text, "audit pipeline is identical");
        assert_eq!(store.len(), 1, "only the applied run stored");
    }
}
