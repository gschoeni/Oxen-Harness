//! `/agents` — the hub: every subagent lane of this chat, running and
//! finished, and `/agents read <n|id>` to print one's full reply.

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
}

fn rows(store: &HistoryStore, session: &str) -> Vec<Row> {
    let live = crate::endpoint::live_lanes();
    let running: std::collections::HashSet<&str> = live.iter().map(|l| l.id.as_str()).collect();
    let mut out: Vec<Row> = store
        .lanes_of(session)
        .unwrap_or_default()
        .into_iter()
        .filter(|lane| !running.contains(lane.id.as_str()))
        .map(|lane| {
            let record: Option<SubagentResult> = lane
                .record
                .and_then(|value| serde_json::from_value(value).ok());
            match record {
                Some(record) => Row {
                    id: lane.id,
                    status: match record.status {
                        LaneStatus::Done => "done".into(),
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
                    label: "agent".into(),
                    status: "unknown".into(),
                    detail: "(interrupted before its record was written)".into(),
                },
            }
        })
        .collect();
    for lane in live {
        out.push(Row {
            id: lane.id,
            label: lane.label,
            status: "running".into(),
            detail: format!("{}s · fleet {}", lane.elapsed_secs, lane.fleet),
        });
    }
    out
}

pub(crate) async fn handle_repl(
    rest: Option<String>,
    store: &Arc<HistoryStore>,
    session: &str,
    ui: &Ui,
) {
    let rows = rows(store, session);
    let mut words = rest.as_deref().unwrap_or("").split_whitespace();
    match (words.next(), words.next()) {
        (Some("read"), Some(which)) => {
            // `read 3` by position, or `read <id-prefix>`.
            let row = which
                .parse::<usize>()
                .ok()
                .and_then(|n| rows.get(n.checked_sub(1)?))
                .or_else(|| rows.iter().find(|r| r.id.starts_with(which)));
            let Some(row) = row else {
                println!("  {}", ui.red(&format!("no agent {which} in this chat")));
                return;
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
            if rows.is_empty() {
                println!(
                    "  {}",
                    ui.dim("No agents in this chat yet — the model spawns them with spawn_agents or map_agents.")
                );
                return;
            }
            println!("  {}", ui.brown("AGENTS"));
            for (index, row) in rows.iter().enumerate() {
                let status = match row.status.as_str() {
                    "done" => ui.green(&row.status),
                    "failed" => ui.red(&row.status),
                    "running" => ui.title(&row.status),
                    _ => ui.dim(&row.status),
                };
                println!(
                    "  {:>2}. {} {} {}",
                    index + 1,
                    ui.cream(&row.label),
                    status,
                    ui.dim(&crate::render::truncate(&row.detail, 100)),
                );
            }
            println!(
                "  {}",
                ui.dim("/agents read <n> prints one's full reply · x stops the watched lane while a fleet runs")
            );
        }
    }
}
