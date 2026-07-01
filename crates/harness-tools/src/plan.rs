//! `update_plan` — lay out a task checklist and tick items off as work
//! progresses (like Claude Code's TodoWrite).
//!
//! The model calls this for any multi-step task: it sends the *full* plan every
//! time, keeping exactly one item `in_progress` and flipping items to
//! `completed` as soon as each is done. Each call replaces the previous list, so
//! the latest call is the current state — front ends render it as a single,
//! in-place-updating checklist rather than a stream of separate cards.
//!
//! Rendering is host-specific (a themed block in the CLI, a pinned panel in the
//! desktop app), driven off the tool-call arguments. This module defines only
//! the [`PlanItem`] data, parsing/validation, and the [`PlanTool`] that records
//! the plan into the transcript (its result is a text rendering so the model
//! sees the current state).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{Tool, ToolError};

/// The tool name the model calls (and front ends special-case for rendering).
pub const PLAN_TOOL: &str = "update_plan";

/// Lifecycle of a single plan item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// Not started yet.
    Pending,
    /// Currently being worked on. At most one item may be in this state.
    InProgress,
    /// Finished successfully.
    Completed,
}

/// One entry in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    /// Imperative description of the step, e.g. "Wire CLI rendering".
    pub content: String,
    /// Present-continuous form shown while active, e.g. "Wiring CLI rendering".
    pub active_form: String,
    /// Current lifecycle state.
    pub status: PlanStatus,
}

/// Parse + validate the tool arguments into the full plan.
///
/// Enforces the same invariants the model is told to maintain: a non-empty list,
/// non-empty text on every item, and at most one `in_progress` item.
fn parse_plan(args: &serde_json::Value) -> Result<Vec<PlanItem>, String> {
    let raw = args
        .get("plan")
        .and_then(|v| v.as_array())
        .ok_or("missing `plan` array")?;
    if raw.is_empty() {
        return Err("`plan` must contain at least one item".into());
    }

    let mut items = Vec::with_capacity(raw.len());
    for (i, entry) in raw.iter().enumerate() {
        let content = entry
            .get("content")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("plan[{i}] is missing a non-empty `content`"))?
            .to_string();
        let active_form = entry
            .get("active_form")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("plan[{i}] is missing a non-empty `active_form`"))?
            .to_string();
        let status = match entry.get("status").and_then(|v| v.as_str()) {
            Some("pending") => PlanStatus::Pending,
            Some("in_progress") => PlanStatus::InProgress,
            Some("completed") => PlanStatus::Completed,
            Some(other) => {
                return Err(format!(
                    "plan[{i}] has unknown `status` {other:?}; use one of: pending, in_progress, completed"
                ))
            }
            None => return Err(format!("plan[{i}] is missing `status`")),
        };
        items.push(PlanItem {
            content,
            active_form,
            status,
        });
    }

    let in_progress = items
        .iter()
        .filter(|it| it.status == PlanStatus::InProgress)
        .count();
    if in_progress > 1 {
        return Err(format!(
            "at most one item may be `in_progress` at a time (found {in_progress})"
        ));
    }

    Ok(items)
}

/// Number of completed items in a plan.
fn completed(items: &[PlanItem]) -> usize {
    items
        .iter()
        .filter(|it| it.status == PlanStatus::Completed)
        .count()
}

/// A plain-text rendering of the plan, recorded as the tool result so the model
/// sees the current state in the transcript.
fn render(items: &[PlanItem]) -> String {
    let mut out = format!("Plan ({}/{} done):\n", completed(items), items.len());
    for it in items {
        let (mark, text) = match it.status {
            PlanStatus::Completed => ('x', it.content.as_str()),
            PlanStatus::InProgress => ('>', it.active_form.as_str()),
            PlanStatus::Pending => (' ', it.content.as_str()),
        };
        out.push_str(&format!("[{mark}] {text}\n"));
    }
    out.truncate(out.trim_end().len());
    out
}

/// The model-facing tool that records/updates the task plan.
#[derive(Default)]
pub struct PlanTool;

impl PlanTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for PlanTool {
    fn name(&self) -> &str {
        PLAN_TOOL
    }

