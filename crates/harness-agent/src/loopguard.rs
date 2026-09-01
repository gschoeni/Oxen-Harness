//! Detection of unproductive tool loops inside a turn.
//!
//! Each model round in a tool loop re-sends the whole growing context, so a
//! model stuck repeating the same call burns a full prompt per iteration while
//! learning nothing. The guard watches consecutive (tool, arguments, result)
//! triples: a repeat with an *identical result* carries zero new information.
//! At [`NUDGE_AFTER`] identical repeats the turn gets one corrective nudge; at
//! [`STOP_AFTER`] the turn is ended with an explanation — continuing is
//! economically irrational.
//!
//! Legitimate repetition survives this: polling a background task yields
//! changing results (the cursor advances), and a retried command after a fix
//! has an edit call in between, which resets the run.

use std::hash::{Hash, Hasher};

/// Identical repeats after which the model gets one corrective nudge.
pub const NUDGE_AFTER: u32 = 3;
/// Identical repeats after which the turn stops.
pub const STOP_AFTER: u32 = 6;

/// What the guard concluded after observing one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopVerdict {
    /// Nothing suspicious.
    Fine,
    /// The call has repeated identically [`NUDGE_AFTER`] times — nudge once.
    Nudge,
    /// The call has repeated identically [`STOP_AFTER`] times — end the turn.
    Stop { name: String, repeats: u32 },
}

/// Per-turn tracker of consecutive identical (tool, arguments, result) calls.
#[derive(Debug, Default)]
pub struct LoopGuard {
    last: Option<u64>,
    last_name: String,
    consecutive: u32,
}

impl LoopGuard {
    /// Observe one executed tool call and its result.
    pub fn observe(&mut self, name: &str, arguments: &str, result: &str) -> LoopVerdict {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (name, canonical_arguments(arguments), result).hash(&mut hasher);
        let key = hasher.finish();
        if self.last == Some(key) {
            self.consecutive += 1;
        } else {
            self.last = Some(key);
            self.last_name = name.to_string();
            self.consecutive = 1;
        }
        if self.consecutive >= STOP_AFTER {
            LoopVerdict::Stop {
                name: self.last_name.clone(),
                repeats: self.consecutive,
            }
        } else if self.consecutive == NUDGE_AFTER {
            LoopVerdict::Nudge
        } else {
            LoopVerdict::Fine
        }
    }
}

/// Arguments in a canonical form — parsed and re-serialized, so key order
/// and whitespace differences between two otherwise identical calls (which
/// models produce freely) don't hide a repeat. Unparseable arguments hash
/// as written.
fn canonical_arguments(arguments: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(value) => canonical_json(&value),
        Err(_) => arguments.to_string(),
    }
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical_json(&map[k])
                    )
                })
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        serde_json::Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_order_and_whitespace_do_not_hide_a_repeat() {
        let mut guard = LoopGuard::default();
        assert_eq!(
            guard.observe("t", r#"{"a":1,"b":2}"#, "r"),
            LoopVerdict::Fine
        );
        assert_eq!(
            guard.observe("t", r#"{ "b": 2, "a": 1 }"#, "r"),
            LoopVerdict::Fine
        );
        assert_eq!(
            guard.observe("t", r#"{"b":2,"a":1}"#, "r"),
            LoopVerdict::Nudge
        );
    }

    #[test]
    fn identical_repeats_nudge_then_stop() {
        let mut guard = LoopGuard::default();
        assert_eq!(
            guard.observe("run_shell", "{\"command\":\"ls\"}", "a"),
            LoopVerdict::Fine
        );
        assert_eq!(
            guard.observe("run_shell", "{\"command\":\"ls\"}", "a"),
            LoopVerdict::Fine
        );
        // Third identical triple → one nudge…
        assert_eq!(
            guard.observe("run_shell", "{\"command\":\"ls\"}", "a"),
            LoopVerdict::Nudge
        );
        // …not re-issued while the count climbs…
        assert_eq!(
            guard.observe("run_shell", "{\"command\":\"ls\"}", "a"),
            LoopVerdict::Fine
        );
        assert_eq!(
            guard.observe("run_shell", "{\"command\":\"ls\"}", "a"),
            LoopVerdict::Fine
        );
        // …and the sixth ends the turn.
        assert_eq!(
            guard.observe("run_shell", "{\"command\":\"ls\"}", "a"),
            LoopVerdict::Stop {
                name: "run_shell".into(),
                repeats: 6
            }
        );
    }

    #[test]
    fn changing_results_or_arguments_reset_the_run() {
        let mut guard = LoopGuard::default();
        // Polling with advancing output never trips the guard.
        for i in 0..10 {
            let result = format!("output chunk {i}");
            assert_eq!(
                guard.observe("task_output", "{\"id\":\"t1\"}", &result),
                LoopVerdict::Fine
            );
        }
        // Two identical calls, then a different one, then identical again:
        // the interleaving resets the count.
        guard.observe("read_file", "{\"path\":\"a\"}", "x");
        guard.observe("read_file", "{\"path\":\"a\"}", "x");
        assert_eq!(
            guard.observe("edit_file", "{\"path\":\"a\"}", "ok"),
            LoopVerdict::Fine
        );
        assert_eq!(
            guard.observe("read_file", "{\"path\":\"a\"}", "x"),
            LoopVerdict::Fine
        );
    }
}
