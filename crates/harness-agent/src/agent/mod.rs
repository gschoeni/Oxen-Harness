//! The [`Agent`] itself: construction, session lifecycle, and accessors.
//!
//! The turn loop lives in [`turn`], compression wiring in [`compression`] —
//! child modules, so the agent's fields stay private to this module tree while
//! each concern reads as one file.

mod compression;
mod turn;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use harness_compress::{CcrStore, CompressConfig};
use harness_llm::types::{ChatMessage, ContentPart};
use harness_llm::{
    hydrate_content_bounded, Attachment, AttachmentStore, ChatRequest, OxenClient,
    MAX_OUTBOUND_ATTACHMENT_BYTES, MAX_OUTBOUND_ATTACHMENT_PARTS,
};
use harness_store::{HistoryStore, SessionMeta};
use harness_tools::ToolRegistry;
use tokio_util::sync::CancellationToken;

use crate::budget;
use crate::config::AgentConfig;
use crate::error::AgentError;

use self::compression::setup_compression;

/// Narrow a registry to what a detached subagent (a `side_agent`, a fleet
/// lane) may hold. Two tools are stripped:
///
/// - `spawn_agents` — a subagent must not spawn its own fleet, so fan-out is
///   exactly one level deep (no fork bombs, bounded cost).
/// - `ask_user_question` — a subagent has no way to drive the host's single
///   interactive prompt: N lanes asking at once would each block on a modal
///   only one of which can be shown, and since a tool call is awaited without
///   cancellation the un-shown lanes (and the whole turn) would hang forever.
///   Subagents run headless; a question is the orchestrating turn's job.
///
/// Both are host-owned, singular capabilities — this is the one place that
/// decides a subagent can't have them, so `side_agent` and the fleet
/// spawner can't drift on the policy.
pub(crate) fn subagent_tools(mut tools: ToolRegistry) -> ToolRegistry {
    tools.remove(crate::fleet_tool::FLEET_TOOL);
    tools.remove(harness_tools::ASK_USER_TOOL);
    tools
}