    fn description(&self) -> &str {
        "Track a LARGE, multi-phase piece of work as a shared checklist so the \
         user can follow progress on something that will take many tool calls \
         and span several distinct areas. Send the ENTIRE plan every call — it \
         replaces the previous one. Each item has `content` (imperative, e.g. \
         \"Run tests\"), `active_form` (present-continuous, e.g. \"Running \
         tests\"), and a `status` of pending, in_progress, or completed.\n\
         This tool is the EXCEPTION, not the default — most tasks should be done \
         directly without it. Only reach for it when ALL of these hold: the work \
         needs roughly 5+ substantial steps, those steps span clearly separate \
         pieces of work (not five edits to one file), and the user would \
         genuinely benefit from seeing a tracked checklist. Also use it when the \
         user explicitly asks for a plan/todo list or hands you a numbered list \
         of separate tasks.\n\
         Do NOT use it for: a single change or bug fix; a handful of edits even \
         across a few files; reading/searching/answering questions; anything you \
         can finish in a few tool calls; or breaking one logical task into busywork \
         steps (\"read the file\", \"make the edit\", \"run the build\") just to have \
         a list. When unsure, do NOT use it — just do the work. A checklist for \
         small work is noise that clutters the UI.\n\
         When you do use it: keep EXACTLY ONE item in_progress while you work \
         (mark the next item in_progress before starting it); mark an item \
         completed IMMEDIATELY when it's done — never batch completions; only \
         mark completed when fully done (if it failed or is partial, leave it \
         in_progress and add a follow-up item); drop items that are no longer \
         relevant."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "plan": {
                    "type": "array",
                    "description": "The full, updated checklist. Send every item every time; this replaces the previous plan.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "Imperative description of the step (e.g. 'Run tests')."
                            },
                            "active_form": {
                                "type": "string",
                                "description": "Present-continuous form shown while active (e.g. 'Running tests')."
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "pending = not started; in_progress = working on it now (at most one); completed = done."
                            }
                        },
                        "required": ["content", "active_form", "status"]
                    }
                }
            },
            "required": ["plan"]
        })
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let items = parse_plan(&args).map_err(ToolError::InvalidArguments)?;
        Ok(render(&items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, status: &str) -> serde_json::Value {
        serde_json::json!({
            "content": content,
            "active_form": format!("{content}ing"),
            "status": status,
        })
    }

    #[test]
    fn parses_a_valid_plan() {
        let items = parse_plan(&serde_json::json!({
            "plan": [
                item("Research", "completed"),
                item("Build", "in_progress"),
                item("Verify", "pending"),
            ]
        }))
        .unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].status, PlanStatus::Completed);
        assert_eq!(items[1].status, PlanStatus::InProgress);
        assert_eq!(completed(&items), 1);
    }

    #[test]
    fn rejects_empty_list() {
        assert!(parse_plan(&serde_json::json!({ "plan": [] })).is_err());
        assert!(parse_plan(&serde_json::json!({})).is_err());
    }

    #[test]
    fn rejects_blank_fields_and_bad_status() {
        assert!(parse_plan(&serde_json::json!({
            "plan": [{ "content": "  ", "active_form": "x", "status": "pending" }]
        }))
        .is_err());
        assert!(parse_plan(&serde_json::json!({
            "plan": [{ "content": "x", "active_form": "x", "status": "nope" }]
        }))
        .is_err());
    }

    #[test]
    fn rejects_two_in_progress() {
        let err = parse_plan(&serde_json::json!({
            "plan": [item("A", "in_progress"), item("B", "in_progress")]
        }))
        .unwrap_err();
        assert!(err.contains("in_progress"), "err: {err}");
    }

    #[tokio::test]
    async fn invoke_returns_rendered_checklist_with_count() {
        let tool = PlanTool::new();
        let out = tool
            .invoke(serde_json::json!({
                "plan": [item("Research", "completed"), item("Build", "in_progress")]
            }))
            .await
            .unwrap();
        assert!(out.contains("1/2 done"), "out: {out}");
        assert!(out.contains("[x] Research"), "out: {out}");
        assert!(out.contains("[>] Building"), "out: {out}");
    }

    #[test]
    fn name_and_schema_shape() {
        let tool = PlanTool::new();
        assert_eq!(tool.name(), PLAN_TOOL);
        let schema = tool.parameters_schema();
        assert_eq!(
            schema["properties"]["plan"]["items"]["properties"]["status"]["enum"][1],
            "in_progress"
        );
    }
}
