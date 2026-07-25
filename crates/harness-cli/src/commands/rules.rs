//! The `/rules` command: see, add, toggle, and test stream rules.
//!
//! A rule watches what the model writes and corrects it the moment a pattern
//! matches (see `harness_agent::rules`). The desktop has a page for this; the
//! terminal gets the same capabilities as subcommands, including the part that
//! matters most — trying a pattern against sample text before trusting it.
//!
//! Every pattern here is checked by the agent's own regex engine rather than
//! being taken on faith, so a rule that would never fire is caught while the
//! user is still writing it rather than in silence three sessions later.

use std::path::Path;

use anyhow::Result;
use harness_agent::rules::{check_pattern, Rule, RuleSet};
use harness_agent::Agent;
use harness_runtime::rules::{self, RuleSpec, Rules};

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// `/rules [add|on <name>|off <name>|rm <name>|test <name>]` — with no
/// argument, print what's in force.
///
/// Edits apply to the live session immediately (the agent's rule set is
/// replaced in place) *and* persist, so a rule written mid-session starts
/// watching the next reply rather than the next chat.
pub(crate) fn handle_repl(
    rest: Option<String>,
    agent: &mut Agent,
    ui: &Ui,
    workspace_root: &Path,
) -> Result<()> {
    let (verb, arg) = split(rest.as_deref());
    match verb {
        None | Some("list") => list(ui, workspace_root),
        Some("add") | Some("new") => add(agent, ui, workspace_root)?,
        Some("on") | Some("off") => {
            toggle(agent, ui, workspace_root, arg, verb == Some("on"))?;
        }
        Some("rm") | Some("remove") | Some("delete") => remove(agent, ui, workspace_root, arg)?,
        Some("test") => test(ui, arg)?,
        Some(other) => {
            println!("  {}", ui.dim(&format!("unknown: /rules {other}")));
            println!(
                "  {}",
                ui.dim("try /rules, /rules add, /rules on|off <name>, /rules rm <name>, /rules test <name>")
            );
        }
    }
    Ok(())
}

/// Split `on no-unwrap` into ("on", Some("no-unwrap")).
fn split(rest: Option<&str>) -> (Option<&str>, Option<&str>) {
    let rest = rest.map(str::trim).filter(|r| !r.is_empty());
    match rest {
        None => (None, None),
        Some(rest) => match rest.split_once(char::is_whitespace) {
            Some((verb, arg)) => (Some(verb), Some(arg.trim())),
            None => (Some(rest), None),
        },
    }
}

fn list(ui: &Ui, workspace_root: &Path) {
    let user = rules::user_rules().rules;
    let project = rules::project_rules(workspace_root).rules;
    if user.is_empty() && project.is_empty() {
        println!(
            "  {}",
            ui.dim("no rules yet — the model is only held to its own guidelines")
        );
        println!(
            "  {}",
            ui.dim("write one with /rules add: a pattern to watch for, and what to say when it matches")
        );
        return;
    }

    if !user.is_empty() {
        println!("  {}", ui.brown("your rules:"));
        for rule in &user {
            print_rule(ui, rule, false);
        }
    }
    if !project.is_empty() {
        println!("  {}", ui.brown("this project's rules:"));
        for rule in &project {
            print_rule(ui, rule, true);
        }
        println!(
            "  {}",
            ui.dim(&format!(
                "committed in {} — edit the file to change them",
                rules::PROJECT_RULES_FILE
            ))
        );
    }
    println!(
        "  {}",
        ui.dim("/rules add · /rules on|off <name> · /rules rm <name> · /rules test <name>")
    );
}

fn print_rule(ui: &Ui, rule: &RuleSpec, from_project: bool) {
    let effect = if rule.interrupt {
        ui.red("interrupts")
    } else {
        ui.dim("reminds")
    };
    let state = if rule.enabled { "" } else { "  (off)" };
    println!(
        "    {} {} {}{}",
        ui.accent(&format!("{:<20}", rule.name)),
        effect,
        ui.dim(&format!("in {}", scope_label(&rule.scope))),
        ui.red(state),
    );
    println!("      {}", ui.cream(&rule.pattern));
    println!("      {}", ui.dim(&clip(&rule.message, 92)));
    let _ = from_project;
}

