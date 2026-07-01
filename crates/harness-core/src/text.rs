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

#[cfg(test)]
mod tests {
    use super::*;

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
