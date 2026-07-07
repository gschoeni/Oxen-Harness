//! The [`Agent`] itself: construction, session lifecycle, and accessors.
//!
//! The turn loop lives in [`turn`], compression wiring in [`compression`] —
//! child modules, so the agent's fields stay private to this module tree while
//! each concern reads as one file.

mod compression;
mod turn;

use std::sync::Arc;

use harness_compress::{CcrStore, CompressConfig};
use harness_llm::types::{ChatMessage, ContentPart};
use harness_llm::{hydrate_content, Attachment, AttachmentStore, ChatRequest, OxenClient};
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::ToolRegistry;
use tokio_util::sync::CancellationToken;

use crate::budget;
use crate::config::AgentConfig;
use crate::error::AgentError;

use self::compression::setup_compression;

/// A running agent bound to a model, tool set, and history session.
pub struct Agent {
    client: OxenClient,
    tools: ToolRegistry,
    store: Arc<HistoryStore>,
    session_id: String,
    config: AgentConfig,
    messages: Vec<ChatMessage>,
    /// Where attachments are persisted + resolved, derived from
    /// [`AgentConfig::attachment_root`]. `None` inlines attachments instead.
    attachments: Option<AttachmentStore>,
    /// Cumulative estimated tokens sent + generated this run (see [`budget`]).
    tokens_used: usize,
    /// Cooperative stop signal for the in-flight turn. Replaced per turn (see
    /// [`Agent::set_cancel_token`]) so the host can cancel a streaming response
    /// without holding the agent's lock; a fresh token each turn keeps a prior
    /// turn's cancellation from poisoning the next.
    cancel: CancellationToken,
    /// Multiplier correcting the client-side token estimate toward the real
    /// counts the endpoint reports (`actual_prompt_tokens / estimated`). Starts
    /// at 1.0 and recalibrates after each call that returns usage, so the budget
    /// check (and compaction trigger) track reality rather than the crude
    /// 4-chars-per-token heuristic. Not persisted — re-learned each session.
    token_ratio: f64,
    /// Where compressed-away tool output lives while compression is `On`
    /// (shared with the `retrieve_original` tool). `None` in `Off`/`Audit`.
    ccr: Option<Arc<CcrStore>>,
    /// Compressor tunables (defaults; not yet user-configurable).
    compress_cfg: CompressConfig,
    /// Cumulative estimated tokens saved (`On`) or would-be saved (`Audit`)
    /// by compression this run.
    tokens_saved: usize,
}

