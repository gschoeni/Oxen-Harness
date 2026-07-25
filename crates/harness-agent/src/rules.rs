//! Stream rules: corrections that fire while the model is still writing.
//!
//! The harness already carries three hard-coded correctives — "you announced
//! an action but never called a tool", "your plan still has open items", "you
//! called the same tool with the same arguments again" (see [`crate::prompt`]).
//! Each is a rule someone decided was worth a round trip. Users have their own:
//! *never `.unwrap()` in this crate*, *don't touch `generated/`*, *stop writing
//! migrations by hand*. Putting those in the system prompt makes every request
//! pay for them whether or not the situation arises.
//!
//! A stream rule costs nothing until it matches. It watches the tokens as they
//! arrive; when one matches, the rule's text is injected as a one-shot
//! reminder — the same channel the built-in nudges use — and, for an
//! interrupting rule, the in-flight stream is abandoned first so the
//! correction lands *before* a bad edit is finished rather than after it has
//! been applied. (The idea is oh-my-pi's "time-traveling stream rules"; this is
//! a smaller version of it: regex, two scopes, no AST matching.)
//!
//! What a rule cannot do: change the model's mind silently. Every injection is
//! visible in the transcript as the reminder it is, and every rule fires a
//! bounded number of times per session.

use std::collections::HashMap;

use regex::Regex;

/// Where a rule watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The assistant's prose as it streams.
    Text,
    /// A tool call's arguments as they stream — so a rule can catch a bad edit
    /// while it is being written, before it is applied.
    ToolArguments,
}

/// How often a rule may fire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Repeat {
    /// Once per session. The default: a reminder the model has already seen
    /// and ignored twice is noise, not steering.
    #[default]
    Once,
    /// Again, but only after this many further rounds.
    AfterRounds(u32),
}

/// One compiled rule.
#[derive(Debug, Clone)]
pub struct Rule {
    /// Stable identity, for repeat tracking and for naming the rule in the
    /// injected reminder.
    pub name: String,
    /// What to watch for.
    pub pattern: Regex,
    pub scopes: Vec<Scope>,
    /// The correction handed to the model.
    pub message: String,
    /// Whether matching abandons the in-flight response. On by default: the
    /// point is to correct before the output lands, not after.
    pub interrupt: bool,
    pub repeat: Repeat,
}

/// A rule that fired, and what to tell the model about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleHit {
    pub name: String,
    pub message: String,
    pub interrupt: bool,
}

impl RuleHit {
    /// The reminder as the model sees it. Framed as a system reminder rather
    /// than a user message: the user did not just say this, and a transcript
    /// that pretends otherwise misleads whoever reads it back.
    pub fn reminder(&self) -> String {
        format!(
            "<system-reminder rule=\"{}\">\n{}\n</system-reminder>",
            self.name,
            self.message.trim()
        )
    }
}

impl Rule {
    /// Compile a rule from the plain strings a config file carries.
    ///
    /// `scopes` accepts `"text"`, `"tool"`, or nothing (meaning both);
    /// `repeat` accepts `"once"` (the default) or `"after:<rounds>"`. An
    /// unparseable regex is the caller's to report — a bad rule in a shared
    /// repository file must not stop a session from starting.
    pub fn compile(
        name: impl Into<String>,
        pattern: &str,
        scopes: &[String],
        message: impl Into<String>,
        interrupt: bool,
        repeat: Option<&str>,
    ) -> Result<Self, regex::Error> {
        let scopes = if scopes.is_empty() {
            vec![Scope::Text, Scope::ToolArguments]
        } else {
            let recognized: Vec<Scope> = scopes
                .iter()
                .filter_map(|s| match s.trim().to_ascii_lowercase().as_str() {
                    "text" | "prose" => Some(Scope::Text),
                    "tool" | "tools" | "tool_arguments" => Some(Scope::ToolArguments),
                    _ => None,
                })
                .collect();
            // A rule that watches nothing loads, lists, and never fires — the
            // failure this whole module is built to avoid. A typo'd scope is
            // reported like a bad pattern instead.
            if recognized.is_empty() {
                return Err(regex::Error::Syntax(format!(
                    "unknown scope {:?} — use \"text\", \"tool\", or omit it for both",
                    scopes.join(", ")
                )));
            }
            recognized
        };
        Ok(Self {
            name: name.into(),
            pattern: Regex::new(pattern)?,
            scopes,
            message: message.into(),
            interrupt,
            repeat: match repeat.map(str::trim) {
                Some(spec) => spec
                    .strip_prefix("after:")
                    .and_then(|n| n.parse().ok())
                    .map(Repeat::AfterRounds)
                    .unwrap_or(Repeat::Once),
                None => Repeat::Once,
            },
        })
    }
}

