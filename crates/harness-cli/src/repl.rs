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
    /// Show the current model, or switch to a new one.
    Model(Option<String>),
    /// Export the current session transcript as JSONL (optional path).
    Export(Option<String>),
    /// Theme command: the whitespace-split args after `/theme` (empty = open picker).
    Theme(Vec<String>),
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
    fn unknown_slash_command_is_treated_as_prompt() {
        assert_eq!(
            parse_command("/frobnicate now"),
            Command::Prompt("/frobnicate now".into())
        );
    }
}
