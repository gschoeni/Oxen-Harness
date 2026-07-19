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
//! ## Light themes on dark terminals
//!
//! The CLI cannot repaint the terminal's background, so a light-on-paper theme
//! (New York Times, Cupertino) declares its intended paper via the palette's
//! app-only `background` slot, and the [`Ui`] uses that to tell "the canvas is
//! the light paper" (desktop app) from "the canvas is the terminal's own
//! background" — in which case near-black ink colors would be invisible. When
//! the background isn't the declared paper, every emitted foreground is lifted
//! just enough (keeping its hue) to stay legible on a dark terminal; on the
//! declared paper, colors pass through untouched.
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
    /// `false` when the theme paints its own background (the desktop app), so
    /// palette colors render as authored. `true` when the terminal's own
    /// background shows through — the CLI can't repaint it — so colors designed
    /// for light paper are lifted just enough to stay legible on dark glass.
    cli_background: bool,
}

/// sRGB channel → linear-light intensity.
fn linearize(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear-light intensity → sRGB channel.
fn unlinearize(c: f32) -> u8 {
    let c = if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

/// WCAG relative luminance of an sRGB color, in `0.0..=1.0`.
fn relative_luminance(Color { r, g, b }: Color) -> f32 {
    0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b)
}

/// Adjust `c` for display on a background the theme doesn't control. Themes
/// declaring light paper get their below-bar colors lifted toward the paper
/// tone — in linear light, so the mix lands on a known luminance — and their
/// saturation pushed back up so hues survive the lift (black ink becomes warm
/// gray, navy stays blue, press red stays red); colors already legible pass
/// through unchanged. Dark themes are tuned for dark terminals and left alone.
fn adapt_for_cli(c: Color, paper: Color) -> Color {
    const TEXT_MIN: f32 = 0.30; // near-black ink on the dark glass (~5:1)
    const ACCENT_MIN: f32 = 0.14; // accent lift floor (~1.8:1)…
    const ACCENT_SAT: f32 = 1.7; // …with saturation pushed back so hues read
    let lum = relative_luminance(c);
    let paper_lum = relative_luminance(paper);
    // Only a light paper can serve as a lift target. On a dark paper the mix
    // below can never reach the floor, so `t` clamps to 1.0 and the color
    // becomes the background itself — invisible. Dark themes are authored for
    // dark terminals; keep them as-is.
    if paper_lum <= TEXT_MIN || lum >= paper_lum {
        return c;
    }
    if lum >= TEXT_MIN {
        return c; // already legible on dark glass
    }
    // True ink (near-black) lifts straight to a paper-gray; accents lift
    // partway and get their saturation pushed back so the hue still reads.
    let ink = lum < 0.03;
    let (floor, sat) = if ink {
        (TEXT_MIN, 1.0)
    } else {
        (ACCENT_MIN, ACCENT_SAT)
    };
    if lum >= floor {
        return c; // already legible; nothing to lift
    }
    let cl = (linearize(c.r), linearize(c.g), linearize(c.b));
    let pl = (linearize(paper.r), linearize(paper.g), linearize(paper.b));
    let t = ((floor - lum) / (paper_lum - lum)).clamp(0.0, 1.0);
    let mix = |c: f32, p: f32| c + (p - c) * t;
    let base = (mix(cl.0, pl.0), mix(cl.1, pl.1), mix(cl.2, pl.2));
    let luma = 0.2126 * base.0 + 0.7152 * base.1 + 0.0722 * base.2;
    let channel = |v: f32| unlinearize(luma + (v - luma) * sat);
    Color::new(channel(base.0), channel(base.1), channel(base.2))
}

impl Ui {
    /// Build a UI with an explicit color setting and theme (used in tests).
    #[cfg(test)]
    pub fn with(color: bool, theme: Arc<Theme>) -> Self {
        Ui {
            color,
            theme,
            cli_background: true,
        }
    }

    /// Detect terminal capabilities for stdout, using `theme` for styling.
    pub fn detect(theme: Arc<Theme>) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let dumb = std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);
        Ui {
            color: io::stdout().is_terminal() && !no_color && !dumb,
            theme,
            cli_background: true,
        }
    }

    /// A color-disabled UI on the default theme (used for non-TTY output/tests).
    #[cfg(test)]
    pub fn plain() -> Self {
        Ui {
            color: false,
            theme: Arc::new(Theme::default()),
            cli_background: true,
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
            cli_background: self.cli_background,
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

    /// Whether ANSI color is being emitted at all (TTY, no `NO_COLOR`, not
    /// `TERM=dumb`) — gates styling that goes beyond this palette's slots,
    /// like syntax highlighting.
    pub fn colored(&self) -> bool {
        self.color
    }

    /// Resolve a palette color for the actual canvas: as-authored when the
    /// theme paints its own background, lifted for legibility when the dark
    /// terminal background shows through (see the module docs).
    fn fg(&self, color: Color) -> Color {
        if self.cli_background {
            adapt_for_cli(color, self.theme.palette.background)
        } else {
            color
        }
    }

    fn paint(&self, text: &str, color: Color) -> String {
        if self.color {
            paint(text, self.fg(color).rgb())
        } else {
            text.to_string()
        }
    }

    fn bold(&self, text: &str, color: Color) -> String {
        if self.color {
            let (r, g, b) = self.fg(color).rgb();
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
            let (r, g, b) = self.fg(self.theme.palette.text).rgb();
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
            let (r, g, b) = self.fg(self.theme.palette.link).rgb();
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

    /// The painted RGB of a semantic slot under `ui`, parsed back out of the
    /// ANSI escape so adaptation can be asserted on numerically.
    fn painted_rgb(ui: &Ui, slot: fn(&Ui, &str) -> String) -> Color {
        let s = slot(ui, "x");
        let inner = s
            .trim_start_matches("\x1b[1;38;2;")
            .trim_start_matches("\x1b[38;2;")
            .trim_start_matches("\x1b[3;38;2;")
            .trim_start_matches("\x1b[4;38;2;");
        let nums: Vec<u8> = inner
            .split([';', 'm'])
            .take(3)
            .filter_map(|n| n.parse().ok())
            .collect();
        assert_eq!(nums.len(), 3, "could not parse rgb out of {s:?}");
        Color::new(nums[0], nums[1], nums[2])
    }

    #[test]
    fn light_theme_ink_is_lifted_on_the_cli_background() {
        let nyt = Arc::new(harness_theme::builtins::by_name("new york times").unwrap());
        let ui = Ui::with(true, nyt.clone());
        let p = &nyt.palette;

        // The near-black masthead/body ink becomes legible on dark glass…
        for slot in [Ui::title as fn(&Ui, &str) -> String, Ui::cream, Ui::strong] {
            assert!(
                relative_luminance(painted_rgb(&ui, slot)) >= 0.28,
                "lifted ink should clear ~5:1 on a dark terminal"
            );
        }
        // …and the warm-gray byline/muted tones lift well off the background.
        for slot in [Ui::dim as fn(&Ui, &str) -> String, Ui::brown] {
            assert!(
                relative_luminance(painted_rgb(&ui, slot)) >= 0.13,
                "lifted grays should stand off dark glass"
            );
        }
        // …while hues survive: press red stays red, navy link stays blue.
        let dominant = |c: Color, lo: u8| (c.r as u16) > (c.g as u16) * 3 / 2 && c.r > lo;
        let accent = painted_rgb(&ui, Ui::accent);
        assert!(dominant(accent, 140), "press red stays red: {accent:?}");
        let red = painted_rgb(&ui, Ui::red);
        assert!(dominant(red, 140), "danger stays red: {red:?}");
        let link = painted_rgb(&ui, Ui::link);
        assert!(link.b > link.r && link.b > 100, "navy stays blue: {link:?}");

        // A UI painting its own background (the app) keeps the authored ink.
        let on_paper = Ui {
            cli_background: false,
            ..ui.clone()
        };
        assert_eq!(painted_rgb(&on_paper, Ui::cream), p.text);
        assert_eq!(painted_rgb(&on_paper, Ui::title), p.title);
    }

    #[test]
    fn dark_themes_pass_through_unchanged() {
        for name in ["oregon trail", "midnight", "synthwave"] {
            let theme = Arc::new(harness_theme::builtins::by_name(name).unwrap());
            let ui = Ui::with(true, theme.clone());
            assert_eq!(painted_rgb(&ui, Ui::cream), theme.palette.text, "{name}");
            assert_eq!(painted_rgb(&ui, Ui::title), theme.palette.title, "{name}");
            assert_eq!(painted_rgb(&ui, Ui::link), theme.palette.link, "{name}");
        }
    }

    #[test]
    fn adapt_keeps_accent_hues_and_lifts_ink() {
        let paper = Color::new(247, 245, 238); // NYT newsprint
                                               // Press red is nudged just enough to read on dark glass, staying red.
        let red = adapt_for_cli(Color::new(150, 42, 36), paper);
        assert!(
            (red.r as u16) > (red.g as u16) * 3 / 2 && relative_luminance(red) >= 0.12,
            "red stays red but brighter: {red:?}"
        );
        // Near-black ink gets a warm-gray lift, not a different hue family.
        let lifted = adapt_for_cli(Color::new(26, 24, 22), paper);
        assert!(relative_luminance(lifted) >= 0.28);
        assert!(lifted.r >= lifted.g && lifted.g >= lifted.b);
        // Colors already past the bar pass through untouched.
        let ok = Color::new(140, 140, 140); // lum ≈ 0.27
        assert_eq!(adapt_for_cli(ok, paper), ok);
    }

    #[test]
    fn adapt_leaves_dark_paper_themes_alone() {
        // A dark paper can't serve as a lift target: mixing toward it would
        // paint the color as the background itself (t clamps to 1.0).
        let paper = Color::new(48, 48, 48); // lum ≈ 0.03, below the lift floor
        let ink = Color::new(16, 16, 16);
        assert_eq!(adapt_for_cli(ink, paper), ink);
        let accent = Color::new(40, 20, 20);
        assert_eq!(adapt_for_cli(accent, paper), accent);
    }
}
