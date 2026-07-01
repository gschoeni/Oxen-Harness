//! The `/queue` REPL command: stack, list, edit, reorder, remove, clear, or run
//! the queued prompts.
//!
//! The [`MessageQueue`] data structure lives in [`crate::queue`]; this module is
//! its command-line UX. Running the queue drains it through the shared
//! [`crate::run_turn_and_drain`], so a `/queue run` behaves exactly like sending
//! each prompt in turn.

use anyhow::Result;
use harness_agent::Agent;

use crate::queue::MessageQueue;
use crate::render::truncate;
use crate::theme::Ui;

/// Handle a `/queue ...` command. Returns `Ok(true)` if the user interrupted a
/// run (Ctrl-C), which ends the session.
pub async fn handle_repl(
    rest: Option<String>,
    queue: &mut MessageQueue,
    agent: &mut Agent,
    ui: &Ui,
    carryover: &mut String,
) -> Result<bool> {
    let rest = rest.unwrap_or_default();
    let mut parts = rest.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("");
    let payload = parts.next().map(str::trim).unwrap_or("");

    match sub {
        "" | "list" | "ls" => print_queue(queue, ui),
        "add" | "push" => {
            if payload.is_empty() {
                println!("  {}", ui.dim("usage: /queue add <message>"));
            } else {
                let n = queue.add(payload);
                println!(
                    "  {} {}",
                    ui.green(&format!("＋ Loaded wagon #{n}:")),
                    ui.dim(&truncate(payload, 80)),
                );
            }
        }
        "edit" => {
            let (pos, text) = split_pos(payload);
            match (pos, text) {
                (Some(pos), Some(text)) => match queue.edit(pos, text) {
                    Ok(()) => println!("  {}", ui.green(&format!("✎ Repacked wagon #{pos}"))),
                    Err(e) => println!("  {}", ui.red(&e)),
                },
                _ => println!("  {}", ui.dim("usage: /queue edit <n> <new message>")),
            }
        }
        "rm" | "remove" | "del" => match parse_pos(payload) {
            Some(pos) => match queue.remove(pos) {
                Ok(msg) => println!(
                    "  {} {}",
                    ui.brown(&format!("✗ Dropped wagon #{pos}:")),
                    ui.dim(&truncate(&msg, 80)),
                ),
                Err(e) => println!("  {}", ui.red(&e)),
            },
            None => println!("  {}", ui.dim("usage: /queue rm <n>")),
        },
        "up" | "down" => match parse_pos(payload) {
            Some(pos) => {
                let dir = if sub == "up" { -1 } else { 1 };
                match queue.move_by(pos, dir) {
                    Ok(()) => print_queue(queue, ui),
                    Err(e) => println!("  {}", ui.red(&e)),
                }
            }
            None => println!("  {}", ui.dim(&format!("usage: /queue {sub} <n>"))),
        },
        "clear" => {
            queue.clear();
            println!("  {}", ui.brown("🧹 Emptied the wagon"));
        }
        "run" | "go" => return run_queue(queue, agent, ui, carryover).await,
        _ => println!(
            "  {}",
            ui.dim("queue: list | add <msg> | edit <n> <msg> | up <n> | down <n> | rm <n> | clear | run"),
        ),
    }
    Ok(false)
}

/// Run every queued prompt in order, draining the queue. Returns `Ok(true)` if
/// the user interrupted (Ctrl-C) mid-run.
async fn run_queue(
    queue: &mut MessageQueue,
    agent: &mut Agent,
    ui: &Ui,
    carryover: &mut String,
) -> Result<bool> {
    if queue.is_empty() {
        println!("  {}", ui.dim("the wagon is empty — nothing to send"));
        return Ok(false);
    }
    let first = queue.pop_front().expect("queue is non-empty");
    println!(
        "  {} {}",
        ui.brown("▶ rolling the wagon:"),
        ui.cream(&truncate(&first, 80)),
    );
    crate::run_turn_and_drain(agent, &first, ui, queue, carryover).await
}

fn print_queue(queue: &MessageQueue, ui: &Ui) {
    if queue.is_empty() {
        println!(
            "  {}",
            ui.dim("the wagon is empty — /queue add <message> to stack one up"),
        );
        return;
    }
    println!(
        "  {}",
        ui.brown(&format!("⛺ {} stacked in the wagon:", queue.len())),
    );
    for (i, msg) in queue.items().iter().enumerate() {
        println!(
            "    {} {}",
            ui.accent(&format!("{}.", i + 1)),
            ui.cream(&truncate(msg, 90)),
        );
    }
    println!(
        "  {}",
        ui.dim("/queue run to send · /queue edit <n> <msg> · /queue rm <n>")
    );
}

/// Parse a leading 1-based position from `s` (the whole string is the number).
fn parse_pos(s: &str) -> Option<usize> {
    s.trim().parse::<usize>().ok()
}

/// Split `"<n> <rest>"` into the position and the remaining text.
fn split_pos(s: &str) -> (Option<usize>, Option<String>) {
    let mut parts = s.trim().splitn(2, char::is_whitespace);
    let pos = parts.next().and_then(|p| p.parse::<usize>().ok());
    let text = parts
        .next()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    (pos, text)
}

#[cfg(test)]
mod tests {
    use super::{parse_pos, split_pos};

    #[test]
    fn parse_pos_reads_a_bare_number() {
        assert_eq!(parse_pos("3"), Some(3));
        assert_eq!(parse_pos("  2 "), Some(2));
        assert_eq!(parse_pos("x"), None);
        assert_eq!(parse_pos(""), None);
    }

    #[test]
    fn split_pos_separates_index_from_message_text() {
        assert_eq!(
            split_pos("2 fix the bug"),
            (Some(2), Some("fix the bug".to_string()))
        );
        assert_eq!(split_pos("5"), (Some(5), None));
        assert_eq!(split_pos("nope"), (None, None));
    }
}
