//! Visual theming for the `oxen-harness` REPL.
//!
//! The *structure* (welcome banner, in-place spinners, transparent tool lines,
//! tombstone exit screen) lives here, but every color and phrase comes from the
//! active [`harness_theme::Theme`], so the whole personality is configurable and
//! shareable. The default theme is the 1980s **Oregon Trail** look.
//!
//! Color is emitted as 24-bit ("truecolor") ANSI and disabled automatically
//! when stdout is not a TTY, when `NO_COLOR` is set, or for `TERM=dumb`, so
//! piped output stays clean.
//!
//! This module owns the [`Ui`] color/theme handle and the small always-present
//! input chrome ([`prompt`], [`divider`]); the larger themed compositions live
//! in focused submodules — [`screens`] (banner, help, exit), [`models`] (the
//! local-model table + progress bar), and [`spinner`] (animated status lines).

use std::io::{self, IsTerminal};
use std::sync::Arc;

use harness_theme::{Color, Theme};

use crate::almanac::pick;

mod models;
mod screens;
mod spinner;

pub use models::{models_table, progress_bar, ModelRow};
pub(crate) use screens::format_usd;
pub use screens::{banner, death_screen, help};
pub(crate) use spinner::LiveSpinner;
pub use spinner::Spinner;

/// An RGB color for 24-bit ANSI.
pub(crate) type Rgb = (u8, u8, u8);

/// Whether/how to render color, plus the active theme. Cheap to clone (the
/// theme sits behind an `Arc`); pass it around freely.
#[derive(Clone)]
pub struct Ui {
    color: bool,
    theme: Arc<Theme>,
}

impl Ui {
    /// Build a UI with an explicit color setting and theme (used in tests).
    #[cfg(test)]
    pub fn with(color: bool, theme: Arc<Theme>) -> Self {
        Ui { color, theme }
    }

