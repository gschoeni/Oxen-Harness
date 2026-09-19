//! `map_agents` — code-driven fan-out: one agent (or one cheap tool-less
//! call) per item, from a template, with coverage guaranteed by
//! construction. The model declares the items and the work; the harness
//! makes exactly one lane per item, so nothing is skipped because the
//! model forgot to spawn it — the finding behind the recursive-agent
//! harness results, where a per-item program beat "spawn some agents" by
//! a wide margin.
//!
//! Every item produces exactly one result; a failed item is a row with a
//! failure, never a missing row. An explicit run_id resumes completed rows
//! when the rendered input, source content, model, and schema still match. `reduce` folds
//! the rows: concatenated (default) or through one more agent given the
//! rows and a reduce prompt.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_trait::async_trait;
use harness_tools::{CallContext, Concurrency, ToolError, TypedTool};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::fleet::{FleetLimits, FleetSink, SubagentTask};
use crate::fleet_tool::{inputs_section, next_fleet_id, FleetSpawner, MAX_FLEET_AGENTS};
use crate::lane::{render_results, LaneStatus, SubagentResult};

pub const MAP_AGENTS_TOOL: &str = "map_agents";

/// Most items one call may fan over.
pub const MAX_MAP_ITEMS: usize = 64;

/// Lanes at once by default (the fleet default) and at most.
const DEFAULT_MAP_PARALLEL: usize = 3;

/// Arguments for `map_agents`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MapAgentsArgs {
    /// The items to fan over (1-64): file paths, names, questions, or parked
    /// content handles (`<<ccr:HASH>>` markers from a retrieve_original
    /// `chunks` listing). One agent per item.
    pub items: Vec<String>,
    /// Resume a prior map run by the run_id printed in its result. Omit to
    /// start fresh. Completed rows are reused only within this explicit run.
    #[serde(default)]
    pub run_id: Option<String>,
    /// Recompute rows even when resuming a run.
    #[serde(default)]
    pub refresh: bool,
    /// The task, as a template: `{{item}}` is replaced with the item and
    /// `{{index}}` with its 1-based position. A parked handle as the item
    /// is also handed to the agent as an input it reads on demand.
    pub prompt: String,
    /// Optional: make each agent's answer a JSON object with these
    /// `required` keys (see spawn_agents).
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// How to fold the rows: "concat" (default) returns every row; "agent"
    /// runs one more agent over the rows with `reduce_prompt` and returns
    /// its reply.
    #[serde(default)]
    pub reduce: Option<String>,
    /// The reduce agent's instructions, when `reduce` is "agent".
    #[serde(default)]
    pub reduce_prompt: Option<String>,
    /// Agents running at once (1-6). Defaults to 3.
    #[serde(default)]
    pub max_parallel: Option<usize>,
    /// Set true when the per-item work needs no tools — reading the item
    /// and answering. Each item is then one cheap tool-less model call
    /// (like ask_model) instead of an agent.
    #[serde(default)]
    pub leaf: Option<bool>,
    /// Default true: wait for every item and return the rows. Set false to
    /// return at once and keep working; the rows are delivered to you
    /// automatically when the run finishes — never poll for them.
    #[serde(default)]
    pub wait: Option<bool>,
}

/// The `map_agents` tool.
pub struct MapAgentsTool {
    spawner: Arc<FleetSpawner>,
    sink: Arc<dyn FleetSink>,
    /// Where a `wait: false` run leaves its rows for the agent to deliver.
    /// Without one, every run waits.
    asides: Option<harness_tools::Asides>,
    admitted: bool,
}

impl MapAgentsTool {
    pub fn new(spawner: Arc<FleetSpawner>, sink: Arc<dyn FleetSink>) -> Self {
        spawner.set_sink(sink.clone());
        Self {
            spawner,
            sink,
            asides: None,
            admitted: false,
        }
    }

    /// Let `wait: false` runs hand their rows to `asides` (the registry's
    /// queue, see `ToolRegistry::asides`).
    pub fn with_asides(mut self, asides: harness_tools::Asides) -> Self {
        self.asides = Some(asides);
        self
    }
}

