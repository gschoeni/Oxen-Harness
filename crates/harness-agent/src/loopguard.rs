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
//! produces a different result. Repeats are counted within a recent window,
//! not only back-to-back, so alternating between two fruitless calls is
//! caught as surely as hammering one — except for the polling tools (`POLLING_TOOLS`), whose
//! identical results between unrelated work are the normal shape of
//! checking on a quiet task; those count only when consecutive.

use std::hash::{Hash, Hasher};

/// Tools whose job is to be called again with the same arguments while
/// something else happens (a task that has produced no new output returns
/// byte-identical "running" results). Interleaved with other work, their
/// repeats carry no loop signal, so only back-to-back repeats count.
const POLLING_TOOLS: &[&str] = &[harness_tools::TASK_OUTPUT_TOOL];

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

/// How many recent calls the guard remembers. Wide enough that an A/B/A/B
/// oscillation (two calls alternating, each learning nothing) reaches the
/// stop line, narrow enough that a legitimately repeated check a dozen
/// calls apart doesn't count against the model.
const WINDOW: usize = 12;

/// Per-turn tracker of identical (tool, arguments, result) calls in the
/// recent window — consecutive or interleaved with other calls.
#[derive(Debug, Default)]
pub struct LoopGuard {
    recent: std::collections::VecDeque<(u64, String)>,
}

impl LoopGuard {
    /// Observe one executed tool call and its result.
    pub fn observe(&mut self, name: &str, arguments: &str, result: &str) -> LoopVerdict {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (name, canonical_arguments(arguments), result).hash(&mut hasher);
        let key = hasher.finish();
        if self.recent.len() == WINDOW {
            self.recent.pop_front();
        }
        self.recent.push_back((key, name.to_string()));
        let repeats = if POLLING_TOOLS.contains(&name) {
            // Only the trailing run: a poll between two unrelated edits is
            // purposeful even when the task has been quiet the whole time.
            self.recent
                .iter()
                .rev()
                .take_while(|(k, _)| *k == key)
                .count() as u32
        } else {
            self.recent.iter().filter(|(k, _)| *k == key).count() as u32
        };
        if repeats >= STOP_AFTER {
            LoopVerdict::Stop {
                name: name.to_string(),
                repeats,
            }
        } else if repeats == NUDGE_AFTER {
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
    fn an_oscillation_between_two_calls_is_caught() {
        let mut guard = LoopGuard::default();
        let mut verdicts = Vec::new();
        for i in 0..12 {
            let (name, result) = if i % 2 == 0 {
                ("read", "same")
            } else {
                ("grep", "nothing")
            };
            verdicts.push(guard.observe(name, "{}", result));
        }
        // The 5th call is the 3rd `read`: a nudge. The 11th is the 6th: stop.
        assert_eq!(verdicts[4], LoopVerdict::Nudge);
        assert!(
            matches!(verdicts[10], LoopVerdict::Stop { ref name, repeats: 6 } if name == "read")
        );
        assert!(verdicts[..4].iter().all(|v| *v == LoopVerdict::Fine));
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

    /// Watching a quiet build: `task_output` returns the same "running,
    /// nothing new" result each time, interleaved with real edits. Six such
    /// polls inside the window used to stop the turn; now only hammering
    /// the poll back-to-back does.
    #[test]
    fn interleaved_polls_of_a_quiet_task_are_not_a_loop() {
        let mut guard = LoopGuard::default();
        let poll = |g: &mut LoopGuard| g.observe("task_output", r#"{"task_id":1}"#, "running");
        for i in 0..8 {
            assert_eq!(poll(&mut guard), LoopVerdict::Fine, "poll {i}");
            assert_eq!(
                guard.observe("edit_file", &format!(r#"{{"path":"f{i}"}}"#), "ok"),
                LoopVerdict::Fine
            );
        }
        // Back-to-back identical polls are still the classic loop.
        let mut guard = LoopGuard::default();
        let mut verdicts = Vec::new();
        for _ in 0..6 {
            verdicts.push(poll(&mut guard));
        }
        assert_eq!(verdicts[2], LoopVerdict::Nudge);
        assert!(matches!(verdicts[5], LoopVerdict::Stop { repeats: 6, .. }));
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
        // Read, read, edit, read: the edit changes what the read returns, so
        // the third read is a different (tool, args, result) and no repeat.
        guard.observe("read_file", "{\"path\":\"a\"}", "x");
        guard.observe("read_file", "{\"path\":\"a\"}", "x");
        assert_eq!(
            guard.observe("edit_file", "{\"path\":\"a\"}", "ok"),
            LoopVerdict::Fine
        );
        assert_eq!(
            guard.observe("read_file", "{\"path\":\"a\"}", "y"),
            LoopVerdict::Fine
        );
        // …whereas a third read returning the very same bytes is a repeat,
        // interleaving or not.
        assert_eq!(
            guard.observe("read_file", "{\"path\":\"a\"}", "x"),
            LoopVerdict::Nudge
        );
    }
}
