//! `update_trail` — chart the session's journey on the Ledger: name the
//! thread and lay out its waypoints (the few macro stages the work passes
//! through), then advance them as each is reached.
//!
//! This is deliberately coarser than `update_plan`. A plan is the working
//! checklist — many small items, churning constantly. The trail is the story
//! an overview surface tells about the whole session: typically define →
//! plan → implement → review, though the model may chart a different route
//! when the work has a different shape (research, debugging, writing). Each
//! call replaces the previous trail, so the latest call is the current state.
//!
//! Rendering is host-specific: the desktop's Ledger draws the waypoints as
//! named stations on the thread's trail line and uses `title` as the wagon's
//! name. This module defines only the data, parsing/validation, and the
//! [`TrailTool`] that records the trail into the transcript.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{ToolError, TypedTool};

/// The tool name the model calls (and front ends special-case for rendering).
pub const TRAIL_TOOL: &str = "update_trail";

/// The most waypoints a trail may carry. These are macro stages, not a todo
/// list — past a handful they stop being a story and start being a plan.
pub const MAX_WAYPOINTS: usize = 7;

/// Lifecycle of a single waypoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaypointStatus {
    /// Not reached yet.
    Ahead,
    /// The stage the session is in right now. At most one at a time.
    Current,
    /// Passed.
    Done,
}

/// One named stage on the session's trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Waypoint {
    /// Stage name, one or two lowercase words, e.g. "implement".
    pub name: String,
    /// Where the session stands relative to this stage.
    pub status: WaypointStatus,
}

/// Arguments to `update_trail`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct TrailArgs {
    /// Short, specific thread name (3–7 words), e.g. "fix flaky sse retry
    /// test" — shown as the thread's title on the user's board. Include it
    /// every call; omitting it keeps the previously charted title.
    #[serde(default)]
    pub title: Option<String>,
    /// The full route, in order (2–7 stages); replaces the previous trail.
    pub waypoints: Vec<Waypoint>,
}

/// The validated, persisted reading of a trail — what overview surfaces
/// render without loading the transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrailSnapshot {
    /// The model-chosen thread title.
    pub title: String,
    /// The route, in order.
    pub waypoints: Vec<Waypoint>,
}

/// Validate an already-parsed trail: 2..=MAX_WAYPOINTS stages with non-empty
/// names, at most one `current`, and no `current` *before* a `done` (the
/// wagon cannot be behind a station it passed). A missing title is accepted —
/// models under momentum reliably omit it (the shape mirrors `update_plan`,
/// which has no title), so the snapshot carries an empty title and the
/// persistence layer keeps whatever name was charted before.
fn validate_trail(
    title: Option<String>,
    mut waypoints: Vec<Waypoint>,
) -> Result<TrailSnapshot, String> {
    let title = title
        .unwrap_or_default()
        .trim()
        .trim_end_matches('.')
        .to_string();
    if waypoints.len() < 2 {
        return Err("a trail needs at least 2 waypoints".into());
    }
    if waypoints.len() > MAX_WAYPOINTS {
        return Err(format!(
            "at most {MAX_WAYPOINTS} waypoints — these are macro stages; use update_plan for a finer checklist"
        ));
    }
    for (i, wp) in waypoints.iter_mut().enumerate() {
        wp.name = wp.name.trim().to_string();
        if wp.name.is_empty() {
            return Err(format!("waypoints[{i}] is missing a non-empty `name`"));
        }
    }
    let current = waypoints
        .iter()
        .filter(|w| w.status == WaypointStatus::Current)
        .count();
    if current > 1 {
        return Err(format!(
            "at most one waypoint may be `current` at a time (found {current})"
        ));
    }
    let mut seen_open = false;
    for wp in &waypoints {
        match wp.status {
            WaypointStatus::Done if seen_open => {
                return Err("waypoints must be in order: a `done` stage cannot follow an unfinished one".into());
            }
            WaypointStatus::Done => {}
            _ => seen_open = true,
        }
    }
    Ok(TrailSnapshot { title, waypoints })
}

