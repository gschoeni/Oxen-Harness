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
    /// Set (or, with `None`, show) the "Location" shown in the hero/opening
    /// screen. Kept raw so multi-word place names keep their spaces.
    Location(Option<String>),
    /// List the skills discovered for this workspace (global + project).
    Skills,
    /// Set the API key and provider endpoint: `/auth` walks a base-URL card
    /// pre-filled with the current endpoint (Enter accepts it; edit it to move
    /// to another Oxen server or any OpenAI-compatible provider) then a masked
    /// key box; `/auth <key>`, `/auth host <host-or-url>`, and
    /// `/auth <host-or-url> <key>` set them directly.
    Auth(Option<String>),
    /// Show or switch context compression: `/compression` opens a picker;
    /// `/compression off|audit|on` switches directly.
    Compression(Option<String>),
    /// Show or switch the permission mode: `/permissions` prints the rules in
    /// force and opens a picker; `/permissions relaxed|cautious|bypass`
    /// switches directly.
    Permissions(Option<String>),
    /// Show all-time input/output tokens and estimated spend by model.
    Usage,
    /// Open the agent-started dev server in the browser (live preview).
    Preview,
    /// Re-drive the last turn against the existing transcript — for a turn
    /// that died (provider error, no internet), possibly after `/model`
    /// switched to a working endpoint. No user message is re-appended.
    Retry,
    /// A prompt to send to the agent.
    Prompt(String),
}

/// How a command's *argument* completes in the composer.
pub(crate) enum ArgCompleter {
    /// No argument completion.
    None,
    /// A fixed set of `(choice, description)` values, offered as a picker.
    Static(&'static [(&'static str, &'static str)]),
    /// The model catalog (cloud + installed local), offered as a picker.
    Models,
}

/// One slash command: its canonical name, aliases, the description shown by
/// completion, how to build its [`Command`], and how its argument completes.
///
/// This registry is the single source of truth — [`parse_command`] and the
/// composer's completion list both derive from it, so a new command is one row
/// here (plus its `Command` variant and `handle_line` dispatch arm).
pub(crate) struct SlashSpec {
    pub(crate) name: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) description: &'static str,
    pub(crate) build: fn(Option<String>) -> Command,
    pub(crate) completer: ArgCompleter,
}

impl SlashSpec {
    pub(crate) fn matches(&self, cmd: &str) -> bool {
        self.name == cmd || self.aliases.contains(&cmd)
    }
}

/// Split whitespace-separated theme args (`/theme use Midnight`).
fn theme_args(rest: Option<String>) -> Command {
    Command::Theme(
        rest.map(|r| r.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default(),
    )
}

/// Every slash command, in the order the completion list shows them.
pub(crate) const SLASH_COMMANDS: &[SlashSpec] = &[
    SlashSpec {
        name: "/model",
        aliases: &[],
        description: "pick, switch, or add a model",
        build: Command::Model,
        completer: ArgCompleter::Models,
    },
    SlashSpec {
        name: "/theme",
        aliases: &["/themes"],
        description: "change the theme",
        build: theme_args,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/queue",
        aliases: &[],
        description: "manage the message queue",
        build: Command::Queue,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/loop",
        aliases: &["/loops"],
        description: "run or list loops",
        build: Command::Loop,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/code-review",
        aliases: &["/review"],
        description: "review your changes (find → verify → report)",
        build: Command::CodeReview,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/export",
        aliases: &[],
        description: "export the transcript",
        build: Command::Export,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/skills",
        aliases: &["/skill"],
        description: "list the skills on hand",
        build: |_| Command::Skills,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/retry",
        aliases: &["/continue"],
        description: "re-drive a turn that died mid-stream",
        build: |_| Command::Retry,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/location",
        aliases: &[],
        description: "set your location (banner + hero screen)",
        build: Command::Location,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/departing",
        aliases: &[],
        description: "set your location (themed alias)",
        build: Command::Departing,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/auth",
        aliases: &["/login"],
        description: "set your API key and provider host",
        build: Command::Auth,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/compression",
        aliases: &["/compress"],
        description: "switch context compression (off/audit/on)",
        build: Command::Compression,
        completer: ArgCompleter::Static(&[
            ("off", "send every tool result untouched"),
            ("audit", "measure savings, change nothing"),
            ("on", "compress stale tool output"),
        ]),
    },
    SlashSpec {
        name: "/permissions",
        aliases: &["/permission", "/perms"],
        description: "when the agent asks first (relaxed/cautious/bypass)",
        build: Command::Permissions,
        completer: ArgCompleter::Static(&[
            ("relaxed", "only dangerous commands ask first"),
            ("cautious", "only read-only commands run unprompted"),
            ("bypass", "never ask (circuit breakers still refuse)"),
        ]),
    },
    SlashSpec {
        name: "/usage",
        aliases: &[],
        description: "show tokens and estimated spend by model",
        build: |_| Command::Usage,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/preview",
        aliases: &["/browser"],
        description: "open the running app in your browser",
        build: |_| Command::Preview,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/help",
        aliases: &["/?"],
        description: "show help",
        build: |_| Command::Help,
        completer: ArgCompleter::None,
    },
    SlashSpec {
        name: "/exit",
        aliases: &["/quit", "/q"],
        description: "quit",
        build: |_| Command::Exit,
        completer: ArgCompleter::None,
    },
];

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

    match SLASH_COMMANDS.iter().find(|spec| spec.matches(cmd)) {
        Some(spec) => (spec.build)(rest),
        // Unknown slash command: treat the whole line as a prompt so users can
        // still send text that happens to start with a slash.
        None => Command::Prompt(trimmed.to_string()),
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
    fn usage_command() {
        assert_eq!(parse_command("/usage"), Command::Usage);
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
    fn location_keeps_raw_remainder() {
        assert_eq!(parse_command("/location"), Command::Location(None));
        assert_eq!(
            parse_command("/location Fort Laramie, Wyoming"),
            Command::Location(Some("Fort Laramie, Wyoming".into()))
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

    #[test]
    fn registry_rows_are_well_formed_and_unambiguous() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for spec in SLASH_COMMANDS {
            // Every name/alias is slash-prefixed, unique, and actually parses
            // to its own command (never falls through to Prompt).
            for cmd in std::iter::once(&spec.name).chain(spec.aliases) {
                assert!(cmd.starts_with('/'), "`{cmd}` must start with /");
                assert!(seen.insert(*cmd), "`{cmd}` is claimed twice");
                assert!(
                    !matches!(parse_command(cmd), Command::Prompt(_)),
                    "`{cmd}` must parse as a command"
                );
            }
            assert!(
                !spec.description.is_empty(),
                "{} needs a description for the completion list",
                spec.name
            );
        }
    }
}
