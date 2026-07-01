//! Explaining *why* an `edit_file` match failed.
//!
//! When `old_string` isn't found, a bare "not found" leaves the model to guess
//! and retry blindly. These helpers diagnose the three mistakes LLM agents make
//! most often — pasting `read_file`'s line-number prefix, mismatched whitespace,
//! and a drifted anchor line — so the tool can hand back a message the model can
//! recover from in a single step.

/// Diagnose a failed `edit_file` match, checked in order of confidence, falling
/// back to a plain "not found".
pub(crate) fn diagnose_no_match(original: &str, old: &str) -> String {
    // 1. Did the model paste `read_file`'s `  123\t` line-number prefix into
    //    `old_string`? Very common, and unambiguous when the de-prefixed text
    //    then matches.
    if let Some(stripped) = strip_line_number_prefix(old) {
        if !stripped.is_empty() && original.contains(&stripped) {
            return "`old_string` not found — but it matches once the line-number/tab \
                    prefix is removed. `read_file` prepends `<number>\\t` to each line; \
                    pass only the real file content that follows the tab."
                .into();
        }
    }

    // 2. Right text, wrong whitespace (tabs vs spaces, indentation width,
    //    trailing spaces, or CRLF vs LF). Compare with all whitespace removed.
    if strip_whitespace(old).len() >= 4
        && strip_whitespace(original).contains(&strip_whitespace(old))
    {
        return "`old_string` not found — the same text exists but the whitespace differs \
                (tabs vs spaces, indentation, trailing spaces, or line endings). Re-read \
                the file and copy the exact indentation and spacing."
            .into();
    }

    // 3. Nothing close. If a single line of `old_string` does occur, point at it
    //    so the model knows its anchor drifted rather than being absent entirely.
    if let Some(anchor) = old.lines().find(|l| {
        let t = l.trim();
        t.len() >= 4 && original.contains(t)
    }) {
        return format!(
            "`old_string` not found. The line `{}` does appear in the file, but the \
             surrounding text does not match — re-read the file and copy the exact \
             current content around it.",
            anchor.trim()
        );
    }

    "`old_string` not found in file".into()
}

/// If every non-empty line of `s` carries `read_file`'s `<spaces><digits>\t`
/// prefix, return `s` with those prefixes removed; otherwise `None`.
fn strip_line_number_prefix(s: &str) -> Option<String> {
    let mut saw_prefixed = false;
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match line.split_once('\t') {
            Some((head, rest))
                if !head.is_empty()
                    && head.chars().all(|c| c.is_ascii_digit() || c == ' ')
                    && head.chars().any(|c| c.is_ascii_digit()) =>
            {
                saw_prefixed = true;
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
    }
    saw_prefixed.then_some(out)
}

/// `s` with all Unicode whitespace removed — used to test for whitespace-only
/// mismatches.
fn strip_whitespace(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_a_pasted_line_number_prefix() {
        let original = "fn main() {\n    let x = 1;\n}\n";
        let msg = diagnose_no_match(original, "     2\t    let x = 1;");
        assert!(msg.contains("line-number"), "got: {msg}");
    }

    #[test]
    fn flags_a_whitespace_only_mismatch() {
        let original = "fn f() {\n\treturn 42;\n}\n"; // tab-indented
        let msg = diagnose_no_match(original, "    return 42;"); // space-indented
        assert!(msg.contains("whitespace"), "got: {msg}");
    }

    #[test]
    fn points_at_a_drifted_anchor_line() {
        let original = "let total = compute_total();\n";
        let msg = diagnose_no_match(
            original,
            "let total = compute_total();\nprintln!(\"{total}\");",
        );
        assert!(msg.contains("does appear"), "got: {msg}");
        assert!(msg.contains("compute_total"), "got: {msg}");
    }

    #[test]
    fn falls_back_to_plain_not_found() {
        let msg = diagnose_no_match("alpha beta gamma", "wholly unrelated content");
        assert_eq!(msg, "`old_string` not found in file");
    }

    #[test]
    fn strip_line_number_prefix_requires_every_line_prefixed() {
        assert_eq!(
            strip_line_number_prefix("  12\tcode"),
            Some("code".to_string())
        );
        // A line with no prefix means the whole block isn't line-numbered output.
        assert_eq!(strip_line_number_prefix("plain text"), None);
    }
}
