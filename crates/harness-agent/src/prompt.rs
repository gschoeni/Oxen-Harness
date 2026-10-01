//! The agent's system prompt and the "unfulfilled intent" nudge.
//!
//! The prompt is assembled from a fixed core plus optional sections that are
//! only included when the host actually registered the corresponding tool
//! (`web_search`, `canvas`, `open_file`), so the model is never told about a
//! tool the registry would reject. Hosts derive that set straight from their
//! finished registry with [`OptionalTools::from_registry`].

/// Which host-optional tools survived registration — drives the prompt
/// sections that advertise them. Derive it from the finished registry
/// ([`OptionalTools::from_registry`]) so the prompt can't drift from what the
/// registry will actually accept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OptionalTools {
    pub web_search: bool,
    pub canvas: bool,
    pub open_file: bool,
    /// The agent tools (`spawn_agents`, `map_agents`, `ask_model`, …) — the
    /// fleet is registered after the prompt is built, so hosts set this from
    /// the same preference that decides whether it registers.
    pub agents: bool,
    /// The media tools (`generate_image`, `generate_video`, `media_models`,
    /// `media_status`) — registered per session by hosts with a spend sink.
    pub media: bool,
}

impl OptionalTools {
    /// Read the optional-tool set off a finished registry (call after user
    /// preferences are applied, so a disabled tool is not advertised).
    pub fn from_registry(tools: &harness_tools::ToolRegistry) -> Self {
        Self {
            web_search: tools.get(harness_tools::WEB_SEARCH_TOOL).is_some(),
            canvas: tools.get(harness_tools::CANVAS_TOOL).is_some(),
            open_file: tools.get(harness_tools::OPEN_FILE_TOOL).is_some(),
            agents: tools.get(crate::fleet_tool::FLEET_TOOL).is_some(),
            media: tools.get(harness_media::GENERATE_IMAGE_TOOL).is_some(),
        }
    }

    /// Whether the agent tools will be registered. Both hosts register the
    /// fleet *after* the prompt is built (the spawner snapshots the registry
    /// without itself), so `from_registry` can't see it — they pass the
    /// preference that decides the registration here.
    pub fn with_agents(mut self, agents: bool) -> Self {
        self.agents = agents;
        self
    }
}

/// Build the default system prompt. `web_search` controls whether the
/// `web_search` tool is advertised — pass whether it's actually registered, so
/// the model is never offered (and never tries to call) a tool that the
/// registry would reject as unknown.
pub fn default_system_prompt(web_search: bool) -> String {
    system_prompt_with(OptionalTools {
        web_search,
        ..OptionalTools::default()
    })
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
pub fn system_prompt_with_env(tools: OptionalTools, workspace: &std::path::Path) -> String {
    format!(
        "{}{}",
        system_prompt_with(tools),
        environment_section(workspace)
    )
}

/// The media tools' entry in the prompt's tool list.
const MEDIA_TOOL_LIST_ENTRY: &str =
    ", `generate_image` / `generate_video` (create images and video clips \
with Oxen.ai models, saved into the project), `media_models` (browse those models and their \
parameters), `media_status` (in-flight generations; cancel)";

/// How to be a good art director: cheap drafts first, the model's own idiom,
/// references through `refs`, and ask the user for the media you need.
const MEDIA_GUIDELINE: &str = "\n- Media generation (`generate_image`, `generate_video`) costs \
money and is saved into the project's media folder. Work like an art director: brainstorm and \
tighten the prompt in chat first, then draft cheaply (the default image model, one or two \
outputs, low video resolution and short duration), show the user, and render the chosen \
direction with a stronger model. Write prompts in the model's idiom — subject, style, \
composition, lighting, camera and motion for video, a timed shot list for multi-beat clips — \
and read `media_models` (with `id`) before using a model's own parameters in `extra`. \
References: when a reference image, video, or audio track would make the result better or \
is needed (a character to keep consistent, a first frame, a style, a soundtrack), ask the \
user to drop it into the chat and say what it's for; attached media shows up labeled \
`[Image #N]`, `[Video #N]`, `[Audio #N]` — pass those labels (or an earlier generation's \
path) in `refs` and mention them in the prompt; the tool places them in the model's matching \
fields. Tell the user the estimated cost before a large batch, and never generate more than \
they asked for. Videos render in the background: keep working and let the result come to \
you rather than polling.";

/// Appended to a lane's system prompt when it may still spawn lanes of its
/// own: delegate reading, never sub-call everything.
pub const LANE_APPENDIX: &str = "\n\n## You are a subagent\n\
You were spawned by another agent for one task and it will read only your final reply, \
so make that reply the deliverable: specific, complete, and as short as the task allows \
(paths, names, numbers, verdicts — not a narrative of what you did). You may spawn agents of \
your own for large or parallel reading, but batch: give each a substantial, self-contained \
chunk rather than one agent per item, and do the small work yourself. You work within a \
token allowance; when a reminder says it is nearly spent, finish the step you are on and \
report — a report you write beats one you are cut off from. You cannot ask the user \
anything; if you are blocked (a command needs approval, something is ambiguous), say \
exactly what and why in your reply.";

/// Appended to a leaf's system prompt: it answers from what it is given.
pub const LEAF_APPENDIX: &str = "\n\n## You are a subagent\n\
You were spawned by another agent for one task and it will read only your final reply, \
so make that reply the deliverable: specific, complete, and as short as the task allows \
(paths, names, numbers, verdicts — not a narrative of what you did). Answer from what you \
are given and what you can read yourself; there are no further agents to delegate to. You \
work within a token allowance; when a reminder says it is nearly spent, finish the step you \
are on and report — a report you write beats one you are cut off from. You cannot ask the \
user anything; if you are blocked (a command needs approval, something is ambiguous), say \
exactly what and why in your reply.";

/// The prompt appendix for a subagent at `depth` under a `max_depth` cap.
pub fn subagent_appendix(depth: u8, max_depth: u8) -> &'static str {
    if depth < max_depth {
        LANE_APPENDIX
    } else {
        LEAF_APPENDIX
    }
}

