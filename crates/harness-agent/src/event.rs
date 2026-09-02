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
    /// A tool is about to run, with its name and JSON arguments. `call_id` is
    /// the model's id for the call; calls in one reply may run concurrently,
    /// so a UI pairs [`AgentEvent::ToolEnd`] with its start by id, not name.
    ToolStart {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// A chunk of a running tool's output (a shell command's stdout/stderr as
    /// it streams), so a UI can show the work as it happens. The full result
    /// still arrives on [`AgentEvent::ToolEnd`].
    ToolProgress {
        call_id: String,
        name: String,
        chunk: String,
    },
    /// A tool finished, with its (possibly truncated for display) result.
    ToolEnd {
        call_id: String,
        name: String,
        result: String,
    },
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
    /// The agent is re-calling the model with a one-shot corrective (a reply
    /// that announced work without doing it, an uncharted trail, a repeated
    /// call, a matched stream rule, a spent round budget). The corrective
    /// itself is never shown or persisted; this says why another round is
    /// starting.
    Nudged { reason: String },
    /// A background shell task (started with `is_background`, or a foreground
    /// command that outlived its patience) finished, and its final output was
    /// just delivered to the model as a message. Surfaced so a UI can print a
    /// one-line notice where the delivery happened — the message itself
    /// never renders.
    BackgroundTaskDone {
        task_id: u64,
        command: String,
        /// The exit code, or `None` when the task died on a signal.
        exit_code: Option<i32>,
    },
    /// A result that finished on its own (a `wait: false` fleet) was just
    /// delivered to the model as a message; `title` is the one-line notice.
    AsideDelivered { kind: String, title: String },
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
        /// Set when the retries on the current model are spent and the call is
        /// moving to a configured fallback instead of failing the turn. The
        /// session model is unchanged — only this call switches.
        switching_to: Option<String>,
    },
}

/// The one-line notice for a background task whose output was just delivered
/// to the model — shared by every front end so the wording can't drift.
pub fn background_task_notice(task_id: u64, command: &str, exit_code: Option<i32>) -> String {
    let exit = match exit_code {
        Some(0) => "finished".to_string(),
        Some(code) => format!("exited with code {code}"),
        None => "ended on a signal".to_string(),
    };
    format!("background task {task_id} {exit} — output delivered to the model: {command}")
}