/// What a pattern does against a sample, for an editor that wants to show it
/// before the rule is ever live.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PatternCheck {
    /// Why the pattern doesn't compile, if it doesn't.
    pub error: Option<String>,
    /// Byte ranges the pattern matches in the sample, in order.
    pub matches: Vec<(usize, usize)>,
}

/// Compile `pattern` and find every match in `sample`.
///
/// This exists so a rules editor can show what a rule will actually do. It
/// must run through *this* engine rather than the UI's own: JavaScript and
/// Rust regexes differ in ways that matter here (Rust's has no lookahead or
/// backreferences), so a browser-side preview would happily accept patterns
/// the agent then rejects, and the first the user heard of it would be a rule
/// that silently never fires.
pub fn check_pattern(pattern: &str, sample: &str) -> PatternCheck {
    match Regex::new(pattern) {
        Err(e) => PatternCheck {
            // regex's errors are multi-line and quote the pattern back; the
            // first non-empty line is the part a person needs.
            error: Some(
                e.to_string()
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("invalid pattern")
                    .trim()
                    .to_string(),
            ),
            matches: Vec::new(),
        },
        Ok(re) => PatternCheck {
            error: None,
            matches: re.find_iter(sample).map(|m| (m.start(), m.end())).collect(),
        },
    }
}

/// The rules in force for a session.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// A watcher for one model call. Cheap to make; holds the per-call buffers.
    pub fn watcher(&self) -> Watcher<'_> {
        Watcher {
            rules: &self.rules,
            text: String::new(),
            tool_args: String::new(),
            hits: Vec::new(),
        }
    }
}

/// Tracks how many times each rule has fired, across a session.
#[derive(Debug, Default)]
pub struct RuleHistory {
    /// Rule name → the round index it last fired on.
    fired: HashMap<String, u32>,
    round: u32,
}

impl RuleHistory {
    /// Advance the round counter (one model call = one round).
    pub fn next_round(&mut self) {
        self.round = self.round.saturating_add(1);
    }

    /// Whether `rule` may fire now, given how recently it last did.
    fn allows(&self, rule: &Rule) -> bool {
        match self.fired.get(&rule.name) {
            None => true,
            Some(_) if rule.repeat == Repeat::Once => false,
            Some(&last) => match rule.repeat {
                Repeat::AfterRounds(gap) => self.round.saturating_sub(last) >= gap,
                Repeat::Once => false,
            },
        }
    }

    fn record(&mut self, name: &str) {
        self.fired.insert(name.to_string(), self.round);
    }

    /// Filter `hits` to the rules allowed to fire now, marking them fired.
    pub fn admit(&mut self, hits: Vec<RuleHit>, rules: &RuleSet) -> Vec<RuleHit> {
        let mut out = Vec::new();
        for hit in hits {
            let Some(rule) = rules.rules.iter().find(|r| r.name == hit.name) else {
                continue;
            };
            if self.allows(rule) {
                self.record(&hit.name);
                out.push(hit);
            }
        }
        out
    }
}

/// Watches one model call's stream. Accumulates per scope, because a pattern
/// can straddle token boundaries — matching each delta in isolation would miss
/// almost everything worth catching.
pub struct Watcher<'a> {
    rules: &'a [Rule],
    text: String,
    tool_args: String,
    hits: Vec<RuleHit>,
}

