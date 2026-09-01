//! Repairing tool arguments the model almost got right.
//!
//! Small models (and big ones on a bad day) send `"5"` for a number, `"yes"`
//! for a boolean, a JSON-encoded string where an array was wanted, or one
//! bracket short of a valid object. Each of those costs a full model round
//! to read the error and try again. The doctrine here, borrowed from
//! oh-my-pi: **never invent a value — only massage a shape the model almost
//! got right**, and only in the direction the tool's own schema asks for.
//!
//! Two entry points, both pure:
//! - [`heal_json`] — for arguments that don't parse at all: trim a trailing
//!   fragment and close up to a few unclosed brackets.
//! - [`coerce_to_schema`] — for parsed arguments the tool rejected: walk the
//!   schema and convert leaf values whose type disagrees with it.

use serde_json::Value;

/// How many closing brackets a heal may add. Past this the JSON is not
/// "almost valid", it's a different document.
const MAX_HEAL_BRACKETS: usize = 3;

/// Try to turn unparseable arguments into a JSON object: strip anything after
/// the last complete token, then close up to [`MAX_HEAL_BRACKETS`] open
/// brackets/braces. Returns `None` when nothing that small makes it parse.
pub fn heal_json(raw: &str) -> Option<Value> {
    let mut text = raw.trim().to_string();
    // Each attempt closes what's open; if that doesn't parse, the trailing
    // fragment (after the last unquoted comma) is a partial member — drop
    // it and try again, a few times at most.
    for _ in 0..=MAX_HEAL_BRACKETS {
        if text.is_empty() {
            return None;
        }
        let (stack, in_string) = scan_brackets(&text)?;
        if stack.len() > MAX_HEAL_BRACKETS {
            return None;
        }
        let candidate = close_brackets(&text, &stack, in_string);
        if let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(&candidate) {
            return Some(value);
        }
        let cut = last_unquoted_comma(&text)?;
        text.truncate(cut);
    }
    None
}

/// The closers still open at the end of `text` (innermost last), and whether
/// it ends inside a string. `None` when a closer doesn't match its opener —
/// that isn't "almost valid".
fn scan_brackets(text: &str) -> Option<(Vec<char>, bool)> {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' if stack.pop() != Some(ch) => return None,
            _ => {}
        }
    }
    Some((stack, in_string))
}

/// `text` with an open string terminated, dangling separators dropped, and
/// every open bracket closed.
fn close_brackets(text: &str, stack: &[char], in_string: bool) -> String {
    let mut candidate = text.to_string();
    if in_string {
        candidate.push('"');
    }
    while candidate.ends_with([',', ':', ' ']) {
        candidate.pop();
    }
    for closer in stack.iter().rev() {
        candidate.push(*closer);
    }
    candidate
}

/// Byte offset of the last `,` outside any string, if any.
fn last_unquoted_comma(text: &str) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;
    let mut last = None;
    for (i, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            ',' => last = Some(i),
            _ => {}
        }
    }
    last
}

/// Repair `args` against a JSON-Schema `schema`, returning the repaired
/// value when anything changed. Only type disagreements are touched:
/// a string that holds a number/boolean/JSON document where the schema wants
/// one, a scalar where a string is wanted, and — for a schema with exactly
/// one required string property — a payload sent under the wrong key.
pub fn coerce_to_schema(schema: &Value, args: &Value) -> Option<Value> {
    let mut repaired = args.clone();
    let mut changed = adopt_single_string_field(schema, &mut repaired);
    changed |= coerce_value(schema, &mut repaired);
    changed.then_some(repaired)
}

/// A tool whose schema has one required string property (the way an
/// `edit`-style patch tool is shaped) sometimes gets its payload under
/// `input`, `text`, or the tool's own name. Adopt it as the declared key
/// when that key is missing and the object holds exactly one string.
fn adopt_single_string_field(schema: &Value, args: &mut Value) -> bool {
    let Some(obj) = args.as_object_mut() else {
        return false;
    };
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    if required.len() != 1 {
        return false;
    }
    let key = required[0];
    let wants_string = schema
        .pointer(&format!("/properties/{key}/type"))
        .and_then(Value::as_str)
        == Some("string");
    if !wants_string || obj.contains_key(key) {
        return false;
    }
    let strings: Vec<String> = obj
        .iter()
        .filter(|(_, v)| v.is_string())
        .map(|(k, _)| k.clone())
        .collect();
    if strings.len() != 1 || obj.len() != 1 {
        return false;
    }
    let value = obj.remove(&strings[0]).expect("present");
    obj.insert(key.to_string(), value);
    true
}

/// Coerce one value toward its schema, recursing into objects and arrays.
fn coerce_value(schema: &Value, value: &mut Value) -> bool {
    let Some(kind) = schema_type(schema) else {
        return false;
    };
    match kind {
        "object" => {
            let Some(props) = schema.get("properties").and_then(Value::as_object) else {
                return coerce_scalar(kind, value);
            };
            if let Value::String(text) = value {
                // A JSON document sent as a string.
                if let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(text) {
                    *value = parsed;
                    coerce_value(schema, value);
                    return true;
                }
                return false;
            }
            let Some(obj) = value.as_object_mut() else {
                return false;
            };
            let mut changed = false;
            for (key, field) in obj.iter_mut() {
                if let Some(field_schema) = props.get(key) {
                    changed |= coerce_value(field_schema, field);
                }
            }
            changed
        }
        "array" => {
            if let Value::String(text) = value {
                if let Ok(parsed @ Value::Array(_)) = serde_json::from_str::<Value>(text) {
                    *value = parsed;
                    coerce_value(schema, value);
                    return true;
                }
                return false;
            }
            let Some(items) = value.as_array_mut() else {
                return false;
            };
            let Some(item_schema) = schema.get("items") else {
                return false;
            };
            let mut changed = false;
            for item in items {
                changed |= coerce_value(item_schema, item);
            }
            changed
        }
        _ => coerce_scalar(kind, value),
    }
}

