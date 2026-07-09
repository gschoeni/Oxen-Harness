//! Small, dependency-free string helpers shared across the workspace.

/// Normalize `name` into a lowercase, filesystem- and anchor-safe slug.
///
/// ASCII alphanumerics are lowercased and kept; every other run of characters
/// collapses to a single `-`, and leading/trailing dashes are trimmed. When the
/// result would be empty, `fallback` is returned instead — callers pass a
/// domain-appropriate default such as `"theme"` or `"loop"`.
///
/// ```
/// use harness_core::text::slug;
/// assert_eq!(slug("Oregon Trail", "theme"), "oregon-trail");
/// assert_eq!(slug("  My!! Cool   Theme  ", "theme"), "my-cool-theme");
/// assert_eq!(slug("***", "theme"), "theme");
/// ```
pub fn slug(name: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

/// Cap `s` at `max` characters, appending `…` when anything was cut.
///
/// Char-safe (never splits a multi-byte character) and width-honest: the
/// result is at most `max + 1` characters — the ellipsis marks the cut rather
/// than eating into the budget. Callers fitting an exact column width should
/// budget for that extra cell.
///
/// ```
/// use harness_core::text::ellipsize;
/// assert_eq!(ellipsize("short", 10), "short");
/// assert_eq!(ellipsize("a very long line", 6), "a very…");
/// ```
pub fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max).collect();
    format!("{kept}…")
}

/// Collapse every run of whitespace (including newlines) to a single space,
/// trimming the ends — turning any text into one display-safe line.
///
/// ```
/// use harness_core::text::collapse_ws;
/// assert_eq!(collapse_ws("reading  the\n parser\tmodule "), "reading the parser module");
/// ```
pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keep only the freshest `cap` characters of `s` — the rolling-tail buffer
/// behind live activity readouts, where the newest output matters and the
/// oldest can fall off. Char-safe.
///
/// ```
/// use harness_core::text::tail_chars;
/// assert_eq!(tail_chars("abcdef", 4), "cdef");
/// assert_eq!(tail_chars("abc", 4), "abc");
/// ```
pub fn tail_chars(s: &str, cap: usize) -> String {
    let count = s.chars().count();
    if count <= cap {
        s.to_string()
    } else {
        s.chars().skip(count - cap).collect()
    }
}

/// Append `addition` to a rolling buffer, keeping roughly the freshest `cap`
/// characters — the in-place, amortized counterpart to [`tail_chars`] for a
/// buffer that grows token-by-token.
///
/// The append is the only work on the common path; the buffer is trimmed back
/// down to `cap` chars *lazily*, once it has grown past a slack margin, so the
/// O(len) scan-and-drop runs at most once per `cap` characters appended rather
/// than on every call (which would make a token stream quadratic). The trim
/// keeps the freshest `cap` chars and never splits a character; between trims
/// the buffer may hold up to ~`cap` extra chars of slack, which callers that
/// display the *end* of the buffer (a rolling tail) don't care about.
///
/// ```
/// use harness_core::text::push_capped;
/// let mut buf = String::new();
/// for _ in 0..1000 { push_capped(&mut buf, "x", 8); }
/// // Bounded near the cap (with up to a few caps of lazy slack), tail kept.
/// assert!(buf.chars().count() >= 8 && buf.chars().count() <= 32);
/// assert!(buf.ends_with('x'));
/// ```
pub fn push_capped(buf: &mut String, addition: &str, cap: usize) {
    buf.push_str(addition);
    // Cheap O(1) gate on byte length (a char is ≤ 4 bytes, so `cap * 4` bytes
    // is always ≥ `cap` chars): only pay for the char scan once we're clearly
    // over the cap, then drop the oldest chars back down to it.
    if buf.len() > cap.saturating_mul(4) {
        let count = buf.chars().count();
        if count > cap {
            if let Some((byte, _)) = buf.char_indices().nth(count - cap) {
                buf.drain(..byte);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_capped_amortizes_and_keeps_the_tail() {
        let mut buf = String::new();
        for i in 0..500 {
            push_capped(&mut buf, &format!("{}", i % 10), 20);
        }
        // Bounded near the cap (with lazy slack), char-safe, freshest kept.
        let n = buf.chars().count();
        assert!((20..=80).contains(&n), "len {n} out of bounds");
        assert!(buf.ends_with('9'));

        // Multi-byte safe: never splits a character on trim.
        let mut emoji = String::new();
        for _ in 0..100 {
            push_capped(&mut emoji, "😀", 4);
        }
        assert!(emoji.chars().all(|c| c == '😀'));
    }

    #[test]
    fn ellipsize_is_char_safe() {
        // Multi-byte characters count as one; the cut never splits them.
        assert_eq!(ellipsize("⠋⠙⠹⠸", 2), "⠋⠙…");
        assert_eq!(ellipsize("⠋⠙", 2), "⠋⠙");
    }

    #[test]
    fn tail_chars_keeps_the_freshest() {
        let tail = tail_chars(&format!("{}END", "x".repeat(100)), 5);
        assert_eq!(tail, "xxEND");
        assert_eq!(tail_chars("⠋⠙⠹", 2), "⠙⠹");
    }

    #[test]
    fn lowercases_and_dashes_separators() {
        assert_eq!(slug("Green Tests", "loop"), "green-tests");
        assert_eq!(slug("SYNTHWAVE", "theme"), "synthwave");
    }

    #[test]
    fn collapses_runs_and_trims_edges() {
        assert_eq!(slug("  Make  it!! green ", "loop"), "make-it-green");
        assert_eq!(slug("--edge--", "x"), "edge");
    }

    #[test]
    fn empty_result_uses_fallback() {
        assert_eq!(slug("***", "loop"), "loop");
        assert_eq!(slug("", "theme"), "theme");
    }
}
