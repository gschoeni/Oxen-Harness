//! The event stream a turn emits as it progresses.
//!
//! Both front ends render the same run from these events in their own idioms —
//! the CLI as spinner/tool lines, the desktop app as chat cards — so agent
//! behavior can't drift between them.

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
        prompt_tokens_used: usize,
        completion_tokens_used: usize,
    },
    /// The transcript was compacted to fit the context window — older history
    /// was pruned and/or summarized so the session can continue instead of
    /// hitting a hard limit. Carries a short human-readable note for the UI.
    Compacted { detail: String },
    /// Stale tool output was compressed before this model call (`mode: "on"`),
    /// or measured without changing the request (`mode: "audit"`). Token
    /// figures use the same calibrated estimate as the usage meter.
    Compression {
        mode: String,
        saved_tokens: usize,
        total_saved_tokens: usize,
        results_compressed: usize,
    },
    /// A gated tool call is waiting on the user's approval decision. An
    /// interactive host hands the screen to its approval prompt on this event
    /// (the way `ask_user_question` hands off to the picker); the decision
    /// itself flows through the host's injected `CommandApprover`, not the
    /// event stream.
    ApprovalPending { name: String, command: String },
    /// The approval prompt resolved; `decision` is a short human-readable
    /// label ("approved", "approved for this session", "denied", …) for the
    /// host to print, and the matching [`AgentEvent::ToolStart`]/[`ToolEnd`]
    /// (or a refusal result) follows.
    ///
    /// [`ToolEnd`]: AgentEvent::ToolEnd
    ApprovalResolved {
        name: String,
        command: String,
        decision: String,
    },
    /// A model call hit a transient provider/network error and will be retried
    /// after `delay_ms`. Surfaced so a UI can show that the turn is still alive
    /// (and why it paused) instead of appearing hung — and, if the stream died
    /// mid-reply, why some text may repeat.
    Retrying {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        error: String,
    },
}