/// A running agent bound to a model, tool set, and history session.
pub struct Agent {
    client: OxenClient,
    tools: ToolRegistry,
    store: Arc<HistoryStore>,
    /// Persistent destination for aggregate usage. Usually the session store;
    /// detached agents keep their transcript in memory but inherit this ledger
    /// so review/fleet calls still count toward all-time model usage.
    usage_store: Arc<HistoryStore>,
    session_id: String,
    config: AgentConfig,
    messages: Vec<ChatMessage>,
    /// Highest verbatim message sequence persisted for this session.
    last_persisted_seq: i64,
    /// Detached agents need a working context, not a second SQLite transcript.
    persist_transcript: bool,
    /// Where attachments are persisted + resolved, derived from
    /// [`AgentConfig::attachment_root`]. `None` inlines attachments instead.
    attachments: Option<AttachmentStore>,
    /// Cumulative estimated tokens sent + generated this run (see [`budget`]).
    tokens_used: usize,
    /// Cumulative prompt (input) tokens this run, tracked separately from
    /// completion tokens so cost can be priced at the model's distinct
    /// input/output rates. Sums to `tokens_used` with `completion_tokens_used`.
    prompt_tokens_used: usize,
    /// Cumulative completion (output) tokens this run (see above).
    completion_tokens_used: usize,
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
    /// Compressed projections keyed by original hash; avoids reparsing the same
    /// stale JSON/log output on every model call.
    compression_cache: HashMap<String, Option<String>>,
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
        let mut last_persisted_seq = -1;
        if let Some(prompt) = &config.system_prompt {
            let system = ChatMessage::system(prompt.clone());
            last_persisted_seq = store.append_message(&session_id, &system)?;
            messages.push(system);
        }
        let attachments = config.attachment_root.clone().map(AttachmentStore::new);
        let ccr = setup_compression(&config, &mut tools);
        Ok(Self {
            client,
            tools,
            usage_store: store.clone(),
            store,
            session_id,
            config,
            messages,
            last_persisted_seq,
            persist_transcript: true,
            attachments,
            tokens_used: 0,
            prompt_tokens_used: 0,
            completion_tokens_used: 0,
            cancel: CancellationToken::new(),
            token_ratio: 1.0,
            ccr,
            compress_cfg: CompressConfig::default(),
            compression_cache: HashMap::new(),
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
        let (through_seq, mut messages) = store
            .context_snapshot::<Vec<ChatMessage>>(&session_id)?
            .unwrap_or((-1, Vec::new()));
        let later = store.messages_typed_after::<ChatMessage>(&session_id, through_seq)?;
        let last_persisted_seq = through_seq + later.len() as i64;
        messages.extend(later);
        let attachments = config.attachment_root.clone().map(AttachmentStore::new);
        let ccr = setup_compression(&config, &mut tools);
        // Seed the cumulative count from the loaded transcript so a resumed
        // session's dashboard reflects prior usage instead of starting at 0.
        let tokens_used = budget::estimate_prompt_tokens(&messages, &tools.definitions());
        Ok(Self {
            client,
            tools,
            usage_store: store.clone(),
            store,
            session_id,
            config,
            messages,
            last_persisted_seq,
            persist_transcript: true,
            attachments,
            tokens_used,
            // The split input/output counters price only tokens we actually
            // observe flowing this run; we can't reliably split a whole-transcript
            // estimate into prompt vs completion, so they start at 0 on resume.
            prompt_tokens_used: 0,
            completion_tokens_used: 0,
            cancel: CancellationToken::new(),
            token_ratio: 1.0,
            ccr,
            compress_cfg: CompressConfig::default(),
            compression_cache: HashMap::new(),
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
        self.last_persisted_seq = self.messages.len() as i64 - 1;
        self.tokens_used = 0;
        self.prompt_tokens_used = 0;
        self.completion_tokens_used = 0;
        self.tokens_saved = 0;
        Ok(())
    }

    /// Switch this live agent to an existing session, loading its persisted
    /// transcript into memory. Reuses the current client, tools, and config so
    /// subsequent turns continue the loaded conversation.
    pub fn load_session(&mut self, session_id: String) -> Result<(), AgentError> {
        let (through_seq, mut messages) = self
            .store
            .context_snapshot::<Vec<ChatMessage>>(&session_id)?
            .unwrap_or((-1, Vec::new()));
        let later = self
            .store
            .messages_typed_after::<ChatMessage>(&session_id, through_seq)?;
        self.last_persisted_seq = through_seq + later.len() as i64;
        messages.extend(later);
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

    /// Cumulative prompt (input) tokens observed this run, for cost pricing.
    pub fn prompt_tokens_used(&self) -> usize {
        self.prompt_tokens_used
    }

    /// Cumulative completion (output) tokens observed this run, for cost pricing.
    pub fn completion_tokens_used(&self) -> usize {
        self.completion_tokens_used
    }

    /// Route aggregate usage to `store` without changing where this agent's
    /// transcript lives. Used by detached review/fleet agents.
    pub(crate) fn set_usage_store(&mut self, store: Arc<HistoryStore>) {
        self.usage_store = store;
    }

    pub(crate) fn disable_transcript_persistence(&mut self) {
        self.persist_transcript = false;
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
        let raw_prompt = budget::estimate_prompt_tokens(&request.messages, &[]);
        let (prompt, completion) = match assembled.usage {
            Some(usage) if usage.prompt_tokens + usage.completion_tokens > 0 => (
                usage.prompt_tokens as usize,
                usage.completion_tokens as usize,
            ),
            _ => (
                raw_prompt,
                budget::estimate_completion_tokens(&assembled.content, &assembled.tool_calls),
            ),
        };
        self.record_usage(prompt, completion);
        Ok(assembled.content)
    }

    /// Spin up a detached agent for an isolated side task (e.g. one step of a
    /// code-review pipeline): same client, tools, and config as this agent, but
    /// backed by an in-memory store, so nothing it does touches the user's
    /// session, history, or context window. Its tool set is narrowed by
    /// `subagent_tools` (no recursion, no interactive tools).
    pub fn side_agent(&self) -> Result<Agent, AgentError> {
        let store = Arc::new(HistoryStore::open_in_memory()?);
        let session = store.create_session(&SessionMeta {
            model: self.config.model.clone(),
            ..Default::default()
        })?;
        let mut side = Agent::new(
            self.client.clone(),
            subagent_tools(self.tools.clone()),
            store,
            session,
            self.config.clone(),
        )?;
        side.disable_transcript_persistence();
        side.set_usage_store(self.usage_store.clone());
        Ok(side)
    }

    /// Persist one model call in the shared per-model ledger. Accounting is
    /// best-effort and must never turn a successful inference into a failed turn.
    fn record_usage(&self, prompt_tokens: usize, completion_tokens: usize) {
        let _ = self.usage_store.record_model_usage(
            &self.config.model,
            self.usage_source(),
            prompt_tokens,
            completion_tokens,
        );
        let total = prompt_tokens.saturating_add(completion_tokens);
        if total > 0 {
            let _ = self
                .usage_store
                .meta_add_i64("total_tokens_used", total as i64);
        }
    }

    /// Only the public Oxen hub catalog has rates this harness can apply. Local
    /// llama-server and custom/self-hosted endpoints remain explicitly
    /// unpriced instead of being mislabeled as free cloud usage.
    fn usage_source(&self) -> &'static str {
        if harness_llm::host_from_base_url(self.client.base_url()) == "hub.oxen.ai" {
            "oxen_cloud"
        } else {
            "unpriced"
        }
    }

    /// Record a synthetic, already-settled user/assistant exchange in the
    /// session without calling the model — how a side task's result (e.g. a
    /// code-review report) enters the conversation so follow-up turns can refer
    /// to it. Both messages persist to the store like any turn's would.
    pub fn inject_exchange(
        &mut self,
        user: impl Into<String>,
        assistant: impl Into<String>,
    ) -> Result<(), AgentError> {
        self.push(ChatMessage::user(user.into()))?;
        self.push(ChatMessage::assistant(assistant.into()))
    }

    /// The current in-memory transcript.
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Persist a message to the session, then append it to the in-memory
    /// transcript — the order every message takes into history.
    fn push(&mut self, message: ChatMessage) -> Result<(), AgentError> {
        if self.persist_transcript {
            let raw = serde_json::to_string(&message)?;
            let title: Option<Cow<'_, str>> = if message.role == "user" {
                message.content.as_ref().map(|content| match content {
                    harness_llm::types::MessageContent::Text(text) => Cow::Borrowed(text.as_str()),
                    parts => Cow::Owned(parts.as_text()),
                })
            } else {
                None
            };
            let title = title.map(|text| {
                Cow::Owned(harness_core::text::truncate_with_marker(
                    text.as_ref(),
                    512,
                    "…",
                ))
            });
            self.last_persisted_seq = self.store.append_raw_message(
                &self.session_id,
                &message.role,
                title.as_deref(),
                &raw,
            )?;
        }
        self.messages.push(message);
        Ok(())
    }

    /// Persist the compact working set separately from verbatim history.
    fn save_context_snapshot(&self) {
        if self.persist_transcript {
            let _ = self.store.save_context_snapshot(
                &self.session_id,
                self.last_persisted_seq,
                &self.messages,
            );
        }
    }

    /// The transcript prepared for sending: a clone of the in-memory messages
    /// with any on-disk attachment references hydrated back into inline data
    /// URIs the provider can consume. When no attachment store is configured the
    /// messages already carry inline content, so this is just the clone.
    fn outbound_messages(&self) -> Vec<ChatMessage> {
        let mut messages = self.messages.clone();
        if let Some(store) = &self.attachments {
            let mut remaining_bytes = MAX_OUTBOUND_ATTACHMENT_BYTES;
            let mut remaining_parts = MAX_OUTBOUND_ATTACHMENT_PARTS;
            for message in messages.iter_mut().rev() {
                if let Some(content) = message.content.as_mut() {
                    hydrate_content_bounded(
                        content,
                        store.root(),
                        &mut remaining_bytes,
                        &mut remaining_parts,
                    );
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
    let total_bytes = attachments.iter().map(|a| a.bytes.len()).sum::<usize>();
    if total_bytes > MAX_OUTBOUND_ATTACHMENT_BYTES {
        return Err(AgentError::AttachmentsTooLarge {
            size: total_bytes,
            max: MAX_OUTBOUND_ATTACHMENT_BYTES,
        });
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
    fn side_agent_is_detached_and_inject_exchange_persists() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new("http://localhost/api/ai", "key", "claude-opus-4-8");
        let mut agent = Agent::new(
            client,
            ToolRegistry::new(),
            store.clone(),
            session.clone(),
            AgentConfig::default(),
        )
        .unwrap();
        let before = store.messages(&session).unwrap().len();

        // The side agent lives in its own store/session: working it leaves the
        // parent session untouched.
        let mut side = agent.side_agent().unwrap();
        assert_ne!(side.session_id(), agent.session_id());
        assert_eq!(side.model(), agent.model());
        side.inject_exchange("scratch work", "scratch reply")
            .unwrap();
        assert_eq!(store.messages(&session).unwrap().len(), before);

        // Injecting into the real agent lands a settled pair in memory + store.
        agent
            .inject_exchange("Run a code review.", "## Findings\n(none)")
            .unwrap();
        let roles: Vec<_> = agent.messages().iter().map(|m| m.role.clone()).collect();
        assert_eq!(roles.last().unwrap(), "assistant");
        assert_eq!(roles[roles.len() - 2], "user");
        assert_eq!(store.messages(&session).unwrap().len(), before + 2);
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
