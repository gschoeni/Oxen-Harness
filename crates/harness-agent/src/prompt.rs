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
    /// `gh` is registered by default but the user can disable it (or lack the
    /// CLI); the prompt must not order the model to verify shipping through a
    /// tool the registry would reject.
    pub gh: bool,
    /// `update_trail` — same: registered by default, disableable in Settings.
    pub trail: bool,
}

impl OptionalTools {
    /// Read the optional-tool set off a finished registry (call after user
    /// preferences are applied, so a disabled tool is not advertised).
    pub fn from_registry(tools: &harness_tools::ToolRegistry) -> Self {
        Self {
            web_search: tools.get(harness_tools::WEB_SEARCH_TOOL).is_some(),
            canvas: tools.get(harness_tools::CANVAS_TOOL).is_some(),
            open_file: tools.get(harness_tools::OPEN_FILE_TOOL).is_some(),
            gh: tools.get(harness_tools::GH_TOOL).is_some(),
            trail: tools.get(harness_tools::TRAIL_TOOL).is_some(),
        }
    }

    /// The default registry's optional set: `gh` and `update_trail` are
    /// registered unless the user disables them, the host-injected tools are
    /// not. Used where a prompt is built without a finished registry in hand
    /// ([`crate::AgentConfig::default`], [`default_system_prompt`]).
    pub fn default_registry() -> Self {
        Self {
            gh: true,
            trail: true,
            ..Self::default()
        }
    }
}

