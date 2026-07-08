//! Small, dependency-free formatting helpers shared across the workspace.

/// Format a byte count as a short, human-readable string (e.g. `512 B`,
/// `1.5 KB`, `5.0 GB`).
///
/// Uses binary (1024-based) units. Values are shown with one decimal place,
/// except below `100` of a unit where a whole number reads cleaner, and plain
/// bytes which are never fractional.
///
/// ```
/// use harness_core::fmt::format_bytes;
/// assert_eq!(format_bytes(0), "0 B");
/// assert_eq!(format_bytes(512), "512 B");
/// assert_eq!(format_bytes(1536), "1.5 KB");
/// assert_eq!(format_bytes(20_400_000_000), "19.0 GB");
/// ```
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Format a token count the way every meter in the harness does: `980`,
/// `12.3k`, `1.2M`.
///
/// ```
/// use harness_core::fmt::human_tokens;
/// assert_eq!(human_tokens(980), "980");
/// assert_eq!(human_tokens(12_300), "12.3k");
/// assert_eq!(human_tokens(1_200_000), "1.2M");
/// ```
pub fn human_tokens(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_tokens_scales_units() {
        assert_eq!(human_tokens(0), "0");
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(1_000), "1.0k");
        assert_eq!(human_tokens(128_000), "128.0k");
        assert_eq!(human_tokens(2_500_000), "2.5M");
    }

    #[test]
    fn formats_byte_counts() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
        assert_eq!(format_bytes(20_400_000_000), "19.0 GB");
    }
}
