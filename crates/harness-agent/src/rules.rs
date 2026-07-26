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

/// Compile a batch of rules from their config form, returning the set that
/// loaded and a note for each that didn't.
///
/// One place decides what a broken rule does — it is skipped, and said so —
/// because a rule that can't compile protects nothing while looking like it
/// does, and both hosts had been deciding that separately.
pub fn compile_all<'a>(
    specs: impl IntoIterator<
        Item = (
            &'a str,
            &'a str,
            &'a [String],
            &'a str,
            bool,
            Option<&'a str>,
        ),
    >,
) -> (RuleSet, Vec<String>) {
    let mut rules = Vec::new();
    let mut skipped = Vec::new();
    for (name, pattern, scopes, message, interrupt, repeat) in specs {
        match Rule::compile(name, pattern, scopes, message, interrupt, repeat) {
            Ok(rule) => rules.push(rule),
            Err(e) => skipped.push(format!("{name}: {e}")),
        }
    }
    (RuleSet::new(rules), skipped)
}

/// A rule the model wrote from a description, with the examples it used to
/// convince itself the pattern works.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct DraftedRule {
    pub name: String,
    pub pattern: String,
    pub scopes: Vec<String>,
    pub message: String,
    pub interrupt: bool,
    /// Something the rule should catch — checked before the draft is offered.
    pub example_match: String,
    /// Something close to it that the rule should *not* catch, which is where
    /// an over-broad pattern gives itself away.
    pub example_miss: String,
}

/// One exchange in a rule-writing conversation, for the model to build on.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DraftTurn {
    /// What the user asked for.
    pub asked: String,
    /// What the model said it did (its note, not the JSON).
    pub said: String,
    /// The rule it produced, as JSON, so a follow-up revises rather than
    /// starts over.
    pub rule: Option<String>,
}

/// A model's reply to a drafting request: what it says it did, and the rule.
#[derive(Debug, Clone)]
pub struct DraftReply {
    /// A sentence or two in the model's own words, shown in the conversation.
    pub note: String,
    pub rule: DraftedRule,
}

impl DraftReply {
    /// Split a reply into the part a person reads and the part the editor
    /// uses. The note is whatever precedes the JSON object.
    pub fn from_model_output(raw: &str) -> Result<Self, String> {
        let rule = DraftedRule::from_model_output(raw)?;
        let note = raw
            .split_once('{')
            .map(|(before, _)| before)
            .unwrap_or(raw)
            .replace("```json", "")
            .replace("```", "")
            .trim()
            .to_string();
        Ok(Self { note, rule })
    }
}

/// Build the user turn for a drafting request, carrying the conversation so a
/// follow-up edits the rule on the table instead of starting fresh.
pub fn draft_prompt(request: &str, history: &[DraftTurn]) -> String {
    if history.is_empty() {
        return request.to_string();
    }
    let mut prompt = String::from("Here is our conversation so far.\n\n");
    for turn in history {
        prompt.push_str(&format!("I asked: {}\n", turn.asked));
        prompt.push_str(&format!("You said: {}\n", turn.said));
        if let Some(rule) = &turn.rule {
            prompt.push_str(&format!("You wrote: {rule}\n"));
        }
        prompt.push('\n');
    }
    prompt.push_str(&format!(
        "Now: {request}\n\nRevise the rule above unless I'm clearly asking for a \
         different one. Keep what still fits."
    ));
    prompt
}

/// The conversation's asks as one line, saved with the rule it produced.
///
/// Reopening a rule should restore what was asked for, and the *last* message
/// is usually a correction — "make it stricter" on its own says nothing about
/// what the rule is for. So the whole thread is kept, in order, as a single
/// request that would produce roughly the same rule if sent again.
pub fn combined_request(history: &[DraftTurn], latest: &str) -> String {
    history
        .iter()
        .map(|turn| turn.asked.trim())
        .chain(std::iter::once(latest.trim()))
        .filter(|ask| !ask.is_empty())
        .collect::<Vec<_>>()
        .join(", then ")
}

/// What to tell a model asked to write a rule.
///
/// The hard parts to get across are the engine's limits (this is Rust's regex,
/// not JavaScript's) and that a rule catches text rather than intent, which is
/// why it must supply a counter-example: a pattern that also matches
/// `example_miss` is over-broad, and we can tell without asking a human.
pub const DRAFT_SYSTEM: &str = "\
You write \"stream rules\" for a coding agent. A rule watches what the agent \
writes and corrects it when a regular expression matches.

Reply with ONE short sentence saying what you're watching for and why, then \
the JSON object. Nothing else — no code fence, no preamble like \"Sure!\".

The sentence is shown to the user as your side of the conversation, so write \
it to them: \"Watching for rm with a path that leaves the project root.\"

The JSON:
{
  \"name\": \"kebab-case-id\",
  \"pattern\": \"a regular expression\",
  \"scopes\": [\"tool\"],
  \"message\": \"what the agent should do instead\",
  \"interrupt\": true,
  \"example_match\": \"a line the pattern must catch\",
  \"example_miss\": \"a similar line it must NOT catch\"
}

