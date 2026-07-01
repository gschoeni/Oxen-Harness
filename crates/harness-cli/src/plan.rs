//! Inline rendering of an `update_plan` tool call: a themed checklist block the
//! live renderer drops in place of the generic tool line, mirroring
//! `canvas::render_canvas_block` and `diff::render_file_change`.
//!
//! Each call carries the full plan, so this reprints the whole checklist — the
//! latest block is the current state.

use crate::theme::Ui;

/// Render a checklist block from an `update_plan` call's raw JSON arguments, or
/// `None` if the arguments don't parse into a non-empty plan.
pub fn render_plan_block(ui: &Ui, arguments: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let items = v.get("plan").and_then(|x| x.as_array())?;
    if items.is_empty() {
        return None;
    }

    let total = items.len();
    let done = items
        .iter()
        .filter(|it| it.get("status").and_then(|s| s.as_str()) == Some("completed"))
        .count();

    let mut out = vec![format!(
        "  {} {}  {}",
        ui.green("◆"),
        ui.accent("Plan"),
        ui.dim(&format!("· {done}/{total}")),
    )];

    for it in items {
        let status = it.get("status").and_then(|s| s.as_str()).unwrap_or("pending");
        let content = it.get("content").and_then(|s| s.as_str()).unwrap_or("");
        let active = it
            .get("active_form")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(content);
        let line = match status {
            "completed" => format!("    {} {}", ui.dim("☒"), strike(ui, content)),
            "in_progress" => format!("    {} {}", ui.green("▸"), ui.accent(active)),
            _ => format!("    {} {}", ui.dim("☐"), ui.cream(content)),
        };
        out.push(line);
    }
    Some(out)
}

/// Dim + strikethrough text, degrading to plain when color is disabled (detected
/// by `dim` returning the input unchanged, i.e. no ANSI was emitted).
fn strike(ui: &Ui, text: &str) -> String {
    let dimmed = ui.dim(text);
    if dimmed == text {
        // No-color mode: avoid leaking an unterminated escape.
        text.to_string()
    } else {
        // `dimmed` ends in a full reset (\x1b[0m), which also closes strikethrough.
        format!("\x1b[9m{dimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Ui;

    fn args() -> String {
        serde_json::json!({
            "plan": [
                { "content": "Research", "active_form": "Researching", "status": "completed" },
                { "content": "Build", "active_form": "Building", "status": "in_progress" },
                { "content": "Verify", "active_form": "Verifying", "status": "pending" },
            ]
        })
        .to_string()
    }

    #[test]
    fn renders_header_and_one_line_per_item() {
        let ui = Ui::plain();
        let block = render_plan_block(&ui, &args()).unwrap();
        // Header + 3 items.
        assert_eq!(block.len(), 4);
        assert!(block[0].contains("Plan"));
        assert!(block[0].contains("1/3"));
        assert!(block[1].contains("Research"));
        // The in-progress row shows the active form.
        assert!(block[2].contains("Building"));
        assert!(block[3].contains("Verify"));
    }

    #[test]
    fn rejects_unparseable_or_empty() {
        let ui = Ui::plain();
        assert!(render_plan_block(&ui, "not json").is_none());
        assert!(render_plan_block(&ui, r#"{"plan": []}"#).is_none());
    }
}