/// Build the default system prompt. `web_search` controls whether the
/// `web_search` tool is advertised — pass whether it's actually registered, so
/// the model is never offered (and never tries to call) a tool that the
/// registry would reject as unknown.
pub fn default_system_prompt(web_search: bool) -> String {
    system_prompt_with(OptionalTools {
        web_search,
        ..OptionalTools::default_registry()
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

/// The trail's entry in the prompt's tool list. A named constant so
/// [`strip_trail_sections`] can remove exactly what was added.
const TRAIL_TOOL_LIST_ENTRY: &str = ", `update_trail` (chart the session's journey)";

/// The trail guideline when `gh` is registered: the shipping stages are
/// verifiable in-session, so the model is ordered to verify them.
///
/// The trigger is a *condition*, not a moment: sessions routinely open as a
/// question ("what could we improve about X?") and only later turn into real
/// work — a "chart before you start" instruction has already expired by then,
/// so the mandate is pinned to the first file edit or shell command instead.
/// It also explicitly severs itself from `update_plan`'s "default to not
/// using it" guidance in the next bullet, which otherwise bleeds onto its
/// sibling tool and suppresses charting.
const TRAIL_GUIDELINE_WITH_GH: &str =
    "\n- Chart this session with `update_trail` as soon as it involves real \
     work — at the latest, right before your first file edit or shell command. \
     A session that opens as a question still gets charted the moment you \
     start actually working. Unlike `update_plan` below, the trail is NOT \
     optional for work sessions. Title the thread and chart its macro stages \
     (standard: define, plan, implement, review; code work ends with pushed, \
     pr-reviewed, merged). Advance stages as you pass them, and re-chart \
     the route whenever the user redirects you or you change approach — \
     the trail must always tell the truth about where this thread stands. \
     The shipping stages are verified, never assumed: `git` push to close \
     \"pushed\", `gh` pr_view to confirm review and merge state. The \
     user's board gates closing the thread on these — they are the open \
     loops you exist to close.";

/// The trail guideline without `gh`: only the push is verifiable in-session;
/// review/merge state is explicitly the user's to confirm.
const TRAIL_GUIDELINE_NO_GH: &str =
    "\n- Chart this session with `update_trail` as soon as it involves real \
     work — at the latest, right before your first file edit or shell command. \
     A session that opens as a question still gets charted the moment you \
     start actually working. Unlike `update_plan` below, the trail is NOT \
     optional for work sessions. Title the thread and chart its macro stages \
     (standard: define, plan, implement, review; code work ends with pushed, \
     pr-reviewed, merged). Advance stages as you pass them, and re-chart \
     the route whenever the user redirects you or you change approach — \
     the trail must always tell the truth about where this thread stands. \
     Close \"pushed\" only after a verified `git` push; pr-reviewed and \
     merged can't be auto-verified in this session, so leave them for the \
     user to confirm rather than marking them done on faith.";

/// Remove the trail sections from a finished system prompt, for detached
/// subagents (side agents, fleet lanes). Their registries drop `update_trail`
/// (see `subagent_tools`): a lane's transcript lives in a throwaway in-memory
/// store, so a charted trail would never reach the user's board — the mandate
/// would be either wasted tokens or a wasted call. The parent's prompt is
/// inherited verbatim otherwise (environment, conventions, project metadata),
/// so this strips exactly the constants the builder spliced in.
pub(crate) fn strip_trail_sections(prompt: &str) -> String {
    prompt
        .replace(TRAIL_TOOL_LIST_ENTRY, "")
        .replace(TRAIL_GUIDELINE_WITH_GH, "")
        .replace(TRAIL_GUIDELINE_NO_GH, "")
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
         open document. Don't use `canvas` for short answers or quick snippets; \
         opening a panel for those is disruptive."
    } else {
        ""
    };
    let gh_tool = if tools.gh { ", `gh` (GitHub PRs)" } else { "" };
    let trail_tool = if tools.trail {
        TRAIL_TOOL_LIST_ENTRY
    } else {
        ""
    };
    // The trail guideline names the shipping stages the model can actually
    // verify: with `gh` the review/merge state is checked; without it, only
    // the push is verifiable and the rest is explicitly the user's to confirm.
    let trail_guideline = match (tools.trail, tools.gh) {
        (false, _) => "",
        (true, true) => TRAIL_GUIDELINE_WITH_GH,
        (true, false) => TRAIL_GUIDELINE_NO_GH,
    };
    let open_file_guideline = if tools.open_file {
        "\n- After creating or substantially rewriting a project file the user \
         will want to look at — or when walking them through one — call \
         `open_file` to put it in their file viewer beside the chat instead of \
         pasting its contents. Open the one or two files that matter, not every \
         file you touch."
    } else {
        ""
    };
    format!(
        "You are oxen-harness, an open source coding agent working in the user's \
         project directory. Available tools: `find_files` (locate files by glob), \
         `search_files` (regex content search), `read_file` (line-numbered, supports \
         offset/limit), `write_file`, `edit_file` (exact-string patch), `run_shell`, \
         `git`{gh_tool}, `update_plan` (maintain a task checklist){trail_tool}, \
         `ask_user_question` (interview the user){web_tool}{canvas_tool}{open_file_tool}.\n\n\
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
           concrete success criterion first so a wrong approach is caught early.{trail_guideline}\n\
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
           depend on it, and tell the user what's blocked and why.\n\
         - When a product/design/implementation decision is genuinely ambiguous and \
           has multiple reasonable approaches with real trade-offs, call \
           `ask_user_question` to interview the user instead of guessing. Keep \
           options concise and distinct; don't add an 'Other' option (the user can \
           always type their own). Don't ask about trivia you can decide yourself.{canvas_guideline}{open_file_guideline}\n\
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
    "You described what you'll do but didn't actually call a tool to do it. \
     If you intended to take an action — open a `canvas`, write or edit a file, run a command — \
     make that tool call now. If you were genuinely finished, reply with your final answer.";

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

/// The one-shot corrective appended when a turn did real work (edited files or
/// ran shell commands) but the session has no charted trail — the "question
/// that quietly became a work session" failure mode the prompt guideline alone
/// doesn't reliably catch (see `Agent::drive_turn`). Sent only on the retry
/// request and never persisted.
pub(crate) const TRAIL_STALL_NUDGE: &str =
    "You did real work this turn, but this session has no charted trail. Call \
     `update_trail` now: title the thread and chart its macro stages, marking \
     stages already passed as done and the stage you're in as current. Then \
     give your final answer.";

/// The one-shot corrective appended when the same tool call has repeated with
/// identical arguments *and* an identical result several times in a row (see
/// [`crate::loopguard`]). Each repeat re-bills the whole context for zero new
/// information. Sent only on the next request and never persisted.
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
    // "let me know" is a sign-off, not an unperformed action — don't nudge on it.
    SIGNALS.iter().any(|s| t.contains(s)) && !t.contains("let me know")
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
        assert!(!bare.contains("`gh`"));
        assert!(!bare.contains("update_trail"));

        let full = system_prompt_with(OptionalTools {
            web_search: true,
            canvas: true,
            open_file: true,
            gh: true,
            trail: true,
        });
        assert!(full.contains("`web_search` (Brave web search)"));
        assert!(full.contains("`canvas` (show a document in a side panel)"));
        assert!(full.contains("`open_file` (show a project file in the user's file viewer)"));
        assert!(full.contains("`open_file` to put it in their file viewer"));
        assert!(full.contains("`gh` (GitHub PRs)"));
        assert!(full.contains("`update_trail` (chart the session's journey)"));
    }

    /// The trail guideline must only order the model to verify through tools
    /// it actually has: with `gh` disabled the prompt neither lists the tool
    /// nor demands pr_view checks; with `update_trail` disabled the whole
    /// trail mandate disappears.
    #[test]
    fn trail_guideline_adapts_to_the_registered_tools() {
        let both = system_prompt_with(OptionalTools {
            gh: true,
            trail: true,
            ..OptionalTools::default()
        });
        assert!(both.contains("`gh` pr_view to confirm review and merge state"));

        let trail_only = system_prompt_with(OptionalTools {
            trail: true,
            ..OptionalTools::default()
        });
        assert!(trail_only.contains("update_trail"));
        assert!(
            !trail_only.contains("`gh`"),
            "must not order an unregistered tool"
        );
        assert!(trail_only.contains("leave them for the user to confirm"));

        let neither = system_prompt_with(OptionalTools::default());
        assert!(!neither.contains("update_trail"));
        assert!(!neither.contains("pr-reviewed"));

        // The trigger is a condition (first edit/shell command), not a
        // "before you start" moment a question-shaped session sails past, and
        // it explicitly severs itself from update_plan's default-off guidance.
        assert!(both.contains("right before your first file edit or shell command"));
        assert!(both.contains("NOT optional for work sessions"));

        // The convenience default advertises the default registry (gh + trail
        // are registered unless the user disables them).
        let default = default_system_prompt(false);
        assert!(default.contains("`gh` (GitHub PRs)"));
        assert!(default.contains("update_trail"));
    }

    /// Subagents inherit the parent's finished prompt but drop `update_trail`
    /// from their registry — stripping must remove every trail section (tool
    /// list entry + guideline, both gh variants) and nothing else.
    #[test]
    fn strip_trail_sections_removes_the_mandate_from_any_variant() {
        for gh in [false, true] {
            let full = system_prompt_with(OptionalTools {
                gh,
                trail: true,
                ..OptionalTools::default()
            });
            let stripped = strip_trail_sections(&full);
            assert!(!stripped.contains("update_trail"), "gh={gh}");
            assert!(!stripped.contains("pr-reviewed"), "gh={gh}");
            // The rest of the prompt survives untouched.
            assert!(stripped.contains("update_plan"));
            if gh {
                assert!(stripped.contains("`gh` (GitHub PRs)"));
            }
            assert_eq!(
                stripped,
                system_prompt_with(OptionalTools {
                    gh,
                    ..OptionalTools::default()
                }),
                "stripping must equal never having advertised the trail (gh={gh})"
            );
        }
        // Still works on a full prompt with the environment section appended,
        // the shape a subagent actually inherits.
        let inherited = system_prompt_with_env(
            OptionalTools::default_registry(),
            std::path::Path::new("/w"),
        );
        assert!(!strip_trail_sections(&inherited).contains("update_trail"));
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
    }
}
