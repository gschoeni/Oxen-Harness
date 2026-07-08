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
    // Whole-string parse first, but only accept an *object*: a reply that is a
    // bare array or scalar still gets the brace hunt below (an array wrapping
    // one object is a shape models actually produce), and a scalar-only reply
    // yields None so callers keep their raw-text diagnostics.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if v.is_object() {
            return Some(v);
        }
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
        // Bare scalars are valid JSON but not objects — callers need None so
        // their "unparseable reply" diagnostics (with the raw text) still fire.
        assert!(first_object("42").is_none());
        assert!(first_object("\"done\"").is_none());
    }

    #[test]
    fn salvages_the_object_inside_an_array_reply() {
        // A verifier reply wrapped in a one-element array: the brace hunt must
        // recover the inner object rather than returning the array.
        let v = first_object(r#"[{"scores":[{"criterion":"a","score":9}]}]"#).unwrap();
        assert!(v.is_object());
        assert!(v.get("scores").is_some());
    }
}
