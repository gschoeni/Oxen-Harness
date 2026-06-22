//! The agentic loop for oxen-harness.
//!
//! This crate sits above [`harness_llm`], [`harness_tools`], and
//! [`harness_store`] and wires them into the runtime loop:
//!
//! 1. Add the user's message to the transcript (and persist it).
//! 2. Call the model (streaming) with the available tool definitions.
//! 3. If the model requested tool calls, execute each tool, append the results
//!    as `tool` messages, and loop.
//! 4. Otherwise, return the assistant's final text.
//!
//! Every message (user, assistant, tool) is persisted verbatim to the history
//! store as it is produced.

use std::sync::Arc;

use harness_core::DEFAULT_MODEL;
use harness_llm::stream::StreamEvent;
use harness_llm::types::ChatMessage;
use harness_llm::{ChatRequest, LlmError, OxenClient, ToolCall};
use harness_store::{HistoryError, HistoryStore};
use harness_tools::{ToolError, ToolRegistry};

/// Errors that can arise while running the agent loop.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("reached max iterations ({0}) without a final response")]
    MaxIterations(usize),
}

/// Events surfaced to the caller (e.g. the REPL) as a turn progresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// An incremental piece of assistant text from the stream.
    Token(String),
    /// A tool is about to run, with its name and JSON arguments.
    ToolStart { name: String, arguments: String },
    /// A tool finished, with its (possibly truncated for display) result.
    ToolEnd { name: String, result: String },
}

/// Configuration for an [`Agent`].
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_iterations: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            system_prompt: Some(default_system_prompt()),
            max_iterations: 25,
        }
    }
}

fn default_system_prompt() -> String {
    "You are oxen-harness, an open source coding agent working in the user's \
     project directory. Available tools: `find_files` (locate files by glob), \
     `search_files` (regex content search), `read_file` (line-numbered, supports \
     offset/limit), `write_file`, `edit_file` (exact-string patch), `run_shell`, \
     and `git`.\n\n\
     Guidelines:\n\
     - Prefer the dedicated tools over shell equivalents: use `find_files` not \
       `find`/`ls`, `search_files` not `grep`, `read_file` not `cat`, and \
       `edit_file`/`write_file` not `sed`/redirects.\n\
     - Always `read_file` before editing it; `edit_file` needs `old_string` to \
       match the real content exactly. Never include `read_file`'s line-number \
       and tab prefix in edit arguments.\n\
     - Work in small, verifiable steps. Run tests/builds and read the real output \
       rather than assuming success. Fix root causes, not symptoms.\n\
     - Make independent tool calls together when they don't depend on each other."
        .to_string()
}

/// A running agent bound to a model, tool set, and history session.
pub struct Agent {
    client: OxenClient,
    tools: ToolRegistry,
    store: Arc<HistoryStore>,
    session_id: String,
    config: AgentConfig,
    messages: Vec<ChatMessage>,
}

impl Agent {
    /// Construct an agent. Seeds the transcript with the system prompt (if any)
    /// and persists it to the session.
    pub fn new(
        client: OxenClient,
        tools: ToolRegistry,
        store: Arc<HistoryStore>,
        session_id: String,
        config: AgentConfig,
    ) -> Result<Self, AgentError> {
        let mut messages = Vec::new();
        if let Some(prompt) = &config.system_prompt {
            let system = ChatMessage::system(prompt.clone());
            store.append_message(&session_id, &system)?;
            messages.push(system);
        }
        Ok(Self {
            client,
            tools,
            store,
            session_id,
            config,
            messages,
        })
    }