    /// Detect terminal capabilities for stdout, using `theme` for styling.
    pub fn detect(theme: Arc<Theme>) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let dumb = std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);
        Ui {
            color: io::stdout().is_terminal() && !no_color && !dumb,
            theme,
        }
    }

    /// A color-disabled UI on the default theme (used for non-TTY output/tests).
    #[cfg(test)]
    pub fn plain() -> Self {
        Ui {
            color: false,
            theme: Arc::new(Theme::default()),
        }
    }

    /// The active theme.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// A copy of this UI with a different theme, preserving color settings.
    pub fn with_theme(&self, theme: Arc<Theme>) -> Ui {
        Ui {
            color: self.color,
            theme,
        }
    }

    /// Update the "Departing" location shown in the main-menu banner.
    ///
    /// The departing location is the first `flavor_top` row of the active
    /// theme. Because the theme sits behind an `Arc`, this clones it, rewrites
    /// that row's value (preserving its themed label, e.g. "Departing" or
    /// "Location"), and swaps in the modified copy. The label used for a
    /// freshly-created row is returned so callers can report what changed.
    pub fn set_departing(&mut self, value: &str) -> String {
        let mut theme = (*self.theme).clone();
        match theme.voice.flavor_top.first_mut() {
            Some(row) => row[1] = value.to_string(),
            None => theme
                .voice
                .flavor_top
                .push(["Departing".to_string(), value.to_string()]),
        }
        let label = theme.voice.flavor_top[0][0].clone();
        self.theme = Arc::new(theme);
        label
    }

    /// The current "Departing" location (first `flavor_top` row value), if any.
    pub fn departing(&self) -> Option<(&str, &str)> {
        self.theme
            .voice
            .flavor_top
            .first()
            .map(|row| (row[0].as_str(), row[1].as_str()))
    }

    /// Whether in-place animations (spinners, progress bars) should run. They
    /// rely on ANSI control codes, so they're tied to color support.
    pub fn animates(&self) -> bool {
        self.color
    }

    fn paint(&self, text: &str, color: Color) -> String {
        if self.color {
            paint(text, color.rgb())
        } else {
            text.to_string()
        }
    }

    fn bold(&self, text: &str, color: Color) -> String {
        if self.color {
            let (r, g, b) = color.rgb();
            format!("\x1b[1;38;2;{r};{g};{b}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    // Semantic colors, resolved from the active palette.
    pub fn title(&self, s: &str) -> String {
        self.bold(s, self.theme.palette.title)
    }
    pub fn brown(&self, s: &str) -> String {
        self.paint(s, self.theme.palette.secondary)
    }
    pub fn green(&self, s: &str) -> String {
        self.paint(s, self.theme.palette.primary)
    }
    pub fn cream(&self, s: &str) -> String {
        self.paint(s, self.theme.palette.text)
    }
    pub fn dim(&self, s: &str) -> String {
        self.paint(s, self.theme.palette.muted)
    }
    pub fn red(&self, s: &str) -> String {
        self.paint(s, self.theme.palette.danger)
    }
    pub fn accent(&self, s: &str) -> String {
        self.bold(s, self.theme.palette.primary)
    }

    // Inline markdown styles.
    pub fn strong(&self, s: &str) -> String {
        self.bold(s, self.theme.palette.text)
    }
    pub fn em(&self, s: &str) -> String {
        if self.color {
            let (r, g, b) = self.theme.palette.text.rgb();
            format!("\x1b[3;38;2;{r};{g};{b}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn code(&self, s: &str) -> String {
        self.paint(s, self.theme.palette.primary)
    }
    pub fn link(&self, s: &str) -> String {
        if self.color {
            let (r, g, b) = self.theme.palette.link.rgb();
            format!("\x1b[4;38;2;{r};{g};{b}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    /// Phrases shown while the model is thinking.
    pub fn thinking(&self) -> Vec<String> {
        self.theme.voice.thinking.clone()
    }

    /// Phrases shown while the model is actively streaming a written response
    /// (so a pause mid-text still animates). Uses the theme's `write_file` verbs
    /// when present — a natural "writing" feel — and otherwise falls back to the
    /// thinking phrases so the indicator is never empty.
    pub fn writing(&self) -> Vec<String> {
        let verbs = self.theme.tool_verbs(harness_tools::WRITE_FILE_TOOL);
        if verbs.is_empty() || verbs == ["Working"] {
            self.thinking()
        } else {
            verbs
        }
    }

    /// Spinner verbs that fit a given tool.
    pub fn tool_verbs(&self, tool: &str) -> Vec<String> {
        self.theme.tool_verbs(tool)
    }

    /// A flavorful "you died" line for a real agent error.
    pub fn death(&self) -> String {
        pick(&self.theme.voice.deaths).to_string()
    }
}

pub(crate) fn paint(text: &str, rgb: Rgb) -> String {
    let (r, g, b) = rgb;
    format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

/// The REPL input prompt (mirrors the game's "What is your choice?").
pub fn prompt(ui: &Ui) -> String {
    let v = &ui.theme.voice;
    format!(
        "{} {} ",
        ui.brown(&v.prompt_icon),
        ui.accent(&v.prompt_label)
    )
}

/// A faint full-width horizontal rule, drawn above the input prompt to set the
/// typing area apart from the agent's output above it (à la Claude Code).
pub fn divider(ui: &Ui) -> String {
    let width = crossterm::terminal::size()
        .map(|(cols, _)| cols as usize)
        .unwrap_or(80)
        .clamp(8, 200);
    ui.dim(&"─".repeat(width))
}

/// The scrollwork divider, echoing the game's title screen. Shared by the
/// startup banner and the local-model table.
fn flourish(ui: &Ui) -> String {
    format!(
        "  {}\n",
        ui.brown("╾━━━━━━━━━━━━━━━━━━━━━◆━━━━━━━━━━━━━━━━━━━━━╼")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colored() -> Ui {
        Ui::with(true, Arc::new(Theme::default()))
    }

    #[test]
    fn no_color_paint_is_plain() {
        let ui = Ui::plain();
        assert_eq!(ui.title("hi"), "hi");
        assert_eq!(ui.dim("trail"), "trail");
    }

    #[test]
    fn color_paint_wraps_in_ansi() {
        let ui = colored();
        let s = ui.green("go");
        assert!(s.starts_with("\x1b[38;2;"));
        assert!(s.ends_with("\x1b[0m"));
        assert!(s.contains("go"));
    }

    #[test]
    fn tool_verbs_are_themed_and_nonempty() {
        let ui = Ui::plain();
        for tool in [
            "read_file",
            "write_file",
            "edit_file",
            "find_files",
            "search_files",
            "run_shell",
            "git",
            "web_search",
            "ask_user_question",
            "wat",
        ] {
            assert!(!ui.tool_verbs(tool).is_empty());
        }
    }

    #[test]
    fn themes_change_phrases() {
        let synth = Ui::with(
            false,
            Arc::new(harness_theme::builtins::by_name("synthwave").unwrap()),
        );
        assert!(prompt(&synth).contains("ride ❯"));
        assert!(synth
            .theme()
            .voice
            .deaths
            .iter()
            .any(|d| d.contains("GAME OVER")));
    }
}
