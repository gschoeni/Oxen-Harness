//! REPL command parsing.
//!
//! Lines beginning with `/` are slash commands; everything else is a prompt for
//! the agent. Parsing is split out here so it can be unit-tested without a live
//! terminal or model.

/// A parsed REPL input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Empty input (whitespace only) — ignored.
    Empty,
    /// Quit the REPL.
    Exit,
    /// Print help.
    Help,
    /// Switch models: `/model` opens the interactive picker, `/model <id>`
    /// switches directly (an id not yet in the catalog is saved as a custom
    /// entry, so any model the endpoint serves can be typed in directly).
    Model(Option<String>),
    /// Export the current session transcript as JSONL (optional path).
    Export(Option<String>),
    /// Theme command: the whitespace-split args after `/theme` (empty = open picker).
    Theme(Vec<String>),
    /// Queue command: the raw text after `/queue` (e.g. `add fix the bug`), or
    /// `None` to list the queue. Kept raw so message text keeps its spaces.
    Queue(Option<String>),
    /// Loop command: the raw text after `/loop` (e.g. `run default` or
    /// `goal make tests pass`), or `None` to list loops. Kept raw so goal text
    /// keeps its spaces.
    Loop(Option<String>),
    /// Code review: the raw text after `/code-review` (a base branch, or a
    /// subcommand like `steps`), or `None` to review uncommitted changes.
    CodeReview(Option<String>),
    /// Set (or, with `None`, show) the "Departing" location shown in the main
    /// menu banner. Kept raw so multi-word place names keep their spaces.
    Departing(Option<String>),
    /// List the skills discovered for this workspace (global + project).
    Skills,
    /// Set the Oxen API key: `/auth` opens a masked entry box; `/auth <key>`
    /// sets it directly.
    Auth(Option<String>),
    /// Show or switch context compression: `/compression` opens a picker;
    /// `/compression off|audit|on` switches directly.
    Compression(Option<String>),
    /// Re-drive the last turn against the existing transcript — for a turn
    /// that died (provider error, no internet), possibly after `/model`
    /// switched to a working endpoint. No user message is re-appended.
    Retry,
    /// A prompt to send to the agent.
    Prompt(String),
}

/// Parse a line of REPL input into a [`Command`].
pub fn parse_command(line: &str) -> Command {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Command::Empty;
    }
    if !trimmed.starts_with('/') {
        return Command::Prompt(trimmed.to_string());
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let rest = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    match cmd {
        "/exit" | "/quit" | "/q" => Command::Exit,
        "/help" | "/?" => Command::Help,
        "/model" => Command::Model(rest),
        "/export" => Command::Export(rest),
        "/theme" | "/themes" => Command::Theme(
            rest.map(|r| r.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
        ),
        "/queue" => Command::Queue(rest),
        "/loop" | "/loops" => Command::Loop(rest),
        "/code-review" | "/review" => Command::CodeReview(rest),
        "/departing" => Command::Departing(rest),
        "/skills" | "/skill" => Command::Skills,
        "/auth" | "/login" => Command::Auth(rest),
        "/compression" | "/compress" => Command::Compression(rest),
        "/retry" | "/continue" => Command::Retry,
        // Unknown slash command: treat the whole line as a prompt so users can
        // still send text that happens to start with a slash.
        _ => Command::Prompt(trimmed.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_is_empty() {
        assert_eq!(parse_command("   "), Command::Empty);
    }

    #[test]
    fn plain_text_is_a_prompt() {
        assert_eq!(
            parse_command("fix the failing test"),
            Command::Prompt("fix the failing test".into())
        );
    }

    #[test]
    fn exit_aliases() {
        assert_eq!(parse_command("/exit"), Command::Exit);
        assert_eq!(parse_command("/quit"), Command::Exit);
        assert_eq!(parse_command("/q"), Command::Exit);
    }

    #[test]
    fn model_with_and_without_argument() {
        assert_eq!(parse_command("/model"), Command::Model(None));
        assert_eq!(
            parse_command("/model claude-sonnet-4-6"),
            Command::Model(Some("claude-sonnet-4-6".into()))
        );
    }

    #[test]
    fn export_with_optional_path() {
        assert_eq!(parse_command("/export"), Command::Export(None));
        assert_eq!(
            parse_command("/export out.jsonl"),
            Command::Export(Some("out.jsonl".into()))
        );
    }

    #[test]
    fn theme_command_splits_args() {
        assert_eq!(parse_command("/theme"), Command::Theme(vec![]));
        assert_eq!(
            parse_command("/theme use Midnight"),
            Command::Theme(vec!["use".into(), "Midnight".into()])
        );
        assert_eq!(
            parse_command("/theme new a cozy autumn vibe"),
            Command::Theme(vec![
                "new".into(),
                "a".into(),
                "cozy".into(),
                "autumn".into(),
                "vibe".into()
            ])
        );
    }

    #[test]
    fn queue_keeps_raw_remainder() {
        assert_eq!(parse_command("/queue"), Command::Queue(None));
        assert_eq!(
            parse_command("/queue add fix the bug"),
            Command::Queue(Some("add fix the bug".into()))
        );
        // `/q` stays an exit alias, not a queue command.
        assert_eq!(parse_command("/q"), Command::Exit);
    }

    #[test]
    fn loop_keeps_raw_remainder() {
        assert_eq!(parse_command("/loop"), Command::Loop(None));
        assert_eq!(
            parse_command("/loop run default"),
            Command::Loop(Some("run default".into()))
        );
        assert_eq!(
            parse_command("/loop goal make every test pass"),
            Command::Loop(Some("goal make every test pass".into()))
        );
    }

    #[test]
    fn code_review_aliases_and_remainder() {
        assert_eq!(parse_command("/code-review"), Command::CodeReview(None));
        assert_eq!(parse_command("/review"), Command::CodeReview(None));
        assert_eq!(
            parse_command("/code-review main"),
            Command::CodeReview(Some("main".into()))
        );
        assert_eq!(
            parse_command("/review steps"),
            Command::CodeReview(Some("steps".into()))
        );
    }

    #[test]
    fn departing_keeps_raw_remainder() {
        assert_eq!(parse_command("/departing"), Command::Departing(None));
        assert_eq!(
            parse_command("/departing Independence, Missouri"),
            Command::Departing(Some("Independence, Missouri".into()))
        );
    }

    #[test]
    fn skills_aliases() {
        assert_eq!(parse_command("/skills"), Command::Skills);
        assert_eq!(parse_command("/skill"), Command::Skills);
    }

    #[test]
    fn auth_with_and_without_inline_key() {
        assert_eq!(parse_command("/auth"), Command::Auth(None));
        assert_eq!(parse_command("/login"), Command::Auth(None));
        assert_eq!(
            parse_command("/auth sk-abc123"),
            Command::Auth(Some("sk-abc123".into()))
        );
    }

    #[test]
    fn retry_aliases() {
        assert_eq!(parse_command("/retry"), Command::Retry);
        assert_eq!(parse_command("/continue"), Command::Retry);
    }

    #[test]
    fn compression_with_and_without_mode() {
        assert_eq!(parse_command("/compression"), Command::Compression(None));
        assert_eq!(
            parse_command("/compress audit"),
            Command::Compression(Some("audit".into()))
        );
    }

    #[test]
    fn unknown_slash_command_is_treated_as_prompt() {
        assert_eq!(
            parse_command("/frobnicate now"),
            Command::Prompt("/frobnicate now".into())
        );
    }
}
