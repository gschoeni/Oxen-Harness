//! `ask_model` — parallel tool-less completions on the smol role.
//! Leaves share lane cancellation, budgets, and durable result history.

use std::sync::Arc;

use async_trait::async_trait;
use harness_tools::{ToolError, TypedTool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::fleet_tool::FleetSpawner;

pub const ASK_MODEL_TOOL: &str = "ask_model";

/// Most prompts one call may batch.
pub const MAX_ASK_PROMPTS: usize = 32;

/// Prompts answered at once.
pub(crate) const ASK_CONCURRENCY: usize = 6;

/// Most characters of one answer that come back — or its share of
/// [`REPLY_CHARS`] when that is tighter, so a 32-prompt call still fits.
const ANSWER_CHARS: usize = 8_000;

/// Most characters the whole numbered reply may take (the agent's parking
/// cap; this tool's results are exempt from parking because they hold to it).
const REPLY_CHARS: usize = 30_000;

/// Arguments for `ask_model`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskModelArgs {
    /// The questions or instructions, one completion each (1-32). Each is
    /// answered on its own with only what you give it here.
    pub prompts: Vec<String>,
    /// Optional parked content (`<<ccr:HASH>>` markers or bare hashes) handed
    /// whole to every prompt — a chunk from retrieve_original `chunks`, an
    /// oversized result, an agent's reply.
    #[serde(default)]
    pub inputs: Option<Vec<String>>,
    /// Optional system instruction for every call (default: answer directly
    /// and concisely from the material given).
    #[serde(default)]
    pub system: Option<String>,
}

/// The `ask_model` tool.
pub struct AskModelTool {
    spawner: Arc<FleetSpawner>,
}

impl AskModelTool {
    pub fn new(spawner: Arc<FleetSpawner>) -> Self {
        Self { spawner }
    }
}

pub(crate) const DEFAULT_SYSTEM: &str = "Answer directly and concisely from the material given. \
    If the material does not contain the answer, say so in one line.";

#[async_trait]
impl TypedTool for AskModelTool {
    const NAME: &'static str = ASK_MODEL_TOOL;

    type Args = AskModelArgs;

    fn description(&self) -> &str {
        "Ask a model several questions at once, cheaply: each prompt is one tool-less \
         completion with no memory of this conversation, answered in parallel, results back \
         numbered. Give parked content as `inputs` (<<ccr:HASH>> markers) and every prompt \
         gets it whole — this is how to read a chunk you never want in your own context: split \
         it with retrieve_original chunks, then ask_model over the pieces. Use it for \
         extraction, classification, summarising, or checking; spawn_agents is for work that \
         needs tools."
    }