fn scope_label(scope: &[String]) -> String {
    if scope.is_empty() {
        return "prose + tool calls".into();
    }
    scope
        .iter()
        .map(|s| match s.as_str() {
            "tool" => "tool calls",
            "text" => "prose",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn clip(text: &str, max: usize) -> String {
    harness_core::text::truncate_with_marker(text, max, "…")
}

/// The guided create flow: name, pattern, message, where it watches, what it
/// does — then a chance to try it before it goes live.
fn add(agent: &mut Agent, ui: &Ui, workspace_root: &Path) -> Result<()> {
    let Some(name) = ask(ui, "Rule name", "A short name, e.g. no-unwrap", "", &[])? else {
        return Ok(());
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        println!("  {}", ui.dim("cancelled — a rule needs a name"));
        return Ok(());
    }

    // Re-ask until the pattern compiles: a rule that never fires is worse than
    // no rule, because it looks like protection.
    let pattern = loop {
        let Some(pattern) = ask(
            ui,
            "Watch for",
            "A regular expression to watch the model's output for",
            "",
            &["e.g. \\.unwrap\\(\\)  ·  generated/  ·  push\\s+--force".to_string()],
        )?
        else {
            return Ok(());
        };
        let pattern = pattern.trim().to_string();
        if pattern.is_empty() {
            println!(
                "  {}",
                ui.dim("cancelled — a rule needs something to watch for")
            );
            return Ok(());
        }
        match check_pattern(&pattern, "").error {
            None => break pattern,
            Some(e) => println!(
                "  {} {}",
                ui.red("that pattern doesn't compile:"),
                ui.dim(&e)
            ),
        }
    };

    let Some(message) = ask(
        ui,
        "Tell the model",
        "What should it do instead?",
        "",
        &["This is injected as a reminder when the pattern matches.".to_string()],
    )?
    else {
        return Ok(());
    };

    let scope = match picker::select(
        ui,
        "Watches",
        "Where should this rule watch?",
        &[
            Choice::new("tool calls", "arguments the model writes into a tool call"),
            Choice::new("prose", "what it says in the reply"),
            Choice::new("both", "everything it writes"),
        ],
        false,
    )? {
        Some(picked) => match picked.first().map(String::as_str) {
            Some("prose") => vec!["text".to_string()],
            Some("both") => vec![],
            _ => vec!["tool".to_string()],
        },
        None => vec!["tool".to_string()],
    };

    let interrupt = matches!(
        picker::select(
            ui,
            "On a match",
            "What should happen when it matches?",
            &[
                Choice::new(
                    "interrupt",
                    "throw the reply away and ask again — the correction lands before the work",
                ),
                Choice::new(
                    "remind",
                    "let the reply finish, then correct before the next step"
                ),
            ],
            false,
        )?
        .and_then(|p| p.first().cloned())
        .as_deref(),
        Some("interrupt") | None
    );

    let spec = RuleSpec {
        name,
        pattern,
        scope,
        message: message.trim().to_string(),
        interrupt,
        repeat: Some("once".into()),
        enabled: true,
    };

    // Offer a dry run before it goes live — the terminal's version of the
    // desktop tester.
    if let Some(sample) = ask(
        ui,
        "Try it",
        "Paste something the model might write (Enter to skip)",
        "",
        &[],
    )? {
        if !sample.trim().is_empty() {
            report_matches(ui, &spec.pattern, &sample);
        }
    }

    let mut saved = rules::user_rules();
    saved.rules.retain(|r| r.name != spec.name);
    saved.rules.push(spec.clone());
    persist(agent, ui, workspace_root, saved)?;
    println!(
        "  {} {}",
        ui.green("⚖ rule added:"),
        ui.cream(&format!(
            "{} — {} in {}",
            spec.name,
            if spec.interrupt {
                "interrupts"
            } else {
                "reminds"
            },
            scope_label(&spec.scope)
        )),
    );
    Ok(())
}

fn toggle(
    agent: &mut Agent,
    ui: &Ui,
    workspace_root: &Path,
    name: Option<&str>,
    on: bool,
) -> Result<()> {
    let Some(name) = name else {
        println!("  {}", ui.dim("which rule? /rules on <name>"));
        return Ok(());
    };
    let mut saved = rules::user_rules();
    let Some(rule) = saved.rules.iter_mut().find(|r| r.name == name) else {
        println!("  {}", ui.dim(&format!("no rule named {name}")));
        return Ok(());
    };
    rule.enabled = on;
    persist(agent, ui, workspace_root, saved)?;
    println!(
        "  {} {}",
        ui.green(if on { "⚖ rule on:" } else { "⚖ rule off:" }),
        ui.cream(name)
    );
    Ok(())
}

fn remove(agent: &mut Agent, ui: &Ui, workspace_root: &Path, name: Option<&str>) -> Result<()> {
    let Some(name) = name else {
        println!("  {}", ui.dim("which rule? /rules rm <name>"));
        return Ok(());
    };
    let mut saved = rules::user_rules();
    let before = saved.rules.len();
    saved.rules.retain(|r| r.name != name);
    if saved.rules.len() == before {
        println!("  {}", ui.dim(&format!("no rule named {name}")));
        return Ok(());
    }
    persist(agent, ui, workspace_root, saved)?;
    println!("  {} {}", ui.green("⚖ rule removed:"), ui.cream(name));
    Ok(())
}

/// `/rules test <name>` — run a saved rule's pattern against text you paste.
fn test(ui: &Ui, name: Option<&str>) -> Result<()> {
    let Some(name) = name else {
        println!("  {}", ui.dim("which rule? /rules test <name>"));
        return Ok(());
    };
    let Some(rule) = rules::user_rules()
        .rules
        .into_iter()
        .find(|r| r.name == name)
    else {
        println!("  {}", ui.dim(&format!("no rule named {name}")));
        return Ok(());
    };
    let Some(sample) = ask(
        ui,
        "Try it",
        &format!("Paste something to test {name} against"),
        "",
        &[format!("watching for {}", rule.pattern)],
    )?
    else {
        return Ok(());
    };
    report_matches(ui, &rule.pattern, &sample);
    Ok(())
}

/// Print what a pattern does to a sample, through the agent's own engine.
fn report_matches(ui: &Ui, pattern: &str, sample: &str) {
    let check = check_pattern(pattern, sample);
    if let Some(error) = check.error {
        println!(
            "  {} {}",
            ui.red("that pattern doesn't compile:"),
            ui.dim(&error)
        );
        return;
    }
    if check.matches.is_empty() {
        println!("  {}", ui.dim("no match — the model would carry on"));
        return;
    }
    println!(
        "  {} {}",
        ui.red(&format!(
            "{} match{}",
            check.matches.len(),
            if check.matches.len() == 1 { "" } else { "es" }
        )),
        ui.dim("— this rule would fire"),
    );
    for (start, end) in check.matches.iter().take(5) {
        if let Some(hit) = sample.get(*start..*end) {
            println!("    {}", ui.cream(hit));
        }
    }
}

/// Save the user's rules and install them on the live agent, so a rule written
/// mid-session watches the next reply rather than the next chat.
fn persist(agent: &mut Agent, ui: &Ui, workspace_root: &Path, saved: Rules) -> Result<()> {
    if let Err(e) = rules::save(&saved) {
        println!(
            "  {} {}",
            ui.red("couldn't save the rules:"),
            ui.dim(&e.to_string())
        );
        return Ok(());
    }
    agent.set_rules(compile(workspace_root, ui));
    Ok(())
}

/// The workspace's rules, compiled. A pattern that no longer compiles is
/// skipped with a note rather than taking the session down.
fn compile(workspace_root: &Path, ui: &Ui) -> RuleSet {
    let compiled = rules::load(workspace_root)
        .into_iter()
        .filter_map(|spec| {
            match Rule::compile(
                &spec.name,
                &spec.pattern,
                &spec.scope,
                &spec.message,
                spec.interrupt,
                spec.repeat.as_deref(),
            ) {
                Ok(rule) => Some(rule),
                Err(e) => {
                    println!(
                        "  {} {}",
                        ui.dim(&format!("skipping rule {}:", spec.name)),
                        ui.dim(&e.to_string())
                    );
                    None
                }
            }
        })
        .collect();
    RuleSet::new(compiled)
}

/// One card-framed question. `None` means the user cancelled or there's no
/// interactive terminal.
fn ask(
    ui: &Ui,
    header: &str,
    title: &str,
    initial: &str,
    help: &[String],
) -> Result<Option<String>> {
    Ok(picker::card_input(
        ui,
        &picker::CardInputSpec {
            header,
            title,
            help,
            prompt: "❯",
            initial,
            mask: false,
            collapse_label: "",
        },
    )?
    .entered())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subcommands_split_from_their_argument() {
        assert_eq!(split(None), (None, None));
        assert_eq!(split(Some("  ")), (None, None));
        assert_eq!(split(Some("add")), (Some("add"), None));
        assert_eq!(split(Some("on no-unwrap")), (Some("on"), Some("no-unwrap")));
        // A name with spaces stays whole.
        assert_eq!(split(Some("rm  my rule  ")), (Some("rm"), Some("my rule")));
    }

    #[test]
    fn scopes_read_as_english() {
        assert_eq!(scope_label(&[]), "prose + tool calls");
        assert_eq!(scope_label(&["tool".into()]), "tool calls");
        assert_eq!(
            scope_label(&["tool".into(), "text".into()]),
            "tool calls + prose"
        );
    }
}