    /// Resume an existing session: load its persisted transcript from the store
    /// into memory so subsequent turns continue the same conversation.
    ///
    /// Unlike [`Agent::new`], this seeds no system prompt and persists nothing —
    /// the history (including the original system prompt) already lives in the
    /// store and is appended to from where it left off.
    pub fn resume_from_store(
        client: OxenClient,
        tools: ToolRegistry,
        store: Arc<HistoryStore>,
        session_id: String,
        config: AgentConfig,
    ) -> Result<Self, AgentError> {
        let raw = store.messages(&session_id)?;
        let mut messages = Vec::with_capacity(raw.len());
        for value in raw {
            messages.push(serde_json::from_value::<ChatMessage>(value)?);
        }
        Ok(Self {
            client,
            tools,
            store,
            session_id,
            config,
            messages,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The model the agent currently calls.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Switch the model used for subsequent turns.
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.config.model = model.into();
    }

    /// The current in-memory transcript.
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Run one user turn to completion, returning the assistant's final text.
    ///
    /// `on_event` is invoked for streamed tokens and tool activity so callers
    /// can render progress live.
    pub async fn run_turn<F>(
        &mut self,
        user_input: impl Into<String>,
        mut on_event: F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        self.push(ChatMessage::user(user_input.into()))?;

        for _ in 0..self.config.max_iterations {
            let request = ChatRequest::new(&self.config.model, self.messages.clone())
                .with_tools(self.tools.definitions())
                .streaming(true);

            let assembled = self
                .client
                .stream_chat(&request, |event| {
                    if let StreamEvent::Token(t) = event {
                        on_event(&AgentEvent::Token(t.clone()));
                    }
                })
                .await?;

            let assistant = ChatMessage {
                role: "assistant".into(),
                content: (!assembled.content.is_empty()).then(|| assembled.content.clone()),
                tool_calls: (!assembled.tool_calls.is_empty())
                    .then(|| assembled.tool_calls.clone()),
                tool_call_id: None,
                name: None,
            };
            self.push(assistant)?;

            if assembled.tool_calls.is_empty() {
                return Ok(assembled.content);
            }

            for call in &assembled.tool_calls {
                let result = self.run_tool(call, &mut on_event).await;
                self.push(ChatMessage::tool_result(call.id.clone(), result))?;
            }
        }

        Err(AgentError::MaxIterations(self.config.max_iterations))
    }

    async fn run_tool<F>(&self, call: &ToolCall, on_event: &mut F) -> String
    where
        F: FnMut(&AgentEvent),
    {
        on_event(&AgentEvent::ToolStart {
            name: call.function.name.clone(),
            arguments: call.function.arguments.clone(),
        });

        let result = match call.function.parsed_arguments() {
            Ok(args) => match self.tools.invoke(&call.function.name, args).await {
                Ok(output) => output,
                Err(e) => format!("tool error: {e}"),
            },
            Err(e) => format!("tool error: invalid arguments JSON: {e}"),
        };

        on_event(&AgentEvent::ToolEnd {
            name: call.function.name.clone(),
            result: result.clone(),
        });
        result
    }

    fn push(&mut self, message: ChatMessage) -> Result<(), AgentError> {
        self.store.append_message(&self.session_id, &message)?;
        self.messages.push(message);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_store::SessionMeta;

    #[test]
    fn resume_loads_persisted_transcript_without_reseeding() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = store
            .create_session(&SessionMeta {
                workspace: "/tmp/proj".into(),
                model: "claude-opus-4-8".into(),
            })
            .unwrap();
        store
            .append_message(&session, &ChatMessage::system("be helpful"))
            .unwrap();
        store
            .append_message(&session, &ChatMessage::user("hello"))
            .unwrap();

        let client = OxenClient::new("http://localhost/api/ai", "key", "claude-opus-4-8");
        let agent = Agent::resume_from_store(
            client,
            ToolRegistry::new(),
            store.clone(),
            session.clone(),
            AgentConfig::default(),
        )
        .unwrap();

        // Exactly the two persisted messages — no extra system prompt seeded.
        assert_eq!(agent.session_id(), session);
        assert_eq!(agent.messages().len(), 2);
        assert_eq!(agent.messages()[0].role, "system");
        assert_eq!(agent.messages()[1].content.as_deref(), Some("hello"));
    }
}
