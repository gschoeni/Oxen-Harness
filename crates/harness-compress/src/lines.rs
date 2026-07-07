//! Line-based compression for logs, shell output, and other long plain text.
//!
//! Three moves, cheapest first:
//!
//! 1. Collapse runs of identical lines into one line + a repeat count
//!    (progress spinners, retry loops, and heartbeat logs collapse hard).
//! 2. Keep the head and tail windows verbatim — commands announce themselves
//!    at the top and conclude at the bottom.
//! 3. Keep every error-bearing line from the elided middle, and replace the
//!    rest with a note carrying a `<<ccr:HASH>>` marker so the model can
//!    retrieve the full output if it needs it.

use crate::ccr::{marker, CcrStore};
use crate::{contains_error_keyword, CompressConfig};

/// Compress `text` line-wise. Returns `(compressed, strategy note)` when the
/// text shrank, `None` when there was nothing worth doing. `store` is `None`
/// in audit mode (marker rendered, original not stashed).
pub fn crush_lines(
    text: &str,
    cfg: &CompressConfig,
    store: Option<&CcrStore>,
) -> Option<(String, String)> {
    let raw: Vec<&str> = text.lines().collect();

    // Pass 1: collapse consecutive duplicates.
    let mut lines: Vec<String> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let mut run = 1;
        while i + run < raw.len() && raw[i + run] == raw[i] {
            run += 1;
        }
        if run >= 3 && !raw[i].trim().is_empty() {
            lines.push(format!("{} [repeated {run}\u{00d7}]", raw[i]));
        } else {
            for _ in 0..run {
                lines.push(raw[i].to_string());
            }
        }
        i += run;
    }

    let total = lines.len();
    if total <= cfg.head_lines + cfg.tail_lines + 4 {
        // Short enough to send whole — worth it only if dedup alone paid.
        let out = lines.join("\n");
        return (out.len() * 100 <= text.len() * 85)
            .then_some((out, format!("deduped {} lines", raw.len())));
    }

    // Pass 2: head + tail verbatim, error lines from the middle, marker note.
    let head = &lines[..cfg.head_lines];
    let tail = &lines[total - cfg.tail_lines..];
    let middle = &lines[cfg.head_lines..total - cfg.tail_lines];
    let errors: Vec<&String> = middle
        .iter()
        .filter(|l| contains_error_keyword(l))
        .take(cfg.max_error_lines)
        .collect();

    let hash = match store {
        Some(store) => store.put(text),
        None => crate::ccr::hash_content(text),
    };
    let elided = middle.len() - errors.len();
    let mut out = String::with_capacity(text.len() / 4);
    for line in head {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!(
        "[... {elided} of {} lines elided \u{2014} full output: {} ...]\n",
        raw.len(),
        marker(&hash, None)
    ));
    for line in &errors {
        out.push_str(line);
        out.push('\n');
    }
    if !errors.is_empty() {
        out.push_str("[... end of preserved error lines ...]\n");
    }
    for line in tail {
        out.push_str(line);
        out.push('\n');
    }
    let out = out.trim_end().to_string();

    Some((
        out,
        format!(
            "{} lines\u{2192}{} + {} error lines",
            raw.len(),
            cfg.head_lines + cfg.tail_lines,
            errors.len()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CompressConfig {
        CompressConfig::default()
    }

    #[test]
    fn long_output_keeps_head_tail_and_error_lines() {
        let mut lines: Vec<String> = (0..300).map(|i| format!("building step {i} ok")).collect();
        lines[150] = "error: undefined reference to `foo`".to_string();
        let text = lines.join("\n");

        let store = CcrStore::default();
        let (out, note) = crush_lines(&text, &cfg(), Some(&store)).expect("should compress");
        assert!(out.len() < text.len() / 2);
        assert!(out.contains("building step 0 ok"), "head kept");
        assert!(out.contains("building step 299 ok"), "tail kept");
        assert!(out.contains("undefined reference"), "error line kept");
        assert!(out.contains("<<ccr:"), "marker present");
        assert!(note.contains("300 lines"));

        // Full original retrievable.
        let hash = out
            .split("<<ccr:")
            .nth(1)
            .unwrap()
            .split(">>")
            .next()
            .unwrap();
        assert_eq!(store.get(hash).as_deref(), Some(text.as_str()));
    }

    #[test]
    fn repeated_lines_collapse_with_count() {
        let text = ["fetching..."; 50].join("\n");
        let (out, _) = crush_lines(&text, &cfg(), None).expect("dedup should pay");
        assert!(out.contains("fetching... [repeated 50\u{00d7}]"));
        assert!(!out.contains("<<ccr:"), "pure dedup needs no marker");
    }

    #[test]
    fn short_unique_output_is_left_alone() {
        let text = (0..10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(crush_lines(&text, &cfg(), None).is_none());
    }
}