/// Parse an `update_trail` call's raw JSON arguments into a validated
/// snapshot — the same parse/validate path a real invocation takes. `None`
/// when the arguments would have been rejected (the call errored, so the
/// trail didn't actually change).
pub fn parse_trail_arguments(arguments: &str) -> Option<TrailSnapshot> {
    let args: TrailArgs = serde_json::from_str(arguments).ok()?;
    validate_trail(args.title, args.waypoints).ok()
}

/// A plain-text rendering recorded as the tool result, so the model sees the
/// current state in the transcript. A missing title gets a gentle nudge here —
/// the result is the one place the model reliably reads, so teach there
/// instead of failing the call.
fn render(trail: &TrailSnapshot) -> String {
    let mut out = if trail.title.is_empty() {
        "Trail:\n".to_string()
    } else {
        format!("Trail “{}”:\n", trail.title)
    };
    for wp in &trail.waypoints {
        let mark = match wp.status {
            WaypointStatus::Done => 'x',
            WaypointStatus::Current => '>',
            WaypointStatus::Ahead => ' ',
        };
        out.push_str(&format!("[{mark}] {}\n", wp.name));
    }
    if trail.title.is_empty() {
        out.push_str("(tip: include `title` — a short 3–7 word name shown on the user's board)\n");
    }
    out.truncate(out.trim_end().len());
    out
}

/// Carry the previously charted title forward when an update omits it — the
/// model updating waypoints mid-ride shouldn't accidentally un-name the
/// thread. The waypoints are always the new call's; only the name persists.
pub fn merge_trail(previous: Option<TrailSnapshot>, mut next: TrailSnapshot) -> TrailSnapshot {
    if next.title.is_empty() {
        if let Some(prev) = previous {
            next.title = prev.title;
        }
    }
    next
}

/// The model-facing tool that charts/updates the session's trail.
#[derive(Default)]
pub struct TrailTool;

impl TrailTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TypedTool for TrailTool {
    const NAME: &'static str = TRAIL_TOOL;
    type Args = TrailArgs;

    fn description(&self) -> &str {
        "Chart this session's journey for the user's overview board: a short \
         thread title plus its macro waypoints, advanced as each stage is \
         reached. The standard route is define, plan, implement, review — \
         chart a different one when the work's shape differs (e.g. reproduce, \
         isolate, fix, verify). Call it early in any session doing real work, \
         then again whenever a stage completes; send the ENTIRE trail every \
         call. Keep exactly one waypoint `current` and mark passed stages \
         `done`. This is the whole session's story at a glance, NOT a todo \
         list — use update_plan for fine-grained steps. Skip it for trivial \
         exchanges."
    }