impl Agent {
    /// Construct an agent. Seeds the transcript with the system prompt (if any)
    /// and persists it to the session.
    pub fn new(
        client: OxenClient,
        mut tools: ToolRegistry,
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
        let attachments = config.attachment_root.clone().map(AttachmentStore::new);
        let ccr = setup_compression(&config, &mut tools);
        Ok(Self {
            client,
            tools,
            store,
            session_id,
            config,
            messages,
            attachments,
            tokens_used: 0,
            cancel: CancellationToken::new(),
            token_ratio: 1.0,
            ccr,
            compress_cfg: CompressConfig::default(),
            tokens_saved: 0,
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
        mut tools: ToolRegistry,
        store: Arc<HistoryStore>,
        session_id: String,
        config: AgentConfig,
    ) -> Result<Self, AgentError> {
        let raw = store.messages(&session_id)?;
        let mut messages = Vec::with_capacity(raw.len());
        for value in raw {
            messages.push(serde_json::from_value::<ChatMessage>(value)?);
        }
        let attachments = config.attachment_root.clone().map(AttachmentStore::new);
        let ccr = setup_compression(&config, &mut tools);
        // Seed the cumulative count from the loaded transcript so a resumed
        // session's dashboard reflects prior usage instead of starting at 0.
        let tokens_used = budget::estimate_prompt_tokens(&messages, &tools.definitions());
        Ok(Self {
            client,
            tools,
            store,
            session_id,
            config,
            messages,
            attachments,
            tokens_used,
            cancel: CancellationToken::new(),
            token_ratio: 1.0,
            ccr,
            compress_cfg: CompressConfig::default(),
            tokens_saved: 0,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Start a fresh session on this live agent, reusing its client, tools, and
    /// config. Creates a new session row, re-seeds the system prompt, and clears
    /// the in-memory transcript so the next turn begins a new conversation.
    pub fn start_new_session(&mut self, meta: &SessionMeta) -> Result<(), AgentError> {
        let session_id = self.store.create_session(meta)?;
        let mut messages = Vec::new();
        if let Some(prompt) = &self.config.system_prompt {
            let system = ChatMessage::system(prompt.clone());
            self.store.append_message(&session_id, &system)?;
            messages.push(system);
        }
        self.session_id = session_id;
        self.messages = messages;
        self.tokens_used = 0;
        self.tokens_saved = 0;
        Ok(())
    }

    /// Switch this live agent to an existing session, loading its persisted
    /// transcript into memory. Reuses the current client, tools, and config so
    /// subsequent turns continue the loaded conversation.
    pub fn load_session(&mut self, session_id: String) -> Result<(), AgentError> {
        let raw = self.store.messages(&session_id)?;
        let mut messages = Vec::with_capacity(raw.len());
        for value in raw {
            messages.push(serde_json::from_value::<ChatMessage>(value)?);
        }
        self.session_id = session_id;
        self.messages = messages;
        // Seed the cumulative count from the loaded transcript so a resumed
        // session's dashboard reflects prior usage instead of starting at 0.
        self.tokens_used = self.context_tokens();
        Ok(())
    }

    /// The model the agent currently calls.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Switch the model used for subsequent turns.
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.config.model = model.into();
    }

    /// Swap the underlying inference client (e.g. to move a live conversation
    /// from a local `llama-server` to the cloud endpoint, or vice-versa) without
    /// disturbing the transcript or session. Pair with [`Self::set_model`] so the
    /// request's model matches the new endpoint.
    pub fn set_client(&mut self, client: OxenClient) {
        self.client = client;
    }

    /// Override (or clear, with `None`) the context window used for budgeting —
    /// e.g. after swapping a local model's small window for a cloud model's,
    /// where `None` lets it derive from the model name again.
    pub fn set_context_window(&mut self, window: Option<usize>) {
        self.config.context_window = window;
    }

    /// Install the stop signal for the next turn. The host keeps a clone so it
    /// can cancel a running turn (`token.cancel()`) without taking the agent's
    /// lock — set a fresh token before each turn so a prior cancellation doesn't
    /// carry over.
    pub fn set_cancel_token(&mut self, token: CancellationToken) {
        self.cancel = token;
    }

    /// The effective context window (tokens): the configured override, else a
    /// best-effort size derived from the model name.
    pub fn context_window(&self) -> usize {
        self.config
            .context_window
            .unwrap_or_else(|| budget::context_window_for(&self.config.model))
    }

    /// Estimated tokens the current transcript (+ tool definitions) occupies —
    /// i.e. how full the context window is right now, calibrated by the latest
    /// real usage so the meter and budget reflect actual consumption.
    pub fn context_tokens(&self) -> usize {
        self.calibrated(budget::estimate_prompt_tokens(
            &self.messages,
            &self.tools.definitions(),
        ))
    }

    /// Scale a raw client-side token estimate by the learned calibration factor.
    fn calibrated(&self, raw: usize) -> usize {
        (raw as f64 * self.token_ratio).round() as usize
    }

    /// Whether the current transcript (+ tools) is within `budget`, calibrated.
    fn fits_budget(&self, budget: usize, tool_defs: &[serde_json::Value]) -> bool {
        self.calibrated(budget::estimate_prompt_tokens(&self.messages, tool_defs)) <= budget
    }

    /// Cumulative estimated tokens sent + generated this run.
    pub fn tokens_used(&self) -> usize {
        self.tokens_used
    }

    /// The tool definitions (JSON schemas) advertised to the model on every
    /// call this turn — i.e. the tools the agent currently has available.
    pub fn tool_definitions(&self) -> Vec<serde_json::Value> {
        self.tools.definitions()
    }

    /// Run a one-shot completion that is *not* part of the session transcript
    /// (no tools, nothing persisted). Used for side tasks like generating a
    /// theme from a natural-language description, reusing the session's model
    /// and endpoint.
    pub async fn complete(&self, system: &str, user: &str) -> Result<String, AgentError> {
        let messages = vec![
            ChatMessage::system(system.to_string()),
            ChatMessage::user(user.to_string()),
        ];
        let request = ChatRequest::new(&self.config.model, messages).streaming(true);
        // A one-shot side task, not the cancellable turn loop: run it to completion.
        let assembled = self
            .client
            .stream_chat(&request, &CancellationToken::new(), |_| {})
            .await?;
        Ok(assembled.content)
    }

    /// The current in-memory transcript.
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Persist a message to the session, then append it to the in-memory
    /// transcript — the order every message takes into history.
    fn push(&mut self, message: ChatMessage) -> Result<(), AgentError> {
        self.store.append_message(&self.session_id, &message)?;
        self.messages.push(message);
        Ok(())
    }

    /// The transcript prepared for sending: a clone of the in-memory messages
    /// with any on-disk attachment references hydrated back into inline data
    /// URIs the provider can consume. When no attachment store is configured the
    /// messages already carry inline content, so this is just the clone.
    fn outbound_messages(&self) -> Vec<ChatMessage> {
        let mut messages = self.messages.clone();
        if let Some(store) = &self.attachments {
            for message in &mut messages {
                if let Some(content) = message.content.as_mut() {
                    hydrate_content(content, store.root());
                }
            }
        }
        messages
    }
}

/// Build the user message for a turn: a plain-text message when there are no
/// attachments, otherwise a multimodal message with the text followed by each
/// attachment's content part.
///
/// When `store` is `Some`, image/PDF attachments are persisted to disk and the
/// message records a project-relative path; otherwise they're inlined as data
/// URIs (legacy behavior). Returns an error only if writing an attachment fails.
fn build_user_message(
    text: String,
    attachments: &[Attachment],
    store: Option<&AttachmentStore>,
) -> Result<ChatMessage, AgentError> {
    if attachments.is_empty() {
        return Ok(ChatMessage::user(text));
    }
    let mut parts = Vec::with_capacity(attachments.len() + 1);
    if !text.is_empty() {
        parts.push(ContentPart::text(text));
    }
    for att in attachments {
        let part = match store {
            Some(store) => store.store_part(att)?,
            None => att.to_content_part(),
        };
        parts.push(part);
    }
    Ok(ChatMessage::user_parts(parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_session;

    #[test]
    fn user_message_is_plain_without_attachments_and_multimodal_with() {
        use harness_llm::types::MessageContent;

        let plain = build_user_message("hi".into(), &[], None).unwrap();
        assert!(matches!(plain.content, Some(MessageContent::Text(_))));

        let img = Attachment::from_bytes("a.png", vec![1, 2, 3]).unwrap();
        let multi = build_user_message("look".into(), std::slice::from_ref(&img), None).unwrap();
        match multi.content {
            Some(MessageContent::Parts(parts)) => assert_eq!(parts.len(), 2), // text + image
            other => panic!("expected multimodal parts, got {other:?}"),
        }
    }

    #[test]
    fn stored_attachment_is_referenced_by_path_then_hydrated_for_sending() {
        use harness_llm::types::{ContentPart, MessageContent};

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = store
            .create_session(&SessionMeta {
                workspace: dir.path().display().to_string(),
                model: "claude-opus-4-8".into(),
                ..Default::default()
            })
            .unwrap();
        let client = OxenClient::new("http://localhost/api/ai", "key", "claude-opus-4-8");
        let config = AgentConfig {
            attachment_root: Some(dir.path().to_path_buf()),
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        let img = Attachment::from_bytes("a.png", vec![9, 8, 7]).unwrap();
        let msg = build_user_message(
            "look".into(),
            std::slice::from_ref(&img),
            agent.attachments.as_ref(),
        )
        .unwrap();
        agent.push(msg).unwrap();

        // Persisted/in-memory form references a project-relative path (small).
        let stored = agent.messages().last().unwrap();
        match &stored.content {
            Some(MessageContent::Parts(parts)) => match &parts[1] {
                ContentPart::ImageUrl { image_url } => {
                    assert!(image_url.url.starts_with(".oxen-harness/attachments/"));
                    assert!(!image_url.url.contains("base64"));
                }
                other => panic!("expected image part, got {other:?}"),
            },
            other => panic!("expected parts, got {other:?}"),
        }

        // Outbound form hydrates that reference back to an inline data URI.
        let outbound = agent.outbound_messages();
        match &outbound.last().unwrap().content {
            Some(MessageContent::Parts(parts)) => match &parts[1] {
                ContentPart::ImageUrl { image_url } => {
                    assert!(image_url.url.starts_with("data:image/png;base64,"))
                }
                other => panic!("expected image part, got {other:?}"),
            },
            other => panic!("expected parts, got {other:?}"),
        }
    }

    #[test]
    fn resume_loads_persisted_transcript_without_reseeding() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
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
        assert_eq!(agent.messages()[1].content_text().as_deref(), Some("hello"));
    }
}