/// Fill the template for one item.
fn render(template: &str, item: &str, index: usize) -> String {
    let filled = template
        .replace("{{item}}", item)
        .replace("{{index}}", &(index + 1).to_string());
    if filled == template {
        // No placeholder: the item goes after the task, so nothing is lost.
        format!("{template}\n\nItem {}: {item}", index + 1)
    } else {
        filled
    }
}

/// Whether an item is a parked-content handle rather than plain text.
/// Most characters of an item shown in its lane's label.
const LABEL_ITEM_CHARS: usize = 40;

/// What a lane is called in the agents hub and the results document: its
/// ordinal (so results map back to items, and identical items stay
/// distinct) and the head of the item itself, so a fleet over "Vikings @
/// Falcons", "Browns @ Ravens", … reads as those games, not as "item 3".
/// A parked handle says nothing about its content, so it keeps the number.
fn lane_label(index: usize, item: &str) -> String {
    if as_handle(item).is_some() {
        return format!("item {}", index + 1);
    }
    let head =
        harness_core::text::ellipsize(&harness_core::text::collapse_ws(item), LABEL_ITEM_CHARS);
    format!("{} · {head}", index + 1)
}

fn as_handle(item: &str) -> Option<&str> {
    let trimmed = item.trim();
    let inner = trimmed.strip_prefix("<<ccr:")?.strip_suffix(">>")?;
    inner.split_whitespace().next()
}

