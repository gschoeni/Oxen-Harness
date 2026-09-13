//! `/fork` and `/rewind` — branch the trail without losing any of it.
//!
//! Both are forks (see `harness_agent::Agent::fork_through`): the original
//! session keeps every entry, and the REPL moves onto a copy. `/fork` copies
//! everything (try a different approach from here, keep the first attempt
//! on record); `/rewind` copies up to *before* a chosen earlier message, so
//! a wrong turn can be retried from the point it went wrong.

use anyhow::Result;
use harness_agent::Agent;

use crate::picker::{self, Choice};
use crate::render::truncate;
use crate::repl_loop::ReplContext;
use crate::theme::Ui;

/// How much of a message the rewind picker shows per row.
const PREVIEW_CHARS: usize = 72;

/// `/fork`: continue on a copy of this session.
pub(crate) fn fork_repl(agent: &mut Agent, ui: &Ui, ctx: &ReplContext<'_>) -> Result<()> {
    let original = agent.session_id().to_string();
    let forked = agent.fork_through(None)?;
    *agent = ctx.adopt(forked);
    println!(
        "  {} {}",
        ui.green("⑂ forked the trail:"),
        ui.cream(&format!(
            "now on {} · the original ({}) keeps every entry",
            short(agent.session_id()),
            short(&original)
        )),
    );
    Ok(())
}

/// `/rewind [n]`: go back to just before the n-th message you sent (a picker
/// when `n` is omitted), on a fork.
pub(crate) fn rewind_repl(
    rest: Option<String>,
    agent: &mut Agent,
    ui: &Ui,
    ctx: &ReplContext<'_>,
) -> Result<()> {
    let turns = agent.user_turns()?;
    if turns.is_empty() {
        println!(
            "  {}",
            ui.dim("nothing to rewind yet — no messages on this trail")
        );
        return Ok(());
    }
    let index = match rest.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(n) => match n.parse::<usize>() {
            Ok(n) if (1..=turns.len()).contains(&n) => n - 1,
            _ => {
                println!(
                    "  {}",
                    ui.red(&format!(
                        "`/rewind <n>` takes a message number from 1 to {}",
                        turns.len()
                    ))
                );
                return Ok(());
            }
        },
        None => match choose(ui, &turns) {
            Some(i) => i,
            None => {
                println!("  {}", ui.brown("Messages on this trail"));
                for (i, (_, text)) in turns.iter().enumerate() {
                    println!(
                        "    {} {}",
                        ui.accent(&format!("{}", i + 1)),
                        ui.dim(&preview(text)),
                    );
                }
                println!(
                    "  {}",
                    ui.dim("`/rewind <n>` goes back to just before message n")
                );
                return Ok(());
            }
        },
    };
    let (seq, text) = &turns[index];
    let original = agent.session_id().to_string();
    let forked = agent.rewind_before(*seq)?;
    *agent = ctx.adopt(forked);
    println!(
        "  {} {}",
        ui.green("↶ rewound:"),
        ui.cream(&format!(
            "back to just before message {} on a fork ({}); the full trail stays as {}",
            index + 1,
            short(agent.session_id()),
            short(&original)
        )),
    );
    println!(
        "  {} {}",
        ui.dim("✎ that message was:"),
        ui.dim(&preview(text)),
    );
    Ok(())
}

fn choose(ui: &Ui, turns: &[(i64, String)]) -> Option<usize> {
    let options: Vec<Choice> = turns
        .iter()
        .enumerate()
        .map(|(i, (_, text))| Choice::new(format!("{}", i + 1), preview(text)))
        .collect();
    let chosen = picker::select(
        ui,
        "Rewind",
        "Go back to just before which message?",
        &options,
        false,
    )
    .ok()??;
    let label = chosen.into_iter().next()?;
    label.parse::<usize>().ok().map(|n| n - 1)
}

fn preview(text: &str) -> String {
    truncate(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        PREVIEW_CHARS,
    )
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}
