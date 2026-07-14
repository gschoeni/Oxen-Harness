//! `/preview` — open the running dev server (started by the agent via
//! `start_dev_server`) in the user's browser, or explain why there's nothing
//! to open.

use crate::preview::{last_status, open_in_browser};
use crate::theme::Ui;

pub(crate) fn handle_repl(ui: &Ui) {
    let Some(status) = last_status() else {
        println!(
            "  {}",
            ui.dim("no dev server yet — ask the agent to run your app and it will start one")
        );
        return;
    };
    match (status.phase, status.url) {
        (harness_preview::PreviewPhase::Ready, Some(url)) => {
            println!(
                "  {} {}  {}",
                ui.green("▸"),
                ui.accent(&format!("{} server running at {url}", status.name)),
                ui.dim("— opening your browser…"),
            );
            open_in_browser(&url);
        }
        (phase, _) => {
            let detail = status.message.unwrap_or_default();
            println!(
                "  {} {}",
                ui.dim(&format!("dev server is {phase:?}{}", {
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(" — {detail}")
                    }
                })),
                ui.dim("(ask the agent to restart it, or check dev_server_logs)"),
            );
        }
    }
}