    async fn run(&self, args: TrailArgs) -> Result<String, ToolError> {
        let trail = validate_trail(args.title, args.waypoints).map_err(ToolError::InvalidArguments)?;
        Ok(render(&trail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wp(name: &str, status: &str) -> serde_json::Value {
        serde_json::json!({ "name": name, "status": status })
    }

    fn parse(args: serde_json::Value) -> Option<TrailSnapshot> {
        parse_trail_arguments(&args.to_string())
    }

    #[test]
    fn parses_a_valid_trail_and_trims_the_title() {
        let trail = parse(serde_json::json!({
            "title": " fix flaky sse retry test. ",
            "waypoints": [wp("define", "done"), wp("implement", "current"), wp("review", "ahead")],
        }))
        .unwrap();
        assert_eq!(trail.title, "fix flaky sse retry test");
        assert_eq!(trail.waypoints.len(), 3);
        assert_eq!(trail.waypoints[1].status, WaypointStatus::Current);
    }

    #[test]
    fn a_missing_or_blank_title_is_accepted_as_unnamed() {
        // Models mirror `update_plan` (no title) and omit it — that must not
        // fail the call. The snapshot is simply unnamed; persistence keeps
        // the previous name and the board falls back to the first message.
        let trail = parse(serde_json::json!({
            "waypoints": [wp("define", "current"), wp("review", "ahead")],
        }))
        .unwrap();
        assert_eq!(trail.title, "");

        let blank = parse(serde_json::json!({
            "title": "  ",
            "waypoints": [wp("a", "ahead"), wp("b", "ahead")],
        }))
        .unwrap();
        assert_eq!(blank.title, "");
    }

    #[test]
    fn merge_keeps_the_charted_name_when_an_update_omits_it() {
        let named = parse(serde_json::json!({
            "title": "fix flaky test",
            "waypoints": [wp("define", "done"), wp("fix", "current")],
        }))
        .unwrap();
        let unnamed = parse(serde_json::json!({
            "waypoints": [wp("define", "done"), wp("fix", "done")],
        }))
        .unwrap();

        let merged = merge_trail(Some(named.clone()), unnamed.clone());
        assert_eq!(merged.title, "fix flaky test");
        assert_eq!(merged.waypoints[1].status, WaypointStatus::Done);

        // No previous trail: stays unnamed. A named update always wins.
        assert_eq!(merge_trail(None, unnamed).title, "");
        assert_eq!(merge_trail(Some(named), parse(serde_json::json!({
            "title": "better name",
            "waypoints": [wp("a", "current"), wp("b", "ahead")],
        })).unwrap()).title, "better name");
    }

    #[tokio::test]
    async fn untitled_invoke_succeeds_and_teaches() {
        let out = TrailTool::new()
            .invoke(serde_json::json!({
                "waypoints": [wp("define", "current"), wp("review", "ahead")],
            }))
            .await
            .unwrap();
        assert!(out.starts_with("Trail:"), "{out}");
        assert!(out.contains("tip: include `title`"), "{out}");
    }

    #[test]
    fn rejects_bad_shapes() {
        // Too few / too many waypoints.
        assert!(parse(serde_json::json!({
            "title": "t", "waypoints": [wp("only", "current")],
        }))
        .is_none());
        let many: Vec<_> = (0..8).map(|i| wp(&format!("s{i}"), "ahead")).collect();
        assert!(parse(serde_json::json!({ "title": "t", "waypoints": many })).is_none());
        // Two currents.
        assert!(parse(serde_json::json!({
            "title": "t",
            "waypoints": [wp("a", "current"), wp("b", "current")],
        }))
        .is_none());
        // Done after an unfinished stage — out of order.
        assert!(parse(serde_json::json!({
            "title": "t",
            "waypoints": [wp("a", "current"), wp("b", "done")],
        }))
        .is_none());
    }

    #[tokio::test]
    async fn invoke_renders_the_route() {
        let out = TrailTool::new()
            .invoke(serde_json::json!({
                "title": "fix flaky sse retry test",
                "waypoints": [wp("define", "done"), wp("implement", "current"), wp("review", "ahead")],
            }))
            .await
            .unwrap();
        assert!(out.contains("Trail “fix flaky sse retry test”"), "{out}");
        assert!(out.contains("[x] define"), "{out}");
        assert!(out.contains("[>] implement"), "{out}");
        assert!(out.contains("[ ] review"), "{out}");
    }

    #[test]
    fn name_and_schema_shape() {
        assert_eq!(TrailTool::NAME, TRAIL_TOOL);
        let schema = crate::schema_for::<TrailArgs>();
        // Only the waypoints are load-bearing; `title` is advertised but
        // optional (see the model-dialects corpus for why).
        assert_eq!(schema["required"][0], "waypoints");
        assert_eq!(schema["required"].as_array().map(|a| a.len()), Some(1));
        let status = &schema["properties"]["waypoints"]["items"]["properties"]["status"];
        assert_eq!(status["enum"][1], "current");
    }
}