Rules for the pattern:
- It is Rust `regex` syntax: NO lookahead/lookbehind ((?=...), (?!...)) and NO \
backreferences. Those fail to compile and the rule silently never fires.
- Escape regex metacharacters that are meant literally: \\.unwrap\\(\\), not .unwrap().
- Prefer narrow and literal. A rule matches text, not intent, so a loose \
pattern fires on innocent mentions and costs the user a wasted turn.

Rules for the rest:
- `scopes`: [\"tool\"] watches the arguments the agent writes into a tool call \
(the earliest place a bad edit or command is visible — use this for anything \
about changes). [\"text\"] watches its prose. Both is [\"tool\", \"text\"].
- `interrupt`: true throws the in-flight reply away so the correction lands \
before the work does — right when landing late means undoing something \
(a force-push, an overwritten file). false lets the reply finish and corrects \
afterwards — right for style notes.
- `message`: say what to do INSTEAD, in one or two sentences, addressed to the \
agent. \"Return a Result instead\" beats \"don't unwrap\".
- `example_miss` must be genuinely similar to `example_match` — that is how \
an over-broad pattern gives itself away.";

impl DraftedRule {
    /// Parse and check a drafted rule, returning why it can't be used rather
    /// than a rule that would never fire.
    ///
    /// The model's own examples do the checking: the pattern must compile,
    /// catch what it says it catches, and leave the near-miss alone. A draft
    /// that fails here is worth another attempt, not a save.
    pub fn from_model_output(raw: &str) -> Result<Self, String> {
        let json = raw
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        let start = json.find('{').ok_or("the model didn't return JSON")?;
        let end = json.rfind('}').ok_or("the model didn't return JSON")?;
        let drafted: Self = serde_json::from_str(&json[start..=end])
            .map_err(|e| format!("the model's JSON didn't parse: {e}"))?;

        if drafted.name.trim().is_empty() {
            return Err("the draft has no name".into());
        }
        if drafted.message.trim().is_empty() {
            return Err("the draft doesn't say what to do instead".into());
        }
        let compiled = Regex::new(&drafted.pattern).map_err(|e| {
            let first = e.to_string().lines().next().unwrap_or_default().to_string();
            format!("the pattern doesn't compile: {first}")
        })?;
        if !compiled.is_match(&drafted.example_match) {
            return Err(format!(
                "the pattern doesn't catch its own example ({:?})",
                drafted.example_match
            ));
        }
        if !drafted.example_miss.is_empty() && compiled.is_match(&drafted.example_miss) {
            return Err(format!(
                "the pattern is too broad — it also catches {:?}, which it shouldn't",
                drafted.example_miss
            ));
        }
        Ok(drafted)
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
        Watcher::new(self.rules.iter())
    }
}

/// Tracks how many times each rule has fired, across a session.
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
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
    fn allows_at(&self, rule: &Rule, round: u32) -> bool {
        match self.fired.get(&rule.name) {
            None => true,
            Some(_) if rule.repeat == Repeat::Once => false,
            Some(&last) => match rule.repeat {
                Repeat::AfterRounds(gap) => round.saturating_sub(last) >= gap,
                Repeat::Once => false,
            },
        }
    }

    fn allows(&self, rule: &Rule) -> bool {
        self.allows_at(rule, self.round)
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

    /// Build a watcher containing only rules whose repeat policy still permits
    /// them to fire. Eligibility is checked before streaming so a spent
    /// interrupting rule cannot cancel and truncate a later response.
    pub fn watcher<'a>(&self, rules: &'a RuleSet) -> Watcher<'a> {
        let upcoming_round = self.round.saturating_add(1);
        Watcher::new(
            rules
                .rules
                .iter()
                .filter(|rule| self.allows_at(rule, upcoming_round)),
        )
    }
}

/// Watches one model call's stream. Accumulates per scope, because a pattern
/// can straddle token boundaries — matching each delta in isolation would miss
/// almost everything worth catching.
pub struct Watcher<'a> {
    rules: Vec<&'a Rule>,
    text: String,
    tool_args: String,
    hits: Vec<RuleHit>,
}

impl<'a> Watcher<'a> {
    fn new(rules: impl IntoIterator<Item = &'a Rule>) -> Self {
        Self {
            rules: rules.into_iter().collect(),
            text: String::new(),
            tool_args: String::new(),
            hits: Vec::new(),
        }
    }