/// The memo key: the same item under the same task with the same schema.
pub(crate) fn memo_key(item: &str, prompt: &str, schema: &Option<serde_json::Value>) -> u64 {
    let mut hasher = DefaultHasher::new();
    item.hash(&mut hasher);
    prompt.hash(&mut hasher);
    schema
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

#[async_trait]
impl TypedTool for MapAgentsTool {
    const NAME: &'static str = MAP_AGENTS_TOOL;

    type Args = MapAgentsArgs;

    fn description(&self) -> &str {
        "Run the same task over a list of items, one agent per item, and get one result per \
         item back — coverage is guaranteed: nothing is skipped or merged. Use it instead of \
         spawn_agents when the work is 'for each of these…' (files to review, chunks of a \
         parked result to read, questions to answer). `{{item}}` in the prompt is the item; \
         a <<ccr:HASH>> item is handed to its agent as parked input. Set `leaf` when no tools \
         are needed (one cheap model call per item). `reduce: \"agent\"` folds the rows through \
         one more agent with `reduce_prompt`. Use the returned run_id to resume finished rows; refresh recomputes them."
    }

    /// Fans out agents that may edit and run commands: alone in its wave,
    /// mutating to the gate.
    fn concurrency(&self) -> Concurrency {
        Concurrency::Exclusive
    }

    async fn run(&self, args: MapAgentsArgs, call: &CallContext) -> Result<String, ToolError> {
        let count = args.items.iter().filter(|i| !i.trim().is_empty()).count();
        if count == 0 || count > MAX_MAP_ITEMS {
            return Err(ToolError::InvalidArguments(format!(
                "map_agents needs 1–{MAX_MAP_ITEMS} nonempty items (got {count})"
            )));
        }
        if args.prompt.trim().is_empty() {
            return Err(ToolError::InvalidArguments("write a task in prompt".into()));
        }
        if args
            .reduce
            .as_deref()
            .is_some_and(|r| r != "concat" && r != "agent")
        {
            return Err(ToolError::InvalidArguments(
                "reduce must be concat or agent".into(),
            ));
        }
        if args.reduce.as_deref() == Some("agent")
            && args
                .reduce_prompt
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
        {
            return Err(ToolError::InvalidArguments(
                "reduce: agent needs a reduce_prompt".into(),
            ));
        }
        if !self.admitted {
            let admission = self.spawner.admit_fleet(0)?;
            let me = Self {
                spawner: self.spawner.operation(),
                sink: self.sink.clone(),
                asides: None,
                admitted: true,
            };
            if args.wait == Some(false) {
                if let Some(asides) = self.asides.clone() {
                    let call = call.clone();
                    tokio::spawn(async move {
                        let _admission = admission;
                        let body = match me.run(args, &call).await {
                            Ok(text) => text,
                            Err(e) => format!("the map run failed: {e}"),
                        };
                        asides.push(harness_tools::Aside {
                            kind: "map".into(),
                            title: format!("map_agents over {count} items finished"),
                            body,
                        });
                    });
                    return Ok(format!("map_agents started over {count} items in the background. Results arrive automatically; keep working."));
                }
            }
            let _admission = admission;
            return me.run(args, call).await;
        }
        let items: Vec<String> = args
            .items
            .into_iter()
            .map(|i| i.trim().to_string())
            .filter(|i| !i.is_empty())
            .collect();
        let reduce_with_agent = matches!(args.reduce.as_deref(), Some("agent"));
        let leaf = args.leaf.unwrap_or(false);
        let run_id = args.run_id.clone().unwrap_or_else(next_fleet_id);
        let model = self.spawner.endpoint_snapshot().1.for_subagent().model;
        let session = self.spawner.session().unwrap_or_default();
        let key_for = |index: usize, item: &str| {
            let file = self
                .spawner
                .root()
                .and_then(|root| harness_tools::Workspace::new(root).ok())
                .and_then(|ws| ws.resolve(item).ok())
                .and_then(|path| std::fs::read(path).ok());
            let context = format!(
                "{session}\n{run_id}\n{model}\n{leaf}\n{}\n{:?}",
                render(&args.prompt, item, index),
                file.map(|bytes| {
                    let mut h = DefaultHasher::new();
                    bytes.hash(&mut h);
                    h.finish()
                })
            );
            memo_key(item, &context, &args.output_schema)
        };
        let keys: Vec<_> = items
            .iter()
            .enumerate()
            .map(|(i, item)| key_for(i, item))
            .collect();
        let schema = args.output_schema;

        // Rows already answered under this exact task are replayed; only the
        // rest run. A row is filled in item order either way.
        let mut rows: Vec<Option<SubagentResult>> = items
            .iter()
            .enumerate()
            .map(|(index, _)| {
                if args.refresh {
                    None
                } else {
                    self.spawner.memo_get(keys[index])
                }
            })
            .collect();
        let pending: Vec<usize> = (0..items.len()).filter(|i| rows[*i].is_none()).collect();

        if !pending.is_empty() {
            let fresh = if leaf {
                self.run_leaves(
                    &items,
                    &pending,
                    &args.prompt,
                    &schema,
                    args.max_parallel
                        .unwrap_or(DEFAULT_MAP_PARALLEL)
                        .clamp(1, MAX_FLEET_AGENTS),
                )
                .await?
            } else {
                let parallel = args
                    .max_parallel
                    .unwrap_or(DEFAULT_MAP_PARALLEL)
                    .clamp(1, MAX_FLEET_AGENTS);
                self.run_lanes(&items, &pending, &args.prompt, &schema, parallel)
                    .await?
            };
            for (index, result) in pending.iter().zip(fresh) {
                if result.status == LaneStatus::Done
                    && (schema.is_none() || result.structured.is_some())
                {
                    self.spawner.memo_put(keys[*index], result.clone());
                }
                rows[*index] = Some(result);
            }
        }
        let rows: Vec<SubagentResult> = rows.into_iter().flatten().collect();
        let replayed = items.len() - pending.len();
        let mut out = format!("Map run: {run_id}. Resume with run_id: \"{run_id}\"; refresh: true recomputes rows.\n\n");
        if replayed > 0 {
            out.push_str(&format!(
                "NOTE: {replayed} of {} items were already answered under this task and are \
                 replayed below.\n\n",
                items.len()
            ));
        }
        let document = render_results(&rows);
        if reduce_with_agent {
            let reduced = self
                .reduce(
                    &rows,
                    args.reduce_prompt.as_deref().unwrap_or_default(),
                    &document,
                )
                .await?;
            out.push_str(&reduced);
        } else {
            out.push_str(&document);
        }
        Ok(out.trim_end().to_string())
    }
}

impl MapAgentsTool {
    /// One lane per pending item.
    async fn run_lanes(
        &self,
        items: &[String],
        pending: &[usize],
        template: &str,
        schema: &Option<serde_json::Value>,
        parallel: usize,
    ) -> Result<Vec<SubagentResult>, ToolError> {
        self.spawner
            .tree_budget()
            .admit_spawn(self.spawner.session().as_deref(), pending.len() as u32)
            .map_err(ToolError::Execution)?;
        let spill = self.spawner.overflow_store();
        let tasks: Vec<SubagentTask> = pending
            .iter()
            .map(|&index| {
                let item = &items[index];
                let mut prompt = render(template, item, index);
                if let Some(hash) = as_handle(item) {
                    prompt.push_str("\n\n");
                    prompt.push_str(&inputs_section(&[hash.to_string()], spill.as_deref()));
                }
                SubagentTask::new(lane_label(index, item), prompt).with_schema(schema.clone())
            })
            .collect();
        let labels: Vec<String> = tasks.iter().map(|t| t.label.clone()).collect();
        let fleet = next_fleet_id();
        let (results, _) = self
            .spawner
            .run_lanes(
                &self.sink,
                &fleet,
                tasks,
                FleetLimits::with_concurrency(parallel),
                {
                    let spawner = self.spawner.clone();
                    let fleet = fleet.clone();
                    move |index: usize, cancel: CancellationToken| {
                        spawner.build_agent(&labels[index], &fleet, None, cancel)
                    }
                },
            )
            .await?;
        Ok(results)
    }

    /// One tool-less call per pending item, typed like a lane's result so
    /// the rows read the same either way.
    async fn run_leaves(
        &self,
        items: &[String],
        pending: &[usize],
        template: &str,
        schema: &Option<serde_json::Value>,
        parallel: usize,
    ) -> Result<Vec<SubagentResult>, ToolError> {
        let store = self.spawner.overflow_store();
        let tasks = pending
            .iter()
            .map(|&index| {
                let item = &items[index];
                let mut user = render(template, item, index);
                if let Some(hash) = as_handle(item) {
                    let content = store.as_ref().and_then(|s| s.get(hash)).ok_or_else(|| {
                        ToolError::InvalidArguments(format!("nothing is parked under {hash}"))
                    })?;
                    user = format!("## Input\n\n{content}\n\n---\n\n{user}");
                }
                Ok(SubagentTask::new(lane_label(index, item), user).with_schema(schema.clone()))
            })
            .collect::<Result<Vec<_>, ToolError>>()?;
        self.spawner
            .run_leaf_tasks_admitted(tasks, parallel, crate::ask_tool::DEFAULT_SYSTEM)
            .await
    }

    /// Fold the rows through one more lane.
    async fn reduce(
        &self,
        rows: &[SubagentResult],
        reduce_prompt: &str,
        document: &str,
    ) -> Result<String, ToolError> {
        self.spawner
            .tree_budget()
            .admit_spawn(self.spawner.session().as_deref(), 1)
            .map_err(ToolError::Execution)?;
        let prompt = format!(
            "{reduce_prompt}\n\n## The rows to reduce ({} items)\n\n{document}",
            rows.len()
        );
        let fleet = next_fleet_id();
        let (results, cancelled) = self
            .spawner
            .run_lanes(
                &self.sink,
                &fleet,
                vec![SubagentTask::new("reduce", prompt)],
                FleetLimits::with_concurrency(1),
                {
                    let spawner = self.spawner.clone();
                    let fleet = fleet.clone();
                    move |_: usize, cancel: CancellationToken| {
                        spawner.build_agent("reduce", &fleet, None, cancel)
                    }
                },
            )
            .await?;
        let mut out = String::new();
        if cancelled {
            out.push_str("NOTE: the reduce agent was stopped before finishing.\n\n");
        }
        out.push_str(&render_results(&results));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_lane_is_named_for_its_item() {
        assert_eq!(
            super::lane_label(2, "Vikings @ Falcons"),
            "3 · Vikings @ Falcons"
        );
        assert_eq!(
            super::lane_label(
                0,
                "  a  long\nitem that goes on well past the label width  "
            ),
            "1 · a long item that goes on well past the l…"
        );
        assert_eq!(super::lane_label(4, "<<ccr:abc chunk>>"), "item 5");
    }

    use std::sync::Arc;

    use harness_llm::OxenClient;
    use harness_store::{HistoryStore, SessionMeta};
    use harness_tools::{ToolRegistry, TypedTool};

    use super::*;
    use crate::fleet::FleetEvent;
    use crate::test_support::sse_prose;
    use crate::AgentConfig;

    #[derive(Default)]
    struct QuietSink;
    impl FleetSink for QuietSink {
        fn started(&self, _: &str, _: &[String], _: CancellationToken) {}
        fn event(&self, _: &str, _: &FleetEvent) {}
        fn finished(&self, _: &str) {}
    }

    #[test]
    fn templates_fill_or_append_the_item() {
        assert_eq!(
            render("review {{item}} (#{{index}})", "a.rs", 1),
            "review a.rs (#2)"
        );
        assert_eq!(render("review it", "a.rs", 0), "review it\n\nItem 1: a.rs");
        assert_eq!(as_handle("<<ccr:abc123 chunk>>"), Some("abc123"));
        assert_eq!(as_handle("src/lib.rs"), None);
    }

    #[tokio::test]
    async fn explicit_resume_invalidates_changed_files_and_refresh_bypasses_cache() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("reviewed"))
            .expect(4)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), "before").unwrap();
        let spawner = Arc::new(
            FleetSpawner::new(
                OxenClient::new(server.url(), "k", "m"),
                ToolRegistry::new(),
                AgentConfig::default(),
            )
            .with_workspace(dir.path()),
        );
        let tool = MapAgentsTool::new(spawner, Arc::new(QuietSink));
        let args = serde_json::json!({"items": ["file"], "prompt": "review {{item}}", "run_id":"review", "leaf":true});
        tool.invoke(args.clone()).await.unwrap();
        assert!(tool
            .invoke(args.clone())
            .await
            .unwrap()
            .contains("already answered"));
        std::fs::write(dir.path().join("file"), "after").unwrap();
        assert!(!tool
            .invoke(args.clone())
            .await
            .unwrap()
            .contains("already answered"));
        let mut refresh = args.clone();
        refresh["refresh"] = true.into();
        assert!(!tool
            .invoke(refresh)
            .await
            .unwrap()
            .contains("already answered"));
        let mut fresh = args;
        fresh.as_object_mut().unwrap().remove("run_id");
        assert!(!tool
            .invoke(fresh)
            .await
            .unwrap()
            .contains("already answered"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn every_item_gets_a_row_and_finished_rows_are_replayed() {
        let mut server = mockito::Server::new_async().await;
        let mut mocks = Vec::new();
        for tag in ["ITEM-a", "ITEM-b"] {
            mocks.push(
                server
                    .mock("POST", "/chat/completions")
                    .match_body(mockito::Matcher::Regex(tag.into()))
                    .with_status(200)
                    .with_header("content-type", "text/event-stream")
                    .with_body(sse_prose(&format!("{tag} looks fine")))
                    // Once: the second call replays these from the memo.
                    .expect(1)
                    .create_async()
                    .await,
            );
        }
        let failing = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex("ITEM-c".into()))
            .with_status(500)
            .with_body("boom")
            .expect_at_least(1)
            .create_async()
            .await;

        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let parent = store.create_session(&SessionMeta::default()).unwrap();
        let spawner = Arc::new(
            FleetSpawner::new(
                OxenClient::new(server.url(), "k", "claude-opus-4-8"),
                ToolRegistry::new(),
                AgentConfig {
                    system_prompt: None,
                    retry: crate::test_support::fast_retry(1),
                    ..AgentConfig::default()
                },
            )
            .with_store(store.clone())
            .with_session(parent.clone()),
        );
        let tool = MapAgentsTool::new(spawner.clone(), Arc::new(QuietSink));
        let args = serde_json::json!({
            "items": ["ITEM-a", "ITEM-b", "ITEM-c"],
            "prompt": "Check {{item}}.", "run_id": "test-map"
        });
        let out = tool.invoke(args.clone()).await.unwrap();
        // One row per item, in item order, the failed one a typed row.
        assert!(out.contains("### 1 · ITEM-a — done"), "{out}");
        assert!(out.contains("ITEM-a looks fine"), "{out}");
        assert!(out.contains("### 2 · ITEM-b — done"), "{out}");
        assert!(out.contains("### 3 · ITEM-c — failed (provider)"), "{out}");
        assert_eq!(store.lanes_of(&parent).unwrap().len(), 3);

        // Re-issued: a and b replay from the memo (their mocks admit one call
        // each), only c runs again.
        let again = tool.invoke(args).await.unwrap();
        assert!(
            again.contains("NOTE: 2 of 3 items were already answered"),
            "{again}"
        );
        assert!(again.contains("ITEM-b looks fine"), "{again}");
        for mock in &mocks {
            mock.assert_async().await;
        }
        failing.assert_async().await;
        assert_eq!(
            store.lanes_of(&parent).unwrap().len(),
            4,
            "only c ran again"
        );

        // A fresh spawner for the same session (a restart, a resume) finds
        // the memo in the store: a and b still replay.
        let reborn = Arc::new(
            FleetSpawner::new(
                OxenClient::new(server.url(), "k", "claude-opus-4-8"),
                ToolRegistry::new(),
                AgentConfig {
                    system_prompt: None,
                    retry: crate::test_support::fast_retry(1),
                    ..AgentConfig::default()
                },
            )
            .with_store(store.clone())
            .with_session(parent.clone()),
        );
        let again = MapAgentsTool::new(reborn, Arc::new(QuietSink))
            .invoke(serde_json::json!({
                "items": ["ITEM-a", "ITEM-b"],
                "prompt": "Check {{item}}.", "run_id": "test-map"
            }))
            .await
            .unwrap();
        assert!(
            again.contains("NOTE: 2 of 2 items were already answered"),
            "{again}"
        );
    }

    #[tokio::test]
    async fn leaf_items_are_cheap_calls_over_parked_chunks_and_can_be_reduced() {
        let mut server = mockito::Server::new_async().await;
        for (tag, answer) in [("CHUNK-ONE", "one: 2 uses"), ("CHUNK-TWO", "two: 0 uses")] {
            server
                .mock("POST", "/chat/completions")
                .match_body(mockito::Matcher::AllOf(vec![
                    mockito::Matcher::Regex(tag.into()),
                    mockito::Matcher::Regex("count the uses".into()),
                ]))
                .with_status(200)
                .with_header("content-type", "text/event-stream")
                .with_body(sse_prose(answer))
                .expect(1)
                .create_async()
                .await;
        }
        // The reduce agent sees both rows.
        let reduce = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("one: 2 uses".into()),
                mockito::Matcher::Regex("two: 0 uses".into()),
                mockito::Matcher::Regex("add them up".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("2 uses in total"))
            .expect(1)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let tools =
            ToolRegistry::default_for_workspace(harness_tools::Workspace::new(dir.path()).unwrap());
        let overflow = tools.overflow_store().cloned().unwrap();
        let one = overflow.put("CHUNK-ONE: fn a() { b(); b(); }");
        let two = overflow.put("CHUNK-TWO: nothing here");
        let spawner = Arc::new(FleetSpawner::new(
            OxenClient::new(server.url(), "k", "claude-opus-4-8"),
            tools,
            AgentConfig {
                system_prompt: None,
                ..AgentConfig::default()
            },
        ));
        let out = MapAgentsTool::new(spawner.clone(), Arc::new(QuietSink))
            .invoke(serde_json::json!({
                "items": [format!("<<ccr:{one} chunk>>"), format!("<<ccr:{two} chunk>>")],
                "prompt": "count the uses of b in this chunk",
                "leaf": true,
                "reduce": "agent",
                "reduce_prompt": "add them up"
            }))
            .await
            .unwrap();
        reduce.assert_async().await;
        assert!(out.contains("### reduce — done"), "{out}");
        assert!(out.contains("2 uses in total"), "{out}");
        // Two leaf calls + one lane call charged the tree; one lane spawned.
        let usage = spawner.tree_budget().usage();
        assert_eq!(usage.requests, 3);
        assert_eq!(usage.spawns, 1);
    }
}
