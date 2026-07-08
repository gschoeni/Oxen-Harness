//! Lenient JSON extraction from model replies.
//!
//! Models asked to "reply with only JSON" still wrap it in prose or code
//! fences often enough that every consumer needs the same salvage step. This
//! is that step, written once: verifiers, report parsers, and graders all call
//! [`first_object`] instead of hand-rolling brace hunts.

/// Pull the outermost JSON object out of a model reply.
///
/// Tries the whole string first (the well-behaved case), then falls back to
/// the slice between the first `{` and the last `}` — which shrugs off code
/// fences, lead-in prose, and trailing sign-offs. Returns `None` when neither
/// parses; callers decide whether that's a failure or a fallback path.
///
/// ```
/// use harness_core::json::first_object;
/// let v = first_object(r#"Here you go:
/// ```json
/// {"score": 9}
/// ```"#).unwrap();
/// assert_eq!(v["score"], 9);
/// assert!(first_object("no json here").is_none());
/// ```
pub fn first_object(raw: &str) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_str(raw) {
        return Some(v);
    }
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&raw[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_fenced_objects() {
        assert_eq!(first_object(r#"{"a":1}"#).unwrap()["a"], 1);
        assert_eq!(first_object("prose {\"a\":1} trailing").unwrap()["a"], 1);
    }

    #[test]
    fn rejects_text_without_an_object() {
        assert!(first_object("").is_none());
        assert!(first_object("} backwards {").is_none());
    }
}