    /// Whether this call has an eligible rule that could abandon its output.
    /// Presentation events are buffered in that case until the reply is known
    /// to be accepted.
    pub fn can_interrupt(&self) -> bool {
        self.rules.iter().any(|rule| rule.interrupt)
    }

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
        for rule in &self.rules {
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
    fn a_spent_interrupting_rule_is_absent_from_future_watchers() {
        let rules = RuleSet::new(vec![rule("no-unwrap", "unwrap", Scope::Text, true)]);
        let mut history = RuleHistory::default();
        let mut first = history.watcher(&rules);
        assert!(first.observe(Scope::Text, "unwrap"));
        assert_eq!(history.admit(first.hits(), &rules).len(), 1);

        let mut later = history.watcher(&rules);
        assert!(!later.can_interrupt());
        assert!(!later.observe(Scope::Text, "unwrap"));
        assert!(later.hits().is_empty());
    }

    #[test]
    fn an_after_rounds_rule_returns_once_the_gap_has_passed() {
        let mut r = rule("no-unwrap", "unwrap", Scope::Text, true);
        r.repeat = Repeat::AfterRounds(3);
        let rules = RuleSet::new(vec![r]);
        let mut history = RuleHistory::default();
        let attempt = |history: &mut RuleHistory| {
            let mut w = history.watcher(&rules);
            w.observe(Scope::Text, "unwrap");
            history.next_round();
            history.admit(w.hits(), &rules).len()
        };

        assert_eq!(attempt(&mut history), 1);
        assert_eq!(attempt(&mut history), 0, "too soon");
        assert_eq!(attempt(&mut history), 0, "still too soon");
        assert_eq!(attempt(&mut history), 1, "gap has passed");
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
    fn a_drafted_rule_is_accepted_when_its_own_examples_hold() {
        let raw = r#"```json
        {"name":"no-force-push","pattern":"push\\s+--force","scopes":["tool"],
         "message":"Don't force-push.","interrupt":true,
         "example_match":"git push --force origin main","example_miss":"git push origin main"}
        ```"#;

        let drafted = DraftedRule::from_model_output(raw).expect("a usable draft");

        assert_eq!(drafted.name, "no-force-push");
        assert!(drafted.interrupt);
        assert_eq!(drafted.scopes, vec!["tool".to_string()]);
    }

    #[test]
    fn a_reply_splits_into_what_the_model_said_and_what_it_wrote() {
        let raw = "Watching for rm with a path that leaves the project root.\n\
                   {\"name\":\"n\",\"pattern\":\"rm .*\\\\.\\\\./\",\"scopes\":[\"tool\"],\
                   \"message\":\"m\",\"interrupt\":true,\
                   \"example_match\":\"rm ../secrets.env\",\"example_miss\":\"rm ./tmp\"}";

        let reply = DraftReply::from_model_output(raw).expect("a usable reply");

        assert_eq!(
            reply.note,
            "Watching for rm with a path that leaves the project root."
        );
        assert_eq!(reply.rule.name, "n");
    }

    #[test]
    fn a_follow_up_carries_the_conversation_so_far() {
        let history = [DraftTurn {
            asked: "don't delete migrations".into(),
            said: "Watching for rm on the migrations directory.".into(),
            rule: Some("{\"name\":\"no-migration-deletes\"}".into()),
        }];

        let prompt = draft_prompt("also catch mv", &history);

        assert!(prompt.contains("don't delete migrations"));
        assert!(prompt.contains("no-migration-deletes"));
        assert!(prompt.contains("Now: also catch mv"));
        assert!(prompt.contains("Revise the rule above"));
        // With no history it's just the request.
        assert_eq!(draft_prompt("x", &[]), "x");
    }

    #[test]
    fn a_saved_prompt_keeps_the_whole_conversation() {
        let history = [
            DraftTurn {
                asked: "  don't delete migrations  ".into(),
                said: String::new(),
                rule: None,
            },
            // A blank ask (the one-click nudges send text, but a stray empty
            // turn shouldn't leave ", then , then " in the saved prompt).
            DraftTurn {
                asked: "  ".into(),
                said: String::new(),
                rule: None,
            },
        ];
        assert_eq!(
            combined_request(&history, "make it stricter"),
            "don't delete migrations, then make it stricter"
        );
        assert_eq!(combined_request(&[], "just this"), "just this");
    }

    #[test]
    fn a_draft_that_misses_its_own_example_is_rejected() {
        let raw = r#"{"name":"n","pattern":"nevermatches","scopes":["tool"],"message":"m",
                      "interrupt":true,"example_match":"git push --force","example_miss":"x"}"#;
        let err = DraftedRule::from_model_output(raw).unwrap_err();
        assert!(err.contains("doesn't catch its own example"), "got: {err}");
    }

    #[test]
    fn an_over_broad_draft_gives_itself_away_on_the_counter_example() {
        // The near-miss is what catches a pattern that matches everything.
        let raw = r#"{"name":"n","pattern":"push","scopes":["tool"],"message":"m",
                      "interrupt":true,"example_match":"git push --force",
                      "example_miss":"git push origin main"}"#;
        let err = DraftedRule::from_model_output(raw).unwrap_err();
        assert!(err.contains("too broad"), "got: {err}");
    }

    #[test]
    fn a_draft_using_lookahead_is_rejected_with_the_reason() {
        let raw = r#"{"name":"n","pattern":"foo(?=bar)","scopes":["tool"],"message":"m",
                      "interrupt":true,"example_match":"foobar","example_miss":"foo"}"#;
        let err = DraftedRule::from_model_output(raw).unwrap_err();
        assert!(err.contains("doesn't compile"), "got: {err}");
    }

    #[test]
    fn junk_from_the_model_is_reported_rather_than_panicking() {
        assert!(DraftedRule::from_model_output("I'd be happy to help!").is_err());
        assert!(DraftedRule::from_model_output("{not json}").is_err());
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
