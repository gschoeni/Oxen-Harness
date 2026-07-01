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

use std::path::PathBuf;
use std::sync::Arc;

use harness_core::DEFAULT_MODEL;
use harness_llm::stream::StreamEvent;
use harness_llm::types::{ChatMessage, ContentPart};
use harness_llm::{
    hydrate_content, Attachment, AttachmentStore, ChatRequest, LlmError, OxenClient, ToolCall,
};
use harness_store::{HistoryError, HistoryStore, SessionMeta};
use harness_tools::{ToolError, ToolRegistry};
use tokio_util::sync::CancellationToken;

pub mod budget;
pub mod compact;

/// Errors that can arise while running the agent loop.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error("attachment IO failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(
        "the conversation grew past the model's context window \
         (~{used} prompt tokens, limit ~{window}); start a fresh session, \
         or switch to a model with a larger context window"
    )]
    ContextWindowExceeded { used: usize, window: usize },
}

/// Events surfaced to the caller (e.g. the REPL) as a turn progresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// An incremental piece of assistant text from the stream.
    Token(String),
    /// The model has started emitting a tool call (name known, arguments still
    /// streaming). Fires before [`AgentEvent::ToolStart`], letting the UI show
    /// progress while a long call — like writing a `canvas` document — streams.
    ToolPending { name: String },
    /// An incremental fragment of a tool call's arguments (raw JSON), tagged with
    /// the tool name — lets a UI stream the in-progress content (a file being
    /// written, a canvas document) before the call is complete.
    ToolDelta { name: String, delta: String },
    /// A tool is about to run, with its name and JSON arguments.
    ToolStart { name: String, arguments: String },
    /// A tool finished, with its (possibly truncated for display) result.
    ToolEnd { name: String, result: String },
    /// The session's cumulative token usage and current context fill, surfaced
    /// around each model call so a UI can track usage live *within* a turn (each
    /// tool-loop iteration re-sends the growing context, which this captures)
    /// rather than only at the end. Fired before a call (reflecting the prompt
    /// about to be sent) and after it (the exact figure, including the reply).
    Usage {
        tokens_used: usize,
        context_tokens: usize,
    },
    /// The transcript was compacted to fit the context window — older history
    /// was pruned and/or summarized so the session can continue instead of
    /// hitting a hard limit. Carries a short human-readable note for the UI.
    Compacted { detail: String },
}

/// Configuration for an [`Agent`].
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub system_prompt: Option<String>,
    /// Context window in tokens. `None` derives it from the model name; set it
    /// explicitly for locally-served models whose `llama-server` context is
    /// smaller than the model's theoretical maximum.
    pub context_window: Option<usize>,
    /// Tokens to keep free for the model's reply when budgeting the prompt.
    pub response_reserve: usize,
    /// Project root under which image/PDF attachments are stored on disk (so the
    /// transcript records a relative path, not inline base64). `None` keeps the
    /// legacy behavior of inlining attachments as data URIs.
    pub attachment_root: Option<PathBuf>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            // Web search off by default: only callers that actually register the
            // tool should advertise it (see `default_system_prompt`).
            system_prompt: Some(default_system_prompt(false)),
            context_window: None,
            response_reserve: 4096,
            attachment_root: None,
        }
    }
}

/// Build the default system prompt. `web_search` controls whether the
/// `web_search` tool is advertised — pass whether it's actually registered, so
/// the model is never offered (and never tries to call) a tool that the
/// registry would reject as unknown.
pub fn default_system_prompt(web_search: bool) -> String {
    system_prompt_with(web_search, false)
}

/// A note pinning the agent to a concrete working directory, so the model knows
/// the absolute project root its file tools and shell operate in rather than
/// guessing. Appended to the system prompt when the workspace is known.
pub fn environment_section(workspace: &std::path::Path) -> String {
    format!(
        "\n\nEnvironment:\n\
         - Working directory (the project root): {}\n\
         - `find_files`, `search_files`, `read_file`, `write_file`, and `edit_file` \
           resolve paths relative to this directory, and `run_shell` runs in it. \
           Prefer relative paths; use the absolute root above only when you need it.",
        workspace.display()
    )
}