const AGENTS_TOOL_LIST_ENTRY: &str = ", `spawn_agents` / `map_agents` / `ask_model` (delegate to \
    parallel subagents)";

/// When to delegate, and to which tool — the effort-scaling rule the
/// multi-agent studies converged on, in one guideline. Only in the prompt
/// when the agent tools are registered, and stripped from a leaf's.
pub const DELEGATION_GUIDELINE: &str = "\n- Delegate reading, not deciding. Your context is \
    for decisions; when a task means reading or searching more than a handful of files, or \
    trying several approaches, hand that out and keep the results: `spawn_agents` for 2-6 \
    distinct, self-contained tasks (a whole subsystem each — not one file each), `map_agents` \
    for the same task over a list of items (every item is guaranteed a result), `ask_model` \
    for cheap questions over parked content with no tools needed. Set `profile: \"research\"` \
    on lanes that only read, search or fetch: they cost roughly a third of a full lane. \
    Don't delegate what you can \
    do in a few tool calls yourself, and don't spawn an agent to spawn agents. An agent sees \
    only its prompt: say what to read, what to decide, and the shape of answer you want back \
    (an `output_schema` when you'll act on it mechanically). Parked content goes in `inputs` \
    as handles, never pasted. Use `wait: false` when you have other work meanwhile — results \
    arrive on their own, never poll — and `send_to_agent` to continue an agent instead of \
    briefing a new one.";

/// Remove the delegation sections from a finished system prompt, for a leaf
/// lane that has no agent tools to delegate with.
pub(crate) fn strip_delegation_sections(prompt: &str) -> String {
    prompt
        .replace(AGENTS_TOOL_LIST_ENTRY, "")
        .replace(DELEGATION_GUIDELINE, "")
}

