//! `/tasks` — the session's background shell commands: what is running, for
//! how long, what it last printed; `/tasks kill <n|id>` stops one.

use harness_tools::tasks::TaskSummary;

use crate::theme::Ui;

fn status_word(task: &TaskSummary) -> String {
    if task.running {
        "running".into()
    } else if task.killed {
        "killed".into()
    } else {
        match task.exit_code {
            Some(0) => "done".into(),
            Some(code) => format!("exit {code}"),
            None => "ended".into(),
        }
    }
}

pub(crate) async fn handle_repl(rest: Option<String>, ui: &Ui) {
    let Some(registry) = crate::endpoint::background_tasks() else {
        println!(
            "  {}",
            ui.dim("Background tasks are not available in this session.")
        );
        return;
    };
    let tasks = registry.snapshot().await;
    let mut words = rest.as_deref().unwrap_or("").split_whitespace();
    match (words.next(), words.next()) {
        (Some("kill"), Some(which)) => {
            // `kill 2` by position in the list, or by task id.
            let task = which
                .parse::<usize>()
                .ok()
                .and_then(|n| tasks.get(n.checked_sub(1)?))
                .or_else(|| {
                    which
                        .parse::<u64>()
                        .ok()
                        .and_then(|id| tasks.iter().find(|t| t.id == id))
                });
            let Some(task) = task else {
                println!("  {}", ui.red(&format!("no background task {which}")));
                return;
            };
            match registry.kill(task.id).await {
                Ok(note) => println!("  {}", ui.dim(&note)),
                Err(e) => println!("  {}", ui.red(&e.to_string())),
            }
        }
        _ => {
            if tasks.is_empty() {
                println!(
                    "  {}",
                    ui.dim("No background tasks — run_shell with is_background, or a long command, starts one.")
                );
                return;
            }
            println!("  {}", ui.brown("BACKGROUND TASKS"));
            for (index, task) in tasks.iter().enumerate() {
                let status = status_word(task);
                let status = if task.running {
                    ui.title(&status)
                } else if status == "done" {
                    ui.green(&status)
                } else {
                    ui.red(&status)
                };
                println!(
                    "  {:>2}. {} {} {}",
                    index + 1,
                    ui.cream(&crate::render::truncate(&task.command, 48)),
                    status,
                    ui.dim(&format!(
                        "{}s{}",
                        task.elapsed_secs,
                        if task.last_line.is_empty() {
                            String::new()
                        } else {
                            format!(" · {}", crate::render::truncate(&task.last_line, 60))
                        }
                    )),
                );
            }
            println!(
                "  {}",
                ui.dim("/tasks kill <n> stops one · finished output is delivered to the model on its own")
            );
        }
    }
}