impl Watcher<'_> {
    /// Feed a streamed fragment. Returns true when an *interrupting* rule
    /// matched, which is the caller's signal to abandon the response.
    pub fn observe(&mut self, scope: Scope, delta: &str) -> bool {
        let buffer = match scope {
            Scope::Text => &mut self.text,
            Scope::ToolArguments => &mut self.tool_args,
        };
        buffer.push_str(delta);
        // Matching the whole buffer each time is O(n²) in the worst case, but
        // n is one reply and the regexes are small; a streaming matcher would
        // be a lot of machinery for a fraction of a millisecond.
        let mut interrupt = false;
        for rule in self.rules {
            if !rule.scopes.contains(&scope) {
                continue;
            }
            if self.hits.iter().any(|h| h.name == rule.name) {
                continue;
            }
            let matched = match scope {
                Scope::Text => rule.pattern.is_match(&self.text),
                Scope::ToolArguments => rule.pattern.is_match(&self.tool_args),
            };
            if matched {
                self.hits.push(RuleHit {
                    name: rule.name.clone(),
                    message: rule.message.clone(),
                    interrupt: rule.interrupt,
                });
                interrupt |= rule.interrupt;
            }
        }
        interrupt
    }

    /// Everything that matched during this call.
    pub fn hits(self) -> Vec<RuleHit> {
        self.hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, pattern: &str, scope: Scope, interrupt: bool) -> Rule {
        Rule {
            name: name.into(),
            pattern: Regex::new(pattern).unwrap(),
            scopes: vec![scope],
            message: format!("do not {name}"),
            interrupt,
            repeat: Repeat::Once,
        }
    }

    #[test]
    fn a_pattern_split_across_deltas_still_matches() {
        let rules = RuleSet::new(vec![rule("no-unwrap", r"\.unwrap\(\)", Scope::Text, true)]);
        let mut watcher = rules.watcher();

        // The tokens a model actually emits do not respect the pattern.
        assert!(!watcher.observe(Scope::Text, "let x = foo"));
        assert!(!watcher.observe(Scope::Text, ".unw"));
        let interrupt = watcher.observe(Scope::Text, "rap()");

        assert!(
            interrupt,
            "the rule should fire once the buffer completes it"
        );
        assert_eq!(watcher.hits().len(), 1);
    }

    #[test]
    fn scopes_are_separate_buffers() {
        let rules = RuleSet::new(vec![rule(
            "no-generated",
            "generated/",
            Scope::ToolArguments,
            true,
        )]);
        let mut watcher = rules.watcher();

        // Prose mentioning the path is fine; writing to it is not.
        assert!(!watcher.observe(Scope::Text, "the generated/ directory is built"));
        assert!(watcher.observe(Scope::ToolArguments, r#"{"path": "generated/api.ts""#));
        assert_eq!(watcher.hits().len(), 1);
    }

    #[test]
    fn a_non_interrupting_rule_records_without_abandoning_the_stream() {
        let rules = RuleSet::new(vec![rule("style", "TODO", Scope::Text, false)]);
        let mut watcher = rules.watcher();

        assert!(!watcher.observe(Scope::Text, "adding a TODO here"));

        let hits = watcher.hits();
        assert_eq!(hits.len(), 1);
        assert!(!hits[0].interrupt);
    }

    #[test]
    fn a_rule_fires_once_per_call_however_often_it_matches() {
        let rules = RuleSet::new(vec![rule("no-unwrap", r"unwrap", Scope::Text, true)]);
        let mut watcher = rules.watcher();

        watcher.observe(Scope::Text, "unwrap");
        watcher.observe(Scope::Text, " and unwrap again");

        assert_eq!(watcher.hits().len(), 1);
    }

    #[test]
    fn once_means_once_per_session() {
        let rules = RuleSet::new(vec![rule("no-unwrap", "unwrap", Scope::Text, true)]);
        let mut history = RuleHistory::default();
        let hit = || {
            let mut w = rules.watcher();
            w.observe(Scope::Text, "unwrap");
            w.hits()
        };

        assert_eq!(history.admit(hit(), &rules).len(), 1);
        history.next_round();
        // The model has seen this reminder; repeating it is noise.
        assert!(history.admit(hit(), &rules).is_empty());
    }

    #[test]
    fn an_after_rounds_rule_returns_once_the_gap_has_passed() {
        let mut r = rule("no-unwrap", "unwrap", Scope::Text, true);
        r.repeat = Repeat::AfterRounds(3);
        let rules = RuleSet::new(vec![r]);
        let mut history = RuleHistory::default();
        let hit = || {
            let mut w = rules.watcher();
            w.observe(Scope::Text, "unwrap");
            w.hits()
        };

        assert_eq!(history.admit(hit(), &rules).len(), 1);
        history.next_round();
        assert!(history.admit(hit(), &rules).is_empty(), "too soon");
        history.next_round();
        history.next_round();
        assert_eq!(history.admit(hit(), &rules).len(), 1, "gap has passed");
    }

    #[test]
    fn the_reminder_names_the_rule_and_frames_itself_as_a_reminder() {
        let hit = RuleHit {
            name: "no-unwrap".into(),
            message: "Return a Result instead.".into(),
            interrupt: true,
        };
        let text = hit.reminder();
        assert!(text.contains("rule=\"no-unwrap\""));
        assert!(text.contains("Return a Result instead."));
        // Not framed as something the user said.
        assert!(text.starts_with("<system-reminder"));
    }

    #[test]
    fn compiling_from_config_strings_maps_scopes_and_repeat() {
        let both = Rule::compile("r", "x", &[], "m", true, None).unwrap();
        // No scope stated means watch everything.
        assert_eq!(both.scopes, vec![Scope::Text, Scope::ToolArguments]);
        assert_eq!(both.repeat, Repeat::Once);

        let tool_only =
            Rule::compile("r", "x", &["tool".to_string()], "m", false, Some("after:5")).unwrap();
        assert_eq!(tool_only.scopes, vec![Scope::ToolArguments]);
        assert_eq!(tool_only.repeat, Repeat::AfterRounds(5));
        assert!(!tool_only.interrupt);

        // An unreadable repeat degrades to the documented default…
        let odd = Rule::compile("r", "x", &[], "m", true, Some("soon")).unwrap();
        assert_eq!(odd.repeat, Repeat::Once);
        // …but a scope nobody recognizes is an error, not a rule that loads
        // and silently never fires.
        let bad_scope = Rule::compile("r", "x", &["tool_args".into()], "m", true, None);
        assert!(bad_scope.is_err(), "a typo'd scope must be reported");

        assert!(Rule::compile("r", "(unclosed", &[], "m", true, None).is_err());
    }

    #[test]
    fn checking_a_pattern_reports_matches_or_a_readable_error() {
        let sample = "let a = x.unwrap(); let b = y.unwrap();";
        let hit = check_pattern(r"\.unwrap\(\)", sample);
        assert!(hit.error.is_none());
        assert_eq!(hit.matches.len(), 2);
        assert_eq!(&sample[hit.matches[0].0..hit.matches[0].1], ".unwrap()");

        let bad = check_pattern("(unclosed", "anything");
        assert!(bad.matches.is_empty());
        let message = bad.error.expect("an error");
        // One line, not the engine's multi-line dump.
        assert!(!message.contains('\n'), "got: {message}");
    }

    #[test]
    fn a_pattern_javascript_would_accept_is_still_rejected_here() {
        // Lookahead is valid in a browser and not in this engine. A preview
        // built on the browser's regex would call this fine, and the rule
        // would then silently never fire.
        assert!(check_pattern(r"foo(?=bar)", "foobar").error.is_some());
    }

    #[test]
    fn no_rules_means_no_work() {
        let rules = RuleSet::default();
        let mut watcher = rules.watcher();
        assert!(!watcher.observe(Scope::Text, "anything at all .unwrap()"));
        assert!(watcher.hits().is_empty());
    }
}
