//! The agent's system prompt and the "unfulfilled intent" nudge.
//!
//! The prompt is assembled from a fixed core plus two optional sections that are
//! only included when the host actually registered the `web_search` and `canvas`
//! tools, so the model is never told about a tool the registry would reject.

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
pub(crate) const INTENT_NUDGE: &str =
    "You described what you'll do but didn't actually call a tool to do it. \
     If you intended to take an action — open a `canvas`, write or edit a file, run a command — \
     make that tool call now. If you were genuinely finished, reply with your final answer.";

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
    fn optional_tool_sections_appear_only_when_enabled() {
        let bare = system_prompt_with(false, false);
        assert!(!bare.contains("web_search"));
        assert!(!bare.contains("`canvas`"));

        let full = system_prompt_with(true, true);
        assert!(full.contains("`web_search` (Brave web search)"));
        assert!(full.contains("`canvas` (show a document in a side panel)"));
    }

    #[test]
    fn environment_section_names_the_working_directory() {
        let section = environment_section(std::path::Path::new("/tmp/project"));
        assert!(section.contains("/tmp/project"));
        assert!(system_prompt_with_env(false, false, std::path::Path::new("/w")).contains("/w"));
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