/// The system prompt with an [`environment_section`] appended, pinning the
/// working directory. Use this at agent construction so every new session knows
/// its project root.
pub fn system_prompt_with_env(
    web_search: bool,
    canvas: bool,
    workspace: &std::path::Path,
) -> String {
    format!(
        "{}{}",
        system_prompt_with(web_search, canvas),
        environment_section(workspace)
    )
}

/// The system prompt, advertising the optional `web_search` and `canvas` tools
/// only when the host actually registered them.
pub fn system_prompt_with(web_search: bool, canvas: bool) -> String {
    let web_tool = if web_search {
        ", `web_search` (Brave web search)"
    } else {
        ""
    };
    let canvas_tool = if canvas {
        ", and `canvas` (show a document in a side panel)"
    } else {
        ""
    };
    let web_guideline = if web_search {
        "\n- Use `web_search` when something may be newer than your training or \
         isn't in the workspace: library/API docs, current events, or an \
         unfamiliar error."
    } else {
        ""
    };
    let canvas_guideline = if canvas {
        "\n- When you produce a substantial, self-contained deliverable the user \
         will read, iterate on, or keep — a report/article (markdown), a rendered \
         web page or interactive demo (html), a sizeable code file (code), or a \
         vector graphic (svg) — show it with `canvas` \
         instead of a long fenced block in chat. Reuse the same `id` to revise an \
         open document. Don't use `canvas` for short answers or quick snippets; \
         opening a panel for those is disruptive."
    } else {
        ""
    };
    format!(
        "You are oxen-harness, an open source coding agent working in the user's \
         project directory. Available tools: `find_files` (locate files by glob), \
         `search_files` (regex content search), `read_file` (line-numbered, supports \
         offset/limit), `write_file`, `edit_file` (exact-string patch), `run_shell`, \
         `git`, `update_plan` (maintain a task checklist), \
         `ask_user_question` (interview the user){web_tool}{canvas_tool}.\n\n\
         Guidelines:\n\
         - Prefer the dedicated tools over shell equivalents: use `find_files` not \
           `find`/`ls`, `search_files` not `grep`, `read_file` not `cat`, and \
           `edit_file`/`write_file` not `sed`/redirects.\n\
         - Read before you write. Read the files you're about to touch — fully, not \
           skimmed — and copy the patterns already there (naming, error handling, the \
           libraries the project actually uses). Always `read_file` before editing it; \
           `edit_file` needs `old_string` to match the real content exactly. Never \
           include `read_file`'s line-number and tab prefix in edit arguments.{web_guideline}\n\
         - Think before you code. When a request is ambiguous, name the assumption \
           you're acting on and the trade-off you're making rather than filling the gap \
           with plausible-looking code. For anything multi-step, state the plan and a \
           concrete success criterion first so a wrong approach is caught early.\n\
         - Default to working WITHOUT `update_plan`. Reach for it only on large, \
           multi-phase work (roughly 5+ substantial steps spanning clearly \
           separate pieces) or when the user explicitly asks for a plan/todo list \
           or hands you a numbered list of separate tasks. Don't use it for a \
           single change, a few edits, or questions you can answer directly, and \
           don't split one logical task into busywork steps just to have a list — \
           when unsure, just do the work. When you do use it, keep exactly one item \
           in_progress and mark items completed the moment they're done.\n\
         - When a product/design/implementation decision is genuinely ambiguous and \
           has multiple reasonable approaches with real trade-offs, call \
           `ask_user_question` to interview the user instead of guessing. Keep \
           options concise and distinct; don't add an 'Other' option (the user can \
           always type their own). Don't ask about trivia you can decide yourself.{canvas_guideline}\n\
         - Keep changes surgical and simple. Write the minimum code that solves the \
           problem in front of you — resist premature abstraction and configuration you \
           don't need yet. Make the smallest diff the task allows: match the existing \
           style, don't reformat, and don't touch code you weren't asked to. If you \
           can't justify a changed line by the task, revert it.\n\
         - Before adding a dependency, check whether the project or the standard library \
           already does the job — a dependency is permanent code you don't control. When \
           you do add one, say why.\n\
         - The user can attach images and PDFs to a message, and you receive their \
           actual visual content — look at them directly and answer from what you \
           see. Never claim you can't view images or that one wasn't provided.\n\
         - Work in small, verifiable steps. Run tests/builds and read the real output \
           rather than assuming success. When fixing a bug, reproduce it first and add a \
           failing test, then fix the root cause — not the symptom. Investigate rather \
           than guess: read the whole error, change one thing at a time, and don't paper \
           over an unexpected null with a null check.\n\
         - Say what you did and why, and be precise about uncertainty — name what you're \
           unsure of and what to verify rather than vaguely claiming it should work.\n\
         - Never end a turn with only a statement of intent. If you say you will \
           create, edit, run, or look at something, emit the tool call that does it \
           in the same turn — don't stop after announcing the plan and wait.\n\
         - Make independent tool calls together when they don't depend on each other."
    )
}

