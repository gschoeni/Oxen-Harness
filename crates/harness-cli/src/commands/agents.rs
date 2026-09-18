//! `/agents` — the hub: every subagent lane of this chat, running and
//! finished, with stable IDs for direct controls and a guided chooser.

use std::sync::Arc;

use harness_agent::{LaneStatus, SubagentResult};
use harness_core::fmt::human_tokens;
use harness_store::HistoryStore;

use crate::theme::Ui;

/// One row of the hub, running or finished.
struct Row {
    id: String,
    label: String,
    status: String,
    detail: String,
    depth: usize,
}

fn rows(store: &HistoryStore, session: &str) -> Vec<Row> {
    let live = crate::endpoint::live_lanes();
    let running: std::collections::HashSet<&str> = live.iter().map(|l| l.id.as_str()).collect();
    let mut out = Vec::new();
    let mut depths = std::collections::HashMap::new();
    let mut pending = vec![(session.to_string(), 0)];
    let mut seen = std::collections::HashSet::new();
    while let Some((parent, depth)) = pending.pop() {
        if !seen.insert(parent.clone()) {
            continue;
        }
        let lanes = store.lanes_of(&parent).unwrap_or_default();
        pending.extend(lanes.iter().map(|l| (l.id.clone(), depth + 1)));
        depths.extend(lanes.iter().map(|l| (l.id.clone(), depth)));
        out.extend(
            lanes
                .into_iter()
                .filter(|lane| !running.contains(lane.id.as_str()))
                .map(|lane| {
                    let record: Option<SubagentResult> = lane
                        .record
                        .and_then(|value| serde_json::from_value(value).ok());
                    match record {
                        Some(record) => Row {
                            id: lane.id,
                            depth,
                            status: match record.status {
                                LaneStatus::Done => "done".into(),
                                LaneStatus::Partial
                                    if record
                                        .stop
                                        .as_deref()
                                        .is_some_and(|s| s.contains("cancelled")) =>
                                {
                                    "stopped".into()
                                }
                                LaneStatus::Partial => "partial".into(),
                                LaneStatus::Failed => "failed".into(),
                            },
                            detail: format!(
                                "{} tok · {} · {}",
                                human_tokens(record.tokens),
                                record.status_line(),
                                record.brief()
                            ),
                            label: record.label,
                        },
                        None => Row {
                            id: lane.id,
                            depth,
                            label: "agent".into(),
                            status: "unknown".into(),
                            detail: "(interrupted before its record was written)".into(),
                        },
                    }
                }),
        );
    }
    for lane in live {
        out.push(Row {
            depth: depths.get(&lane.id).copied().unwrap_or_default(),
            id: lane.id,
            label: lane.label,
            status: "running".into(),
            detail: format!("{}s · fleet {}", lane.elapsed_secs, lane.fleet),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub(crate) async fn handle_repl(
    rest: Option<String>,
    store: &Arc<HistoryStore>,
    session: &str,
    ui: &Ui,
) {
    let rows = rows(store, session);
    let hub = crate::fleet_ui::FleetHub::global();
    if rest.as_deref().is_some_and(|s| s.trim() == "drafts") {
        let mut found = false;
        for row in &rows {
            let draft = hub.draft(&row.id);
            if !draft.is_empty() {
                found = true;
                println!(
                    "  {}\n    {}",
                    ui.title(&format!("{} · {}", row.label, short_id(&rows, &row.id))),
                    ui.cream(&draft)
                );
                println!(
                    "  {}",
                    ui.dim(&format!(
                        "/agents follow-up {} sends this saved draft",
                        short_id(&rows, &row.id)
                    ))
                );
            }
        }
        if !found {
            println!("  {}", ui.dim("No unsent agent directions in this chat"));
        }
        return;
    }
    if rest.as_deref().is_some_and(|s| s.trim() == "help") {
        help(ui, None);
        return;
    }
    let mut input = rest.unwrap_or_default().trim().to_string();
    if input.is_empty() && !rows.is_empty() {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            let options: Vec<_> = std::iter::once(crate::picker::Choice::new(
                "Main chat",
                "Return to the main conversation",
            ))
            .chain(rows.iter().map(|row| {
                crate::picker::Choice::new(
                    format!("{} · {}", row.label, short_id(&rows, &row.id)),
                    format!(
                        "{} · {}",
                        row.status,
                        if row.status == "running" {
                            "watch live output and send directions"
                        } else {
                            "read the saved answer"
                        }
                    ),
                )
            }))
            .collect();
            let ui = ui.clone();
            let picked = tokio::task::spawn_blocking(move || {
                crate::picker::select(
                    &ui,
                    "Agents",
                    "Choose an agent · ↑/↓ move · Enter opens · Esc returns",
                    &options,
                    false,
                )
            })
            .await;
            let Some(choice) = picked
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|v| v.into_iter().next())
            else {
                return;
            };
            if choice == "Main chat" {
                if let Some(state) = crate::fleet_ui::FleetHub::global().lock().primary_mut() {
                    state.focus(None);
                }
                return;
            }
            if let Some(row) = rows
                .iter()
                .find(|r| choice == format!("{} · {}", r.label, short_id(&rows, &r.id)))
            {
                input = format!(
                    "{} {}",
                    if row.status == "running" {
                        "watch"
                    } else {
                        "read"
                    },
                    row.id
                );
            } else {
                input = format!("watch {choice}");
            }
        }
    }
    let input = input.as_str();
    let (command, rest) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
    let rest = rest.trim_start();
    let (which, message) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if matches!(command, "watch" | "stop" | "send" | "follow-up" | "patch") {
        let row = match resolve(&rows, which) {
            Ok(row) => row,
            Err(error) => {
                println!("  {}", ui.red(&error));
                return;
            }
        };
        let Some(spawner) = crate::endpoint::fleet_spawner() else {
            return;
        };
        let saved = hub.draft(&row.id);
        let message = if command == "follow-up" && message.trim().is_empty() {
            saved.as_str()
        } else {
            message
        };
        let result: Result<String, String> = match command {
            "watch" => {
                if crate::fleet_ui::FleetHub::global()
                    .lock()
                    .watch_lane(&row.id)
                {
                    Ok(format!(
                        "Watching {} · Esc returns to main chat · Alt+X stops this agent",
                        row.label
                    ))
                } else {
                    Err("Agent has finished; use /agents read or show".into())
                }
            }
            "stop" => {
                if spawner.tree().cancel(&row.id) {
                    Ok("Stop requested".into())
                } else {
                    Err("Agent has already finished".into())
                }
            }
            "send" if message.trim().is_empty() => Err("Usage: /agents send <id> <message>".into()),
            "send" => {
                if spawner.tree().interject(&row.id, message) {
                    Ok("Message queued".into())
                } else {
                    Err("Agent has finished; use /agents follow-up <id> <message>".into())
                }
            }
            "follow-up" => spawner
                .follow_up(&row.id, message)
                .await
                .map_err(|e| e.to_string()),
            "patch" => spawner.patch(&row.id).map_err(|e| e.to_string()),
            _ => Err(format!("Unknown agents command: {command}")),
        };
        match result {
            Ok(text) => {
                if matches!(command, "follow-up" | "send") {
                    hub.keep_draft(&row.id, "");
                }
                println!("  {}", ui.cream(&text));
            }
            Err(error) => println!("  {}", ui.red(&error)),
        }
        return;
    }
    let mut words = input.split_whitespace();
    match (words.next(), words.next()) {
        (Some("show"), Some(which)) => {
            let row = match resolve(&rows, which) {
                Ok(row) => row,
                Err(error) => {
                    println!("  {}", ui.red(&error));
                    return;
                }
            };
            println!(
                "  {}",
                ui.title(&format!(
                    "─── {} ({}) — transcript ───",
                    row.label, row.status
                ))
            );
            for message in store.messages(&row.id).unwrap_or_default() {
                let role = message["role"].as_str().unwrap_or("?");
                let text = message["content"].as_str().unwrap_or("").trim().to_string();
                let calls: Vec<String> = message["tool_calls"]
                    .as_array()
                    .map(|calls| {
                        calls
                            .iter()
                            .filter_map(|c| c["function"]["name"].as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let head = match role {
                    "user" => ui.green("▸ user"),
                    "assistant" => ui.title("◂ agent"),
                    "tool" => ui.dim("  ⚙ result"),
                    other => ui.dim(other),
                };
                let calls = if calls.is_empty() {
                    String::new()
                } else {
                    format!(" → {}", calls.join(", "))
                };
                println!("  {head}{}", ui.dim(&calls));
                for line in crate::render::truncate(&text, 1_200).lines() {
                    println!("    {}", ui.cream(line));
                }
            }
        }
        (Some("read"), Some(which)) => {
            // Direct commands use the same stable selector as the chooser.
            let row = match resolve(&rows, which) {
                Ok(row) => row,
                Err(error) => {
                    println!("  {}", ui.red(&error));
                    return;
                }
            };
            match store.last_assistant_text(&row.id) {
                Ok(Some(text)) => {
                    println!(
                        "  {}",
                        ui.title(&format!("─── {} ({}) ───", row.label, row.status))
                    );
                    for line in text.lines() {
                        println!("  {}", ui.cream(line));
                    }
                }
                _ => println!("  {}", ui.dim("(this agent has not replied yet)")),
            }
        }
        _ => {
            if !input.is_empty() && input != "list" {
                println!(
                    "  {}",
                    ui.red(&format!("Unknown or incomplete agent command: {input}"))
                );
                help(ui, None);
                return;
            }
            if rows.is_empty() {
                println!(
                    "  {}",
                    ui.dim("No agents in this chat yet — the model spawns them with spawn_agents or map_agents.")
                );
                return;
            }
            println!("  {}", ui.brown("AGENTS"));
            for row in &rows {
                let status = match row.status.as_str() {
                    "done" => ui.green(&row.status),
                    "failed" => ui.red(&row.status),
                    "running" => ui.title(&row.status),
                    _ => ui.dim(&row.status),
                };
                println!(
                    "  {} {}{} {} {}",
                    ui.dim(&short_id(&rows, &row.id)),
                    "  ".repeat(row.depth.min(4)),
                    ui.cream(&row.label),
                    status,
                    ui.dim(&crate::render::truncate(&row.detail, 100)),
                );
            }
            help(ui, rows.first().map(|r| short_id(&rows, &r.id)).as_deref());
        }
    }
}

fn resolve<'a>(rows: &'a [Row], which: &str) -> Result<&'a Row, String> {
    if which.is_empty() {
        return Err("Specify an agent ID from /agents".into());
    }
    if let Some(row) = rows.iter().find(|r| r.id == which) {
        return Ok(row);
    }
    let mut matches = rows
        .iter()
        .filter(|r| r.id.starts_with(which) || r.label == which);
    match (matches.next(), matches.next()) {
        (Some(row), None) => Ok(row),
        (Some(_), Some(_)) => Err(format!(
            "Agent prefix {which} is ambiguous; use more of its ID"
        )),
        _ => Err(format!(
            "No agent {which} in this chat; use an ID or unique name from /agents list"
        )),
    }
}

fn short_id(rows: &[Row], id: &str) -> String {
    let mut end = id.len().min(8);
    while end < id.len()
        && (!id.is_char_boundary(end)
            || rows
                .iter()
                .any(|r| r.id != id && r.id.starts_with(&id[..end])))
    {
        end += 1;
    }
    id[..end].to_string()
}

fn help(ui: &Ui, example: Option<&str>) {
    let id = example.unwrap_or("<id>");
    for line in [
        format!("Watch an agent    /agents watch {id}"),
        "Switch live      ↑ / ↓ with an empty box · Alt+← / → any time · Alt+1–9 jump to a row".into(),
        "Main chat        Esc while watching (or Alt+0) · the header always shows where Enter sends".into(),
        "Switch fleets    Alt+[ / ] when multiple fleets are running".into(),
        format!("Send a direction /agents send {id} check the tests too"),
        format!("Stop one         /agents stop {id} · or Alt+X while watching"),
        format!("Read results     /agents read {id} · show {id} for the transcript"),
        format!("Continue         /agents follow-up {id} fix the remaining issue"),
        format!("Review edits     /agents patch {id}"),
        "Recover drafts   /agents drafts · follow-up <id> sends its saved direction".into(),
        "/agents opens the chooser · /agents list prints IDs · /agents help shows this guide."
            .into(),
    ] {
        println!("  {}", ui.dim(&line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(id: &str, label: &str) -> Row {
        Row {
            id: id.into(),
            label: label.into(),
            status: "running".into(),
            detail: String::new(),
            depth: 0,
        }
    }
    #[test]
    fn targets_require_unique_ids_or_names() {
        let rows = [
            row("abcdefgh1", "scan"),
            row("abcdefgh2", "scan"),
            row("other1234", "tests"),
        ];
        assert!(resolve(&rows, "abcdefgh").is_err());
        assert!(resolve(&rows, "scan").is_err());
        assert!(
            resolve(&rows, "1").is_err(),
            "list positions must not silently retarget commands"
        );
        assert_eq!(resolve(&rows, "tests").unwrap().id, "other1234");
        assert_eq!(resolve(&rows, "abcdefgh1").unwrap().id, "abcdefgh1");
        for row in &rows {
            assert_eq!(
                resolve(&rows, &short_id(&rows, &row.id)).unwrap().id,
                row.id
            );
        }
    }
}