/// The declared type of a schema node, ignoring a nullable union.
fn schema_type(schema: &Value) -> Option<&str> {
    match schema.get("type")? {
        Value::String(s) => Some(s.as_str()),
        Value::Array(types) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null"),
        _ => None,
    }
}

fn coerce_scalar(kind: &str, value: &mut Value) -> bool {
    match (kind, &*value) {
        ("integer", Value::String(s)) => match s.trim().parse::<i64>() {
            Ok(n) => {
                *value = Value::from(n);
                true
            }
            Err(_) => false,
        },
        ("integer", Value::Number(n)) if !n.is_i64() && !n.is_u64() => match n.as_f64() {
            Some(f) if f.fract() == 0.0 => {
                *value = Value::from(f as i64);
                true
            }
            _ => false,
        },
        ("number", Value::String(s)) => match s.trim().parse::<f64>() {
            Ok(f) => {
                *value = serde_json::json!(f);
                true
            }
            Err(_) => false,
        },
        ("boolean", Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => {
                *value = Value::Bool(true);
                true
            }
            "false" | "no" | "off" | "0" => {
                *value = Value::Bool(false);
                true
            }
            _ => false,
        },
        ("boolean", Value::Number(n)) => match n.as_i64() {
            Some(0) => {
                *value = Value::Bool(false);
                true
            }
            Some(1) => {
                *value = Value::Bool(true);
                true
            }
            _ => false,
        },
        ("string", Value::Number(_)) | ("string", Value::Bool(_)) => {
            *value = Value::String(value.to_string());
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn heals_a_missing_closing_brace() {
        let healed = heal_json(r#"{"path":"a.rs","limit":20"#).unwrap();
        assert_eq!(healed, json!({"path":"a.rs","limit":20}));
    }

    #[test]
    fn heals_an_unterminated_string_and_nested_brackets() {
        let healed = heal_json(r#"{"edits":[{"old_string":"foo"#).unwrap();
        assert_eq!(healed, json!({"edits":[{"old_string":"foo"}]}));
    }

    #[test]
    fn drops_a_dangling_member_before_closing() {
        let healed = heal_json(r#"{"path":"a.rs","limit":"#).unwrap();
        assert_eq!(healed, json!({"path":"a.rs"}));
    }

    #[test]
    fn refuses_to_invent_a_document() {
        assert!(heal_json("").is_none());
        assert!(heal_json("not json at all").is_none());
        assert!(
            heal_json(r#"{"a":[[[["#).is_none(),
            "too many open brackets"
        );
        assert!(heal_json(r#"{"a":1]"#).is_none(), "mismatched closer");
    }

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "limit": {"type": "integer"},
                "ratio": {"type": "number"},
                "replace_all": {"type": "boolean"},
                "edits": {"type": "array", "items": {"type": "object", "properties": {
                    "old_string": {"type": "string"}, "replace_all": {"type": "boolean"}}}},
                "opts": {"type": "object", "properties": {"deep": {"type": "boolean"}}}
            }
        })
    }

    #[test]
    fn coerces_stringly_typed_scalars_toward_the_schema() {
        let args = json!({"path": 42, "limit": "20", "ratio": "0.5", "replace_all": "yes"});
        let fixed = coerce_to_schema(&schema(), &args).unwrap();
        assert_eq!(
            fixed,
            json!({"path": "42", "limit": 20, "ratio": 0.5, "replace_all": true})
        );
    }

    #[test]
    fn parses_json_documents_sent_as_strings_and_recurses() {
        let args = json!({
            "edits": "[{\"old_string\":\"a\",\"replace_all\":\"true\"}]",
            "opts": "{\"deep\":\"no\"}"
        });
        let fixed = coerce_to_schema(&schema(), &args).unwrap();
        assert_eq!(
            fixed,
            json!({"edits": [{"old_string": "a", "replace_all": true}], "opts": {"deep": false}})
        );
    }

    #[test]
    fn adopts_a_mis_keyed_single_required_string() {
        let schema = json!({
            "type": "object",
            "properties": {"patch": {"type": "string"}},
            "required": ["patch"]
        });
        let fixed = coerce_to_schema(&schema, &json!({"input": "PUT 1"})).unwrap();
        assert_eq!(fixed, json!({"patch": "PUT 1"}));
        // Two fields, or the right key present: nothing to adopt.
        assert!(coerce_to_schema(&schema, &json!({"patch": "x"})).is_none());
        assert!(coerce_to_schema(&schema, &json!({"input": "x", "other": "y"})).is_none());
    }

    #[test]
    fn leaves_already_valid_arguments_alone() {
        let args = json!({"path": "a.rs", "limit": 20, "replace_all": false});
        assert!(coerce_to_schema(&schema(), &args).is_none());
    }

    #[test]
    fn never_invents_values_for_unparseable_scalars() {
        let args = json!({"limit": "twenty", "replace_all": "maybe"});
        assert!(coerce_to_schema(&schema(), &args).is_none());
    }
}