/// The one-shot corrective appended when the model announces an action but
/// doesn't call a tool (see [`looks_like_unfulfilled_intent`]). Sent only on the
/// retry request and never persisted.
const INTENT_NUDGE: &str =
    "You described what you'll do but didn't actually call a tool to do it. \
     If you intended to take an action — open a `canvas`, write or edit a file, run a command — \
     make that tool call now. If you were genuinely finished, reply with your final answer.";

/// Heuristic: does a text-only reply read as "I'm about to do X" rather than a
/// finished answer? Used at most once per turn to nudge the model into emitting
/// the tool call it announced instead of ending the turn on the plan.
/// Deliberately conservative — a false positive only costs one extra model
/// round-trip, since the nudge is capped at one per turn.
fn looks_like_unfulfilled_intent(text: &str) -> bool {
    let t = text.to_lowercase();
    const SIGNALS: &[&str] = &[
        "i'll ",
        "i will ",
        "i'm going to",
        "i am going to",
        "i'm gonna",
        "let me ",
        "now i'll",
        "next, i",
        "i'll go ahead",
    ];
    // "let me know" is a sign-off, not an unperformed action — don't nudge on it.
    SIGNALS.iter().any(|s| t.contains(s)) && !t.contains("let me know")
}

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
        let attachments = config.attachment_root.clone().map(AttachmentStore::new);
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
        let attachments = config.attachment_root.clone().map(AttachmentStore::new);
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
    /// disturbing the transcript or session. Pair with [`set_model`] so the
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

    /// Run one user turn to completion, returning the assistant's final text.
    ///
    /// `on_event` is invoked for streamed tokens and tool activity so callers
    /// can render progress live.
    pub async fn run_turn<F>(
        &mut self,
        user_input: impl Into<String>,
        on_event: F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        self.run_turn_with_attachments(user_input, Vec::new(), on_event)
            .await
    }

    /// Run one user turn that may carry attachments (images/PDFs/videos dropped
    /// into the chat). Attachments become content parts on the user message;
    /// with none, this is identical to [`Agent::run_turn`].
    pub async fn run_turn_with_attachments<F>(
        &mut self,
        user_input: impl Into<String>,
        attachments: Vec<Attachment>,
        mut on_event: F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        self.push(build_user_message(
            user_input.into(),
            &attachments,
            self.attachments.as_ref(),
        )?)?;

        // Tool definitions are fixed for the turn; compute once.
        let tool_defs = self.tools.definitions();
        let window = self.context_window();
        let budget = budget::prompt_budget(window, self.config.response_reserve);

        // A one-shot corrective for the "announce a plan, then stop" failure: if
        // the model returns a text-only reply that reads as intent-to-act, we
        // append this nudge to the *next* request only and let the loop run once
        // more. It's never persisted, so it stays out of both the stored
        // transcript and the visible chat. Capped at one nudge per turn.
        let mut nudge: Option<ChatMessage> = None;
        let mut nudged = false;

        // The stop signal for this turn (a clone, so cancelling it from the host
        // doesn't require the agent lock the turn is holding).
        let cancel = self.cancel.clone();

        // No fixed iteration cap: the loop runs until the model returns a final
        // answer, bounded only by how much fits in the context window.
        loop {
            // Honor a stop requested between model calls (e.g. while tools ran).
            if cancel.is_cancelled() {
                return Ok(String::new());
            }

            // Keep the next request within the window. The raw estimate is
            // calibrated by the latest real usage so the check tracks reality
            // (code under-counts at 4 chars/token). If we'd overflow, compact
            // (prune stale tool output, then summarize old turns) rather than
            // hard-stopping; only error if even a compacted transcript can't fit.
            let mut raw_prompt_tokens = budget::estimate_prompt_tokens(&self.messages, &tool_defs);
            if self.calibrated(raw_prompt_tokens) > budget {
                let fit = self
                    .compact_to_fit(budget, &tool_defs, &mut on_event)
                    .await?;
                raw_prompt_tokens = budget::estimate_prompt_tokens(&self.messages, &tool_defs);
                if !fit || self.calibrated(raw_prompt_tokens) > budget {
                    return Err(AgentError::ContextWindowExceeded {
                        used: self.calibrated(raw_prompt_tokens),
                        window,
                    });
                }
            }
            let prompt_tokens = self.calibrated(raw_prompt_tokens);

            // Reflect this call's prompt cost the moment it's sent (the transcript
            // is `prompt_tokens` of context), so a live meter accounts for it now
            // rather than jumping when the reply finishes. The reply then streams
            // on top, and the post-call event below snaps to the exact figure.
            on_event(&AgentEvent::Usage {
                tokens_used: self.tokens_used + prompt_tokens,
                context_tokens: prompt_tokens,
            });

            let mut outbound = self.outbound_messages();
            if let Some(n) = &nudge {
                outbound.push(n.clone());
            }
            let request = ChatRequest::new(&self.config.model, outbound)
                .with_tools(tool_defs.clone())
                .streaming(true);

            let assembled = self
                .client
                .stream_chat(&request, &cancel, |event| match event {
                    StreamEvent::Token(t) => on_event(&AgentEvent::Token(t.clone())),
                    StreamEvent::ToolCallStart { name } => {
                        on_event(&AgentEvent::ToolPending { name: name.clone() })
                    }
                    StreamEvent::ToolCallDelta { name, arguments } => {
                        on_event(&AgentEvent::ToolDelta {
                            name: name.clone(),
                            delta: arguments.clone(),
                        })
                    }
                    StreamEvent::Done { .. } => {}
                })
                .await?;

            // A stop mid-stream returns whatever assembled so far. Persist only
            // the partial prose (a half-formed tool call would be malformed and
            // must not be replayed), keep it out of the token tally, and end the
            // turn cleanly so the UI settles to a normal reply rather than error.
            if cancel.is_cancelled() {
                if !assembled.content.is_empty() {
                    self.push(ChatMessage::assistant(assembled.content.clone()))?;
                }
                return Ok(assembled.content);
            }

            // Recalibrate the estimate against the real prompt size the endpoint
            // billed, so the next budget check tracks reality.
            if let Some(usage) = &assembled.usage {
                if usage.prompt_tokens > 0 && raw_prompt_tokens > 0 {
                    self.token_ratio = usage.prompt_tokens as f64 / raw_prompt_tokens as f64;
                }
            }

            // Account for this round's prompt + generated tokens, preferring the
            // endpoint's real counts and falling back to the calibrated estimate.
            self.tokens_used += match &assembled.usage {
                Some(u) if u.prompt_tokens + u.completion_tokens > 0 => {
                    (u.prompt_tokens + u.completion_tokens) as usize
                }
                _ => {
                    prompt_tokens
                        + budget::estimate_completion_tokens(
                            &assembled.content,
                            &assembled.tool_calls,
                        )
                }
            };

            self.push(ChatMessage::assistant_with_tools(
                assembled.content.clone(),
                assembled.tool_calls.clone(),
            ))?;

            // The exact cumulative + context now that the reply is in the
            // transcript; the UI snaps its live estimate to this.
            on_event(&AgentEvent::Usage {
                tokens_used: self.tokens_used,
                context_tokens: self.context_tokens(),
            });

            if assembled.tool_calls.is_empty() {
                // The model replied with prose and no tool call. If it reads as
                // an announced-but-unperformed action, nudge it once to actually
                // emit the call; otherwise this is its final answer.
                if !nudged && looks_like_unfulfilled_intent(&assembled.content) {
                    nudged = true;
                    nudge = Some(ChatMessage::user(INTENT_NUDGE.to_string()));
                    continue;
                }
                return Ok(assembled.content);
            }

            // A tool call landed; the corrective (if any) served its purpose.
            nudge = None;

            for call in &assembled.tool_calls {
                let result = self.run_tool(call, &mut on_event).await;
                self.push(ChatMessage::tool_result(call.id.clone(), result))?;
            }
        }
    }

    /// Free context so the next request fits `budget`, in two stages (see
    /// [`compact`]): prune stale tool output, then summarize the oldest turns.
    /// Emits an [`AgentEvent::Compacted`] for each stage that does work and
    /// returns whether the transcript now fits. Mutates only the in-memory
    /// transcript — the history store keeps the full record.
    async fn compact_to_fit<F>(
        &mut self,
        budget: usize,
        tool_defs: &[serde_json::Value],
        on_event: &mut F,
    ) -> Result<bool, AgentError>
    where
        F: FnMut(&AgentEvent),
    {
        // Keep the latest few tool outputs and the last few turns verbatim.
        const KEEP_RECENT_TOOLS: usize = 2;
        const KEEP_RECENT_TURNS: usize = 3;

        // Stage 1: prune stale tool output — cheap, no model call.
        let freed = compact::prune_tool_results(&mut self.messages, KEEP_RECENT_TOOLS);
        if freed > 0 {
            on_event(&AgentEvent::Compacted {
                detail: format!("pruned ~{freed} chars of older tool output"),
            });
        }
        if self.fits_budget(budget, tool_defs) {
            return Ok(true);
        }

        // Stage 2: summarize the oldest turns into a single message. The cut is
        // on a user-turn boundary, so no tool result is orphaned from its call.
        let Some(cut) = compact::summary_cut_index(&self.messages, KEEP_RECENT_TURNS) else {
            return Ok(self.fits_budget(budget, tool_defs));
        };
        let start = usize::from(self.messages.first().is_some_and(|m| m.role == "system"));
        let rendered = compact::render_for_summary(&self.messages[start..cut]);
        let summary = self.complete(compact::SUMMARY_PROMPT, &rendered).await?;
        let note = ChatMessage::user(format!("{}\n{}", compact::SUMMARY_MARKER, summary));
        self.messages.splice(start..cut, std::iter::once(note));
        on_event(&AgentEvent::Compacted {
            detail: "summarized earlier conversation".to_string(),
        });
        Ok(self.fits_budget(budget, tool_defs))
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
    use harness_store::SessionMeta;

    #[test]
    fn system_prompt_forbids_ending_on_intent_in_every_variant() {
        // The guardrail against the "announce the plan, then stop" failure mode
        // must be present regardless of which optional tools are advertised.
        let needle = "Never end a turn with only a statement of intent";
        for web_search in [false, true] {
            for canvas in [false, true] {
                let prompt = system_prompt_with(web_search, canvas);
                assert!(
                    prompt.contains(needle),
                    "guardrail missing for web_search={web_search}, canvas={canvas}"
                );
            }
        }
        // ...and via the public convenience wrapper the host uses by default.
        assert!(default_system_prompt(false).contains(needle));
        // The always-available planning tool is advertised in every variant.
        assert!(default_system_prompt(false).contains("update_plan"));
        assert!(AgentConfig::default()
            .system_prompt
            .unwrap()
            .contains(needle));
    }

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
        let session = store
            .create_session(&SessionMeta {
                workspace: "/tmp/proj".into(),
                model: "claude-opus-4-8".into(),
                ..Default::default()
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
        assert_eq!(agent.messages()[1].content_text().as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn run_turn_stops_when_context_window_is_exhausted() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = store
            .create_session(&SessionMeta {
                workspace: "/tmp/proj".into(),
                model: "claude-opus-4-8".into(),
                ..Default::default()
            })
            .unwrap();
        // A 1-token window can't fit any real prompt, so the budget check trips
        // on the first iteration — before any network call is attempted.
        let config = AgentConfig {
            model: "claude-opus-4-8".into(),
            system_prompt: None,
            context_window: Some(1),
            response_reserve: 0,
            ..AgentConfig::default()
        };
        let client = OxenClient::new("http://127.0.0.1:1/api/ai", "key", "claude-opus-4-8");
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        let err = agent
            .run_turn("please do something that needs more than one token", |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::ContextWindowExceeded { .. }));
    }

    #[tokio::test]
    async fn run_turn_stops_immediately_when_cancelled() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = store
            .create_session(&SessionMeta {
                workspace: "/tmp/proj".into(),
                model: "claude-opus-4-8".into(),
                ..Default::default()
            })
            .unwrap();
        // Point at an unroutable address: if cancellation didn't short-circuit
        // before the network call, the turn would hang/err on connect instead of
        // returning cleanly.
        let client = OxenClient::new("http://127.0.0.1:1/api/ai", "key", "claude-opus-4-8");
        let config = AgentConfig {
            system_prompt: None,
            ..AgentConfig::default()
        };
        let mut agent = Agent::new(client, ToolRegistry::new(), store, session, config).unwrap();

        // Pre-cancel the turn's stop signal; the loop bails before any request.
        let token = CancellationToken::new();
        token.cancel();
        agent.set_cancel_token(token);

        let out = agent.run_turn("do a lot of work", |_| {}).await.unwrap();
        assert_eq!(out, "");
        // Only the user message was persisted — no assistant reply for a turn that
        // never reached the model.
        assert_eq!(agent.messages().last().unwrap().role, "user");
    }

    #[tokio::test]
    async fn run_turn_compacts_instead_of_erroring_when_over_budget() {
        // A streaming endpoint that returns a short final answer.
        let mut server = mockito::Server::new_async().await;
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"all done\"},\"finish_reason\":\"stop\"}]}\n\n\
                   data: [DONE]\n\n";
        server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        // Seed a transcript with three big tool results — over a small window.
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = store
            .create_session(&SessionMeta {
                workspace: "/tmp/proj".into(),
                model: "qwen3-8b".into(),
                ..Default::default()
            })
            .unwrap();
        let big = "x".repeat(8000); // ~2000 tokens each
        for (i, _) in (0..3).enumerate() {
            store
                .append_message(&session, &ChatMessage::user(format!("q{i}")))
                .unwrap();
            store
                .append_message(
                    &session,
                    &ChatMessage::tool_result(format!("t{i}"), big.clone()),
                )
                .unwrap();
        }

        let client = OxenClient::new(server.url(), "key", "qwen3-8b");
        let config = AgentConfig {
            model: "qwen3-8b".into(),
            system_prompt: None,
            // Fits two of the three big tool results, not all three.
            context_window: Some(4500),
            response_reserve: 0,
            ..AgentConfig::default()
        };
        let mut agent =
            Agent::resume_from_store(client, ToolRegistry::new(), store, session, config).unwrap();

        let mut compacted = false;
        let out = agent
            .run_turn("continue", |e| {
                if matches!(e, AgentEvent::Compacted { .. }) {
                    compacted = true;
                }
            })
            .await
            .expect("turn should compact and succeed, not error");

        assert_eq!(out, "all done");
        assert!(compacted, "a Compacted event should have fired");
        // The oldest tool result was stubbed; the newest stays verbatim.
        let tool_texts: Vec<String> = agent
            .messages()
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.content_text())
            .collect();
        assert!(tool_texts.first().unwrap().contains("elided"));
        assert!(tool_texts.last().unwrap().contains(&big));
    }
}
