//! `ask_model` — batched, tool-less model calls: the recursive-language-model
//! `llm_query_batched`. Each prompt is one completion on the cheap `smol`
//! role with no tools and no transcript; parked content the caller names as
//! `inputs` is handed to every call whole. That is the leaf of the RLM
//! pattern — the parent never loads the chunk, the leaf reads all of it and
//! answers in a line — at a fraction of a lane's cost (no tool loop, no
//! session, one round).
//!
//! Available at every depth. Charges the tree budget like a lane's call.

use std::sync::Arc;

use async_trait::async_trait;
use harness_llm::types::ChatMessage;
use harness_llm::ChatRequest;
use harness_tools::{ToolError, TypedTool};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::fleet_tool::FleetSpawner;

pub const ASK_MODEL_TOOL: &str = "ask_model";

/// Most prompts one call may batch.
pub const MAX_ASK_PROMPTS: usize = 32;

/// Prompts answered at once.
pub(crate) const ASK_CONCURRENCY: usize = 6;

/// Most characters of one answer that come back.
const ANSWER_CHARS: usize = 8_000;

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
        let tree = self.spawner.tree_budget();
        tree.admit_spawn(0).map_err(ToolError::Execution)?;

        // Fan out with a small cap; a failed prompt is one numbered error,
        // not a failed call.
        let slots = Arc::new(tokio::sync::Semaphore::new(ASK_CONCURRENCY));
        let mut join = tokio::task::JoinSet::new();
        for (index, prompt) in prompts.into_iter().enumerate() {
            let spawner = self.spawner.clone();
            let slots = slots.clone();
            let system = system.clone();
            let user = match &material {
                Some(material) => format!("{material}\n\n---\n\n{prompt}"),
                None => prompt,
            };
            join.spawn(async move {
                let _slot = slots.acquire_owned().await;
                (index, spawner.ask(&system, &user).await)
            });
        }
        let mut answers: Vec<Option<Result<String, String>>> = Vec::new();
        while let Some(joined) = join.join_next().await {
            let (index, answer) = match joined {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!("ask_model prompt task died: {e}");
                    continue;
                }
            };
            if answers.len() <= index {
                answers.resize(index + 1, None);
            }
            answers[index] = Some(answer);
        }
        let mut out = String::new();
        for (index, answer) in answers.iter().enumerate() {
            out.push_str(&format!("### {}\n", index + 1));
            match answer {
                Some(Ok(text)) => out.push_str(&harness_core::text::truncate_with_marker(
                    text.trim(),
                    ANSWER_CHARS,
                    "\n… [answer cut]",
                )),
                Some(Err(e)) => out.push_str(&format!("(failed: {e})")),
                None => out.push_str("(no answer)"),
            }
            out.push_str("\n\n");
        }
        Ok(out.trim_end().to_string())
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

impl FleetSpawner {
    /// One tool-less completion on the `smol` role, charged to the tree
    /// budget and the session's ledger. Not cancellable mid-stream: a
    /// batched question is short, and a stop lands between them.
    pub(crate) async fn ask(&self, system: &str, user: &str) -> Result<String, String> {
        let (client, config) = self.endpoint_snapshot();
        let model = config
            .roles
            .resolve(crate::config::Role::Smol, &config.model)
            .to_string();
        let messages = vec![
            ChatMessage::system(system.to_string()),
            ChatMessage::user(user.to_string()),
        ];
        let estimated_prompt = crate::budget::estimate_prompt_tokens(&messages, &[]);
        let request = ChatRequest::new(&model, messages).streaming(true);
        let assembled = client
            .stream_chat(&request, &CancellationToken::new(), |_| {})
            .await
            .map_err(|e| e.to_string())?;
        let (prompt, completion) = match &assembled.usage {
            Some(usage) => (
                usage.prompt_tokens as usize,
                usage.completion_tokens as usize,
            ),
            None => (
                estimated_prompt,
                assembled.content.chars().count() / crate::budget::CHARS_PER_TOKEN,
            ),
        };
        self.tree_budget().charge((prompt + completion) as u64);
        if let (Some(store), Some(session)) = (self.store(), self.session()) {
            let source = if harness_llm::host_from_base_url(client.base_url()) == "hub.oxen.ai" {
                "oxen_cloud"
            } else {
                "unpriced"
            };
            let _ = store.record_model_usage_detailed(
                &model,
                source,
                prompt,
                completion,
                &harness_store::UsageDetail {
                    session_id: &session,
                    kind: "oneshot",
                    ..Default::default()
                },
            );
        }
        Ok(assembled.content)
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