    async fn run(&self, args: AskModelArgs) -> Result<String, ToolError> {
        let prompts: Vec<String> = args
            .prompts
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if prompts.is_empty() {
            return Err(ToolError::InvalidArguments(
                "ask_model needs at least one prompt".into(),
            ));
        }
        if prompts.len() > MAX_ASK_PROMPTS {
            return Err(ToolError::InvalidArguments(format!(
                "ask_model answers at most {MAX_ASK_PROMPTS} prompts per call (got {})",
                prompts.len()
            )));
        }
        let material = self.material(args.inputs.as_deref().unwrap_or_default())?;
        let system = args
            .system
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SYSTEM.to_string());
        let tasks = prompts
            .into_iter()
            .enumerate()
            .map(|(index, prompt)| {
                let user = match &material {
                    Some(material) => format!("{material}\n\n---\n\n{prompt}"),
                    None => prompt,
                };
                crate::fleet::SubagentTask::new((index + 1).to_string(), user)
            })
            .collect();
        let results = self
            .spawner
            .run_leaf_tasks(tasks, ASK_CONCURRENCY, &system)
            .await?;
        let cap = ANSWER_CHARS.min(REPLY_CHARS / results.len().max(1));
        Ok(results
            .iter()
            .map(|r| {
                let label = if r.status == crate::LaneStatus::Done {
                    r.label.clone()
                } else {
                    format!("{} · {}", r.label, r.status_line())
                };
                format!(
                    "### {}\n{}",
                    label,
                    harness_core::text::truncate_with_marker(
                        &r.summary,
                        cap,
                        &format!("\n… [read_agent {} has the full answer]", r.id)
                    )
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"))
    }
}

impl AskModelTool {
    /// The inputs' content, concatenated and labelled, or `None` when none
    /// were given. An unknown handle is an error the caller can act on.
    fn material(&self, inputs: &[String]) -> Result<Option<String>, ToolError> {
        if inputs.is_empty() {
            return Ok(None);
        }
        let store = self.spawner.overflow_store().ok_or_else(|| {
            ToolError::Execution("no parked content is available in this session".into())
        })?;
        let mut material = String::new();
        for (index, input) in inputs.iter().enumerate() {
            let hash = input
                .trim()
                .trim_start_matches("<<ccr:")
                .trim_end_matches(">>")
                .split_whitespace()
                .next()
                .unwrap_or_default();
            let content = store.get(hash).ok_or_else(|| {
                ToolError::InvalidArguments(format!(
                    "nothing is parked under <<ccr:{hash}>> — use a marker from an earlier result"
                ))
            })?;
            material.push_str(&format!("## Input {}\n\n{}\n\n", index + 1, content.trim()));
        }
        Ok(Some(material.trim_end().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use harness_llm::OxenClient;
    use harness_tools::{ToolRegistry, TypedTool};

    use super::*;
    use crate::test_support::sse_prose;
    use crate::AgentConfig;

    #[tokio::test]
    async fn queued_leaves_obey_request_budget_and_cancellation() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_prose("answer"))
            .expect(1)
            .create_async()
            .await;
        let budget = Arc::new(crate::TreeBudget::new(crate::TreeLimits {
            max_requests: 1,
            max_tokens: 10000,
            max_spawns: 24,
        }));
        let spawner = Arc::new(FleetSpawner::new(
            OxenClient::new(server.url(), "k", "m"),
            ToolRegistry::new(),
            AgentConfig {
                tree: Some(budget.clone()),
                ..Default::default()
            },
        ));
        let tool = AskModelTool::new(spawner.clone());
        let output = tool.invoke(serde_json::json!({"prompts": ["one", "two", "three", "four", "five", "six", "seven"]})).await.unwrap();
        mock.assert_async().await;
        assert!(output.contains("partial"), "{output}");
        assert_eq!(budget.usage().requests, 1);
        assert!(spawner.tree().live().is_empty());
        budget.reset();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        spawner.set_cancel(cancel);
        let output = tool
            .invoke(serde_json::json!({"prompts": ["cancelled one", "cancelled two"]}))
            .await
            .unwrap();
        assert!(output.contains("cancelled"));
        assert_eq!(budget.usage().requests, 0);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn prompts_are_answered_in_parallel_over_the_parked_material() {
        let mut server = mockito::Server::new_async().await;
        for (tag, answer) in [("Q-ONE", "one: yes"), ("Q-TWO", "two: no")] {
            server
                .mock("POST", "/chat/completions")
                .match_body(mockito::Matcher::AllOf(vec![
                    mockito::Matcher::Regex(tag.into()),
                    // Every prompt gets the whole parked chunk.
                    mockito::Matcher::Regex("PARKED-MATERIAL line 3".into()),
                    // On the cheap role, tool-less.
                    mockito::Matcher::Regex("\"model\":\"tiny\"".into()),
                ]))
                .with_status(200)
                .with_header("content-type", "text/event-stream")
                .with_body(sse_prose(answer))
                .expect(1)
                .create_async()
                .await;
        }
        let dir = tempfile::tempdir().unwrap();
        let tools =
            ToolRegistry::default_for_workspace(harness_tools::Workspace::new(dir.path()).unwrap());
        let hash = tools
            .overflow_store()
            .unwrap()
            .put("PARKED-MATERIAL line 1\nPARKED-MATERIAL line 2\nPARKED-MATERIAL line 3");
        let spawner = Arc::new(FleetSpawner::new(
            OxenClient::new(server.url(), "k", "frontier"),
            tools,
            AgentConfig {
                model: "frontier".into(),
                roles: crate::config::ModelRoles {
                    smol: Some("tiny".into()),
                    ..Default::default()
                },
                ..AgentConfig::default()
            },
        ));
        let out = AskModelTool::new(spawner.clone())
            .invoke(serde_json::json!({
                "prompts": ["Q-ONE?", "Q-TWO?"],
                "inputs": [format!("<<ccr:{hash} chunk>>")]
            }))
            .await
            .unwrap();
        assert_eq!(out, "### 1\none: yes\n\n### 2\ntwo: no");
        assert_eq!(
            spawner.tree_budget().usage().requests,
            2,
            "each call charges the tree"
        );

        let err = AskModelTool::new(spawner.clone())
            .invoke(serde_json::json!({ "prompts": ["x"], "inputs": ["<<ccr:000000000000>>"] }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("nothing is parked"), "{err}");
        assert!(AskModelTool::new(spawner)
            .invoke(serde_json::json!({ "prompts": [] }))
            .await
            .is_err());
    }
}