/// The system prompt, advertising the host-optional tools (`web_search`,
/// `canvas`, `open_file`) only when the host actually registered them.
pub fn system_prompt_with(tools: OptionalTools) -> String {
    let web_tool = if tools.web_search {
        ", `web_search` (Brave web search)"
    } else {
        ""
    };
    let canvas_tool = if tools.canvas {
        ", `canvas` (show a document in a side panel)"
    } else {
        ""
    };
    let open_file_tool = if tools.open_file {
        ", `open_file` (show a project file in the user's file viewer)"
    } else {
        ""
    };
    let web_guideline = if tools.web_search {
        "\n- Use `web_search` when something may be newer than your training or \
         isn't in the workspace: library/API docs, current events, or an \
         unfamiliar error."
    } else {
        ""
    };
    let canvas_guideline = if tools.canvas {
        "\n- When you produce a substantial, self-contained deliverable the user \
         will read, iterate on, or keep — a report/article (markdown), a rendered \
         web page or interactive demo (html), a sizeable code file (code), or a \
         vector graphic (svg) — show it with `canvas` \
         instead of a long fenced block in chat. Reuse the same `id` to revise an \
         open document. For a page or document you also wrote to a project file, \
         pass its `path` instead of `content`: the canvas renders the file and \
         follows your later edits to it. Don't use `canvas` for short answers or \
         quick snippets; opening a panel for those is disruptive."
    } else {
        ""
    };
    let agents_tool = if tools.agents {
        AGENTS_TOOL_LIST_ENTRY
    } else {
        ""
    };
    let delegation_guideline = if tools.agents {
        DELEGATION_GUIDELINE
    } else {
        ""
    };
    let media_tool = if tools.media {
        MEDIA_TOOL_LIST_ENTRY
    } else {
        ""
    };
    let media_guideline = if tools.media { MEDIA_GUIDELINE } else { "" };
    let open_file_guideline = if tools.open_file {
        "\n- After creating or substantially rewriting a project file the user \
         will want to look at — or when walking them through one — call \
         `open_file` to put it in their file viewer beside the chat instead of \
         pasting its contents. Open the one or two files that matter, not every \
         file you touch. `open_file` shows source: an HTML page (or a document) \
         the user should see rendered goes to `canvas` with the file's `path`."
    } else {
        ""
    };
    format!(
        "You are oxen-harness, an open source coding agent working in the user's \
         project directory. Available tools: `find_files` (locate files by glob), \
         `search_files` (regex content search), `read_file` (line-numbered, supports \
         offset/limit), `write_file`, `edit_file` (exact-string patch), `run_shell`, \
         `update_plan` (maintain a task checklist), \
         `ask_user_question` (interview the user){web_tool}{canvas_tool}{open_file_tool}{agents_tool}{media_tool}.\n\n\
         Guidelines:\n\
         - Prefer the dedicated tools over shell equivalents: use `find_files` not \
           `find`/`ls`, `search_files` not `grep`, `read_file` not `cat`, and \
           `edit_file`/`write_file` not `sed`/redirects.\n\
         - Read before you write. Read the files you're about to touch — fully, not \
           skimmed — and copy the patterns already there (naming, error handling, the \
           libraries the project actually uses). `edit_file` and overwriting with \
           `write_file` refuse a file you haven't read this session, and refuse again \
           if it changed after you read it — when that happens, re-read and re-apply \
           your change on top of what's there now rather than forcing the old one. \
           Never include `read_file`'s line-number and tab prefix in edit \
           arguments.{web_guideline}\n\
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
           in_progress and mark items completed the moment they're done. If a step \
           fails or is blocked (a tool error, missing auth, an impossible subtask), \
           never abandon the checklist silently: update the plan to reflect it — \
           annotate or drop the blocked step — continue with the steps that don't \
           depend on it, and tell the user what's blocked and why.{delegation_guideline}\n\
         - When a product/design/implementation decision is genuinely ambiguous and \
           has multiple reasonable approaches with real trade-offs, call \
           `ask_user_question` to interview the user instead of guessing. Keep \
           options concise and distinct; don't add an 'Other' option (the user can \
           always type their own). Don't ask about trivia you can decide yourself.{canvas_guideline}{open_file_guideline}{media_guideline}\n\
         - Be careful with destructive commands. Prefer reversible, narrowly-scoped \
           operations, and never chain a destructive action (deleting files, killing \
           processes, force-pushing, rewriting git history) with unrelated commands in \
           one `run_shell` call — run it alone, right after saying why it's needed, so \
           any approval prompt covers exactly that action. If the user declines a \
           command, do not retry it or pursue the same effect another way; adjust your \
           approach or ask what they'd prefer.\n\
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
pub(crate) const INTENT_NUDGE: &str =
    "<system-reminder>An automatic check, not a message from the user: your reply described \
     what you'll do but didn't call a tool to do it. If you intended to take an action — open a \
     `canvas`, write or edit a file, run a command — make that tool call now. If you were \
     finished, or are waiting on the user or on background work, reply with only your final \
     answer and don't mention this check.</system-reminder>";

/// The one-shot corrective appended when the model ends its turn while a plan it
/// updated *this turn* still has unfinished items — the "one subtask failed, so
/// the whole checklist silently stalls" failure mode (see `Agent::drive_turn`).
/// Sent only on the retry request and never persisted.
pub(crate) const PLAN_STALL_NUDGE: &str =
    "Your plan still has unfinished items. If you can keep working, continue with \
     the next step now. If a step failed or is blocked, call `update_plan` to make \
     the checklist reflect reality — keep what's done, drop or annotate the blocked \
     step, continue any steps that don't depend on it — and then give your final \
     answer explaining what's blocked and what you completed instead. Do not leave \
     the checklist stale.";

/// The one-shot corrective appended when the same tool call has repeated with
/// identical arguments *and* an identical result several times in a row (see
/// [`crate::loopguard`]). Each repeat re-bills the whole context for zero new
/// information. Sent only on the next request and never persisted.
/// Sent when a turn reaches its round budget's wrap-up line.
pub(crate) const WRAP_UP_NUDGE: &str = "<system-reminder>You are near this task's round budget. \
Wrap up now: finish the current step, then give your final report with what you did, \
what you verified, and what is left. Do not start new work.</system-reminder>";

/// The follow-up a lane gets when it is run again after failing on the
/// provider (see `FleetSpawner::retry_transient_failures`): pick up, don't
/// start over.
pub(crate) fn lane_retry_prompt(error: &str) -> String {
    format!(
        "Your previous attempt stopped with an error before you could finish: {error}. What \
         you already read and found is above. Continue from where you left off and finish \
         the task; do not start over."
    )
}

/// The one warning a lane gets as its allowance runs low (see
/// `ALLOWANCE_WARN_PERCENT_LEFT`): wrap up on purpose, not by being stopped.
pub(crate) fn allowance_nudge(left: u64, cap: u64) -> String {
    format!(
        "<system-reminder>You have about {left} tokens of your {cap}-token allowance left. \
         Wrap up now: finish the current step, then give your final report with what you \
         found, what you verified, and what is left. Do not start new reading.</system-reminder>"
    )
}

/// The reminder that rides with the one call a budget-stopped turn still
/// gets (see `Agent::final_report`): no tools, one reply, the deliverable.
pub(crate) fn final_report_nudge(reason: &str) -> String {
    format!(
        "<system-reminder>Stopped: {reason}. Tools are no longer available and this is your \
         last reply. Give your final report now — what you found, what you verified, and what \
         is left — in the shape you were asked for, with the specifics (paths, names, numbers, \
         verdicts). Do not describe what you would do next.</system-reminder>"
    )
}

pub(crate) const LOOP_NUDGE: &str =
    "You have made the same tool call with identical arguments several times in a row, \
     and it returned the identical result each time — repeating it again will not produce \
     new information. Change your approach: use different arguments, a different tool, or \
     explain to the user what you're blocked on.";

/// The most of an interjection that reaches the transcript — a paste-bomb
/// mid-turn must not blow the context budget the turn was working within.
const INTERJECTION_MAX_CHARS: usize = 25_000;

/// Bound a message the user sent mid-turn. It enters the transcript as an
/// ordinary user message — no framing wrapper: the store is verbatim history
/// (renderers and fine-tuning exports read it back), and the message's
/// position between tool rounds already tells the model it arrived
/// mid-work. Deliberately no "defer this" instruction either — the model
/// weighs it against the work in flight (an urgent "stop, wrong file!"
/// should win; an "also bump the version" can wait for the natural next
/// step).
pub(crate) fn clip_interjection(text: &str) -> String {
    harness_core::text::truncate_with_marker(
        text,
        INTERJECTION_MAX_CHARS,
        "\n… [interjection truncated]",
    )
}

/// Heuristic: does a text-only reply read as "I'm about to do X" rather than a
/// finished answer? Used at most once per turn to nudge the model into emitting
/// the tool call it announced instead of ending the turn on the plan.
/// Deliberately conservative — a false positive only costs one extra model
/// round-trip, since the nudge is capped at one per turn.
pub(crate) fn looks_like_unfulfilled_intent(text: &str) -> bool {
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
    SIGNALS.iter().any(|s| t.contains(s)) && !hands_turn_to_user(&t)
}

/// Whether a (lowercased) reply ends by waiting — on the user (a question, or
/// "tell me X and I'll do Y") or on background work it already started ("I'll
/// show it the moment it lands"). Its "I'll" is conditional on that, so
/// ending the turn there is correct, not a stall.
fn hands_turn_to_user(t: &str) -> bool {
    const HAND_OFFS: &[&str] = &[
        "let me know",
        "tell me",
        "once you",
        "if you'd like",
        "if you want",
        "the moment it",
        "as soon as it",
        "once it's",
        "once it finishes",
        "once it lands",
        "when it's done",
        "when it's ready",
        "when it finishes",
        "when it lands",
    ];
    let last_paragraph = t.trim_end().rsplit("\n\n").next().unwrap_or(t);
    last_paragraph.contains('?') || HAND_OFFS.iter().any(|h| t.contains(h))
}

/// The most of a finished background task's output delivered inline; the
/// rest stays reachable through `task_output`'s log reference.
const BACKGROUND_DELIVERY_CHARS: usize = 12_000;

/// Frame a finished background task's output as the message that delivers it
/// to the model — clearly marked as automatic, so the model neither thanks
/// the user for it nor keeps polling.
pub fn background_task_delivery(
    task_id: u64,
    command: &str,
    exit_code: Option<i32>,
    output: &str,
) -> String {
    let exit = match exit_code {
        Some(code) => code.to_string(),
        None => "signal".to_string(),
    };
    let body = harness_core::text::truncate_with_marker(
        output,
        BACKGROUND_DELIVERY_CHARS,
        "\n… [delivery truncated — the rest is in the task log]",
    );
    format!(
        "<background-task-result task_id=\"{task_id}\" exit=\"{exit}\">\n\
         This is the automatic delivery of a background task's final output — not a \
         message from the user. Act on it and continue your work.\n\
         command: {command}\n{body}\n</background-task-result>"
    )
}

/// Pushed into the conversation when the user turns plan mode on, so the model
/// reads the constraint as part of the thread rather than as a system rule it
/// might weigh against the request. The sections are fixed on purpose: a plan
/// the user has to finish deciding is not a plan, so there is nowhere to park
/// unresolved options.
pub const PLAN_MODE_ENTER: &str = "\
<plan-mode>
Plan mode is on. The working tree is read-only: file writes and edits, background-task kills, any shell command that is not provably read-only, and any other tool that is not read-only (custom tools, dev servers) will all be refused until the user leaves plan mode. Do not test the limits, and do not ask to have them lifted.

Do this instead:

1. Explore. Read files, search the codebase, run read-only commands until you understand the real code — not the code you assume is there.
2. Ask only when it matters. Use `ask_user_question` for a decision where the options lead to materially different work; never to confirm something you could have read, and never to hand a choice back that you are equipped to make.
3. Write the plan to `.oxen-harness/plans/<slug>.md` — a short kebab-case slug naming the work. That directory is the only path you can write. Use exactly these sections, in this order:

   ## Context — what is true today, in this repository, that the plan must fit.
   ## Approach — the decided plan, in order. Say what changes and why.
   ## Critical files — at most five paths, each with the anchor to change (function, type, section) and one line on the edit.
   ## Verification — the commands and observations that will prove it worked.
   ## Assumptions & contingencies — what you assumed, and what you will do instead if an assumption turns out to be wrong.

   No other sections. In particular: no Non-Goals, no Alternatives, no Risks. Decision-complete beats brief — a plan that leaves a choice open has failed. Fold anything you would have put under Alternatives or Risks into Approach (as the decision you made) or Assumptions & contingencies (as the fallback).

4. Stop and tell the user the plan is ready, in one or two sentences, naming the file you wrote. Do not start the work.
</plan-mode>";

/// Pushed when the user leaves plan mode *without* approving — the tree is
/// writable again, but that is not permission to start.
pub const PLAN_MODE_EXIT: &str = "\
<plan-mode>
Plan mode is off; the tree is writable again. This is not an approval of any plan you wrote: do not start work until the user asks for it. If you were mid-exploration, summarize what you found and wait.
</plan-mode>";

/// The execution brief sent when the user approves a plan: the file becomes
/// the authoritative statement of the work, outranking anything earlier in the
/// thread (including ideas the user talked the model out of while planning).
pub fn plan_approved_prompt(plan_path: &str, plan_text: &str) -> String {
    format!(
        "<approved-plan path=\"{plan_path}\">\n\
         The user approved this plan. Execute it exactly as written. It is \
         authoritative over anything earlier in this conversation — where they \
         disagree, the plan wins. Do not re-plan, do not redesign it, and do not \
         ask for approval again; the approval is this message. If you hit \
         something the plan genuinely did not anticipate, follow its \
         contingencies, and if none apply, stop and say so rather than \
         improvising a different plan. When you are done, report what you \
         verified — the commands you ran and what they showed — not just what \
         you changed.\n\
         \n{plan_text}\n</approved-plan>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentConfig;

    #[test]
    fn system_prompt_forbids_ending_on_intent_in_every_variant() {
        // The guardrail against the "announce the plan, then stop" failure mode
        // must be present regardless of which optional tools are advertised.
        let needle = "Never end a turn with only a statement of intent";
        for web_search in [false, true] {
            for canvas in [false, true] {
                for open_file in [false, true] {
                    let tools = OptionalTools {
                        web_search,
                        canvas,
                        open_file,
                        ..OptionalTools::default()
                    };
                    let prompt = system_prompt_with(tools);
                    assert!(prompt.contains(needle), "guardrail missing for {tools:?}");
                }
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
    fn optional_tool_sections_appear_only_when_enabled() {
        let bare = system_prompt_with(OptionalTools::default());
        assert!(!bare.contains("web_search"));
        assert!(!bare.contains("`canvas`"));
        assert!(!bare.contains("`open_file`"));

        let full = system_prompt_with(OptionalTools {
            web_search: true,
            canvas: true,
            open_file: true,
            agents: false,
            media: true,
        });
        assert!(full.contains("`web_search` (Brave web search)"));
        assert!(full.contains("`generate_image` / `generate_video`"));
        assert!(full.contains("Work like an art director"));
        assert!(!bare.contains("generate_image"));
        assert!(full.contains("`canvas` (show a document in a side panel)"));
        assert!(full.contains("`open_file` (show a project file in the user's file viewer)"));
        assert!(full.contains("`open_file` to put it in their file viewer"));
    }

    #[test]
    fn the_delegation_guideline_rides_with_the_agent_tools_and_leaves_lose_it() {
        let with = system_prompt_with(OptionalTools {
            agents: true,
            ..OptionalTools::default()
        });
        assert!(with.contains("`spawn_agents` / `map_agents` / `ask_model`"));
        assert!(with.contains("Delegate reading, not deciding."));
        assert!(with.contains("profile: \"research\""));
        assert!(with.contains("roughly a third of a full lane"));
        let without = system_prompt_with(OptionalTools::default());
        assert!(!without.contains("spawn_agents"));
        let leaf = strip_delegation_sections(&with);
        assert!(!leaf.contains("spawn_agents"), "{leaf}");
        assert!(!leaf.contains("Delegate reading"));
        assert_eq!(leaf, without);
    }

    #[test]
    fn optional_tools_derive_from_the_registry() {
        // The prompt must reflect what the finished registry actually accepts.
        let registry = harness_tools::ToolRegistry::new();
        assert_eq!(
            OptionalTools::from_registry(&registry),
            OptionalTools::default()
        );
    }

    #[test]
    fn environment_section_names_the_working_directory() {
        let section = environment_section(std::path::Path::new("/tmp/project"));
        assert!(section.contains("/tmp/project"));
        assert!(
            system_prompt_with_env(OptionalTools::default(), std::path::Path::new("/w"))
                .contains("/w")
        );
    }

    #[test]
    fn intent_heuristic_flags_announcements_but_not_sign_offs() {
        assert!(looks_like_unfulfilled_intent("I'll create the file now."));
        assert!(looks_like_unfulfilled_intent(
            "Let me read the config first"
        ));
        assert!(looks_like_unfulfilled_intent("Now I'll run the tests"));
        // Finished answers and sign-offs must not trip the nudge.
        assert!(!looks_like_unfulfilled_intent("Done — the bug is fixed."));
        assert!(!looks_like_unfulfilled_intent(
            "Let me know if you need anything else."
        ));
        // A question back to the user: the "I'll" waits on their answer.
        assert!(!looks_like_unfulfilled_intent(
            "Which model do you want?\n\nWhat are you trying to make — text-to-video or \
             animate an image? Tell me the shot + length, and I'll pick the model and generate it."
        ));
        assert!(!looks_like_unfulfilled_intent(
            "I'll need one detail first: should the output be PNG or JPEG?"
        ));
        // Waiting on background work it already queued.
        assert!(!looks_like_unfulfilled_intent(
            "Queued — 5s, 480p draft.\n\nI'll show it here the moment it lands. If you \
             like the motion, I can re-render it at 720p."
        ));
        // A question earlier in the reply doesn't excuse a trailing announcement.
        assert!(looks_like_unfulfilled_intent(
            "Why does it fail?\n\nI'll read the logs to find out."
        ));
    }
}
