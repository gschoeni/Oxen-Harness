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

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use harness_theme::{Color, Theme};

/// An RGB color for 24-bit ANSI.
type Rgb = (u8, u8, u8);

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

fn paint(text: &str, rgb: Rgb) -> String {
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

// ===========================================================================
// Pseudo-random selection (no `rand` dependency — a tiny time-seeded xorshift).
// ===========================================================================

/// Today's date formatted like the journal's flavor ("March 21, 1848"), but for
/// the present day. Derived from `SystemTime` (UTC) with a civil-date conversion
/// so we avoid pulling in a date/time dependency.
fn today() -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{} {}, {}", MONTHS[(month - 1) as usize], day, year)
}

/// Convert a count of days since the Unix epoch (1970-01-01) into a
/// (year, month, day) civil date. Algorithm from Howard Hinnant's `chrono`
/// date library (`civil_from_days`), valid across the Gregorian calendar.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A random weather reading for the trail journal — Oregon Trail flavor, picked
/// fresh each run from the same time-seeded PRNG used for the death screen.
fn weather() -> &'static str {
    const CONDITIONS: [&str; 10] = [
        "warm", "hot", "cool", "cold", "freezing", "rainy", "snowy", "foggy", "windy", "clear",
    ];
    let mut s = seed();
    CONDITIONS[(xorshift(&mut s) as usize) % CONDITIONS.len()]
}

fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
        | 1
}

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn pick(pool: &[String]) -> &str {
    if pool.is_empty() {
        return "";
    }
    let mut s = seed();
    let idx = (xorshift(&mut s) as usize) % pool.len();
    &pool[idx]
}

/// A tombstone "game over" screen shown when the user ends the session — a
/// random cause of death from the theme, engraved alongside the resume command
/// so the pioneer can pick the trail back up where they left off.
pub fn death_screen(ui: &Ui, session: &str) -> String {
    let v = &ui.theme.voice;
    let cause = pick(&v.deaths);

    let mut out = String::from("\n");
    for line in &v.exit_art {
        out.push_str(&format!("  {}\n", ui.dim(line)));
    }
    if !v.exit_ground.is_empty() {
        out.push_str(&format!("  {}\n", ui.green(&v.exit_ground)));
    }
    out.push('\n');
    out.push_str(&format!("  {}\n", ui.red(cause)));
    out.push('\n');
    out.push_str(&format!("  {}\n", ui.dim(&v.resume_message)));
    out.push_str(&format!(
        "    {}\n",
        ui.accent(&format!("oxen-harness --resume {session}"))
    ));
    out
}

// ===========================================================================
// Banner / wordmark
// ===========================================================================

/// Render the word as 5-row block "figlet" letters (only the glyphs we need).
fn wordmark(word: &str) -> Vec<String> {
    let mut rows = vec![String::new(); 5];
    for (i, ch) in word.chars().enumerate() {
        let glyph = glyph(ch);
        if i > 0 {
            for row in rows.iter_mut() {
                row.push(' ');
            }
        }
        for (r, line) in glyph.iter().enumerate() {
            rows[r].push_str(line);
        }
    }
    rows
}

/// 6-wide, 5-tall block glyphs for A–Z (others render blank).
fn glyph(ch: char) -> [&'static str; 5] {
    match ch.to_ascii_uppercase() {
        'A' => [" ████ ", "██  ██", "██████", "██  ██", "██  ██"],
        'B' => ["█████ ", "██  ██", "█████ ", "██  ██", "█████ "],
        'C' => [" █████", "██    ", "██    ", "██    ", " █████"],
        'D' => ["█████ ", "██  ██", "██  ██", "██  ██", "█████ "],
        'E' => ["██████", "██    ", "█████ ", "██    ", "██████"],
        'F' => ["██████", "██    ", "█████ ", "██    ", "██    "],
        'G' => [" █████", "██    ", "██ ███", "██  ██", " █████"],
        'H' => ["██  ██", "██  ██", "██████", "██  ██", "██  ██"],
        'I' => ["██████", "  ██  ", "  ██  ", "  ██  ", "██████"],
        'J' => ["██████", "   ██ ", "   ██ ", "██ ██ ", " ███  "],
        'K' => ["██  ██", "██ ██ ", "████  ", "██ ██ ", "██  ██"],
        'L' => ["██    ", "██    ", "██    ", "██    ", "██████"],
        'M' => ["██  ██", "██████", "██████", "██  ██", "██  ██"],
        'N' => ["██  ██", "███ ██", "██████", "██ ███", "██  ██"],
        'O' => ["██████", "██  ██", "██  ██", "██  ██", "██████"],
        'P' => ["█████ ", "██  ██", "█████ ", "██    ", "██    "],
        'Q' => [" ████ ", "██  ██", "██  ██", "██ ██ ", " ██ ██"],
        'R' => ["█████ ", "██  ██", "█████ ", "██ ██ ", "██  ██"],
        'S' => [" █████", "██    ", " ████ ", "    ██", "█████ "],
        'T' => ["██████", "  ██  ", "  ██  ", "  ██  ", "  ██  "],
        'U' => ["██  ██", "██  ██", "██  ██", "██  ██", "██████"],
        'V' => ["██  ██", "██  ██", "██  ██", " ████ ", "  ██  "],
        'W' => ["██  ██", "██  ██", "██████", "██████", "██  ██"],
        'X' => ["██  ██", " ████ ", "  ██  ", " ████ ", "██  ██"],
        'Y' => ["██  ██", " ████ ", "  ██  ", "  ██  ", "  ██  "],
        'Z' => ["██████", "   ██ ", "  ██  ", " ██   ", "██████"],
        _ => ["      ", "      ", "      ", "      ", "      "],
    }
}

/// Build the full startup banner from the active theme.
///
/// `tokens_used` is the cumulative token count for the live session; it
/// replaces the value of any `flavor_bottom` row labelled "Total tokens used"
/// so the banner reflects real usage rather than static flavor text.
pub fn banner(
    ui: &Ui,
    base_url: &str,
    model: &str,
    workspace: &str,
    session: &str,
    tokens_used: usize,
) -> String {
    let v = &ui.theme.voice;
    let mut out = String::new();
    out.push('\n');

    // ASCII scene: body in text color, trailing ground (`^~`-style) in primary.
    for line in &v.banner_art {
        let (body, ground) = split_ground(line);
        out.push_str(&ui.cream(body));
        out.push_str(&ui.green(ground));
        out.push('\n');
    }
    out.push('\n');

    if !v.pre_tagline.is_empty() {
        out.push_str(&format!("    {}\n", ui.brown(&v.pre_tagline)));
    }
    for row in wordmark(&v.wordmark) {
        out.push_str(&format!("  {}\n", ui.title(&row)));
    }
    out.push_str(&format!("  {}\n", ui.dim(&v.subtitle)));

    out.push_str(&flourish(ui));
    out.push('\n');

    for [label, value] in &v.flavor_top {
        out.push_str(&journal_row(ui, label, value));
    }
    out.push_str(&journal_row(
        ui,
        &v.label_provider,
        &format!("Oxen.ai · {base_url}"),
    ));
    out.push_str(&journal_row(ui, &v.label_model, model));
    out.push_str(&journal_row(ui, &v.label_workspace, workspace));
    out.push_str(&journal_row(ui, &v.label_session, session));
    out.push_str(&journal_row(ui, "Theme", &ui.theme.meta.name));
    for [label, value] in &v.flavor_bottom {
        // A few rows carry live state, substituted for the static flavor value:
        // "Total tokens used" gets the real cumulative count, and "Date" gets
        // today's date so the trail journal opens on the present day.
        if label == "Total tokens used" {
            out.push_str(&journal_row(ui, label, &format!("{tokens_used} tokens")));
        } else if label == "Date" {
            out.push_str(&journal_row(ui, label, &today()));
        } else if label == "Weather" {
            out.push_str(&journal_row(ui, label, weather()));
        } else {
            out.push_str(&journal_row(ui, label, value));
        }
    }

    out.push('\n');
    out.push_str(&format!("  {}\n", ui.dim(&v.bottom_hint)));
    out
}

/// The scrollwork divider, echoing the game's title screen.
fn flourish(ui: &Ui) -> String {
    format!(
        "  {}\n",
        ui.brown("╾━━━━━━━━━━━━━━━━━━━━━◆━━━━━━━━━━━━━━━━━━━━━╼")
    )
}

fn journal_row(ui: &Ui, label: &str, value: &str) -> String {
    format!(
        "  {} {}\n",
        ui.brown(&format!("{label:>17} :")),
        ui.cream(value)
    )
}

/// Split an art line into its body and a trailing decorative-ground run.
fn split_ground(line: &str) -> (&str, &str) {
    match line.find('^') {
        Some(idx)
            if line[idx..]
                .chars()
                .all(|c| matches!(c, '^' | '~' | '`' | ',')) =>
        {
            line.split_at(idx)
        }
        _ => (line, ""),
    }
}

/// The themed `/help` menu.
pub fn help(ui: &Ui) -> String {
    let v = &ui.theme.voice;
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!("  {}\n\n", ui.title(&v.help_header)));
    for item in &v.help_items {
        out.push_str(&format!(
            "    {} {}  {}\n",
            ui.accent(&item.key),
            ui.cream(&format!("{:<22}", item.title)),
            ui.dim(&item.hint),
        ));
    }
    out.push_str(&format!("\n  {}\n", ui.brown(&v.help_footer)));
    out
}

// ===========================================================================
// Local models — list table + download progress bar.
// ===========================================================================

/// One row in the `models list` table (a catalog model + its local status).
pub struct ModelRow<'a> {
    pub id: &'a str,
    pub params: &'a str,
    /// Pre-formatted size (actual when installed, else the estimate).
    pub size: String,
    pub installed: bool,
    pub note: &'a str,
}

/// Render the local-model catalog as an aligned, themed table.
pub fn models_table(ui: &Ui, rows: &[ModelRow], total_disk: &str, dir: &str) -> String {
    let id_w = rows
        .iter()
        .map(|r| r.id.len())
        .chain(std::iter::once("Model".len()))
        .max()
        .unwrap_or(5);
    let par_w = rows
        .iter()
        .map(|r| r.params.len())
        .chain(std::iter::once("Params".len()))
        .max()
        .unwrap_or(6);
    let size_w = rows
        .iter()
        .map(|r| r.size.len())
        .chain(std::iter::once("Size".len()))
        .max()
        .unwrap_or(4);

    let mut out = String::from("\n");
    out.push_str(&format!(
        "  {}  {}  {}   {}\n",
        ui.title(&format!("{:<id_w$}", "Model")),
        ui.title(&format!("{:<par_w$}", "Params")),
        ui.title(&format!("{:>size_w$}", "Size")),
        ui.title("Status"),
    ));
    out.push_str(&flourish(ui));
    for r in rows {
        let status = if r.installed {
            ui.green("● on disk")
        } else {
            ui.dim("○ not yet")
        };
        out.push_str(&format!(
            "  {}  {}  {}   {}\n",
            ui.cream(&format!("{:<id_w$}", r.id)),
            ui.brown(&format!("{:<par_w$}", r.params)),
            ui.cream(&format!("{:>size_w$}", r.size)),
            status,
        ));
        out.push_str(&format!(
            "  {}\n",
            ui.dim(&format!("{:id_w$}   {}", "", r.note))
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "  {} {}\n",
        ui.brown(&ui.theme.voice.label_disk_used),
        ui.cream(total_disk),
    ));
    out.push_str(&format!(
        "  {} {}\n",
        ui.brown(&ui.theme.voice.label_models_dir),
        ui.dim(dir)
    ));
    out.push('\n');
    out.push_str(&format!(
        "  {}\n",
        ui.dim(
            "Pull one with  oxen-harness models pull <Model>   ·   ride it with  --local <Model>"
        ),
    ));
    out
}

/// A single-line, in-place download progress bar with theme flavor.
///
/// `fraction` is `None` when the total size is unknown (the bar shows `?%`).
/// Print it with a leading `\r`; finish with a newline once complete.
pub fn progress_bar(ui: &Ui, fraction: Option<f64>, detail: &str) -> String {
    const WIDTH: usize = 24;
    let frac = fraction.unwrap_or(0.0).clamp(0.0, 1.0);
    let filled = (frac * WIDTH as f64).round() as usize;
    let bar: String = (0..WIDTH)
        .map(|i| if i < filled { '▰' } else { '▱' })
        .collect();
    let pct = match fraction {
        Some(f) => format!("{:>3.0}%", (f * 100.0).clamp(0.0, 100.0)),
        None => "  ?%".to_string(),
    };
    format!(
        "  {} {}  {}  {}",
        ui.brown(&ui.theme.voice.progress_icon),
        ui.green(&bar),
        ui.accent(&pct),
        ui.dim(detail),
    )
}

// ===========================================================================
// Spinner — an in-place animated status line (no-op when color is disabled).
// ===========================================================================

/// An animated status spinner running on a background thread.
///
/// Mirrors Claude Code's approach: it updates a single line in place with ANSI
/// escapes (`\r` + clear-to-end-of-line) and hides the cursor while spinning,
/// so there is no flicker and no scrollback spam.
pub struct Spinner {
    inner: Option<Inner>,
}

struct Inner {
    stop: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

/// The colors + glyphs the spinner needs, captured so the background thread is
/// self-contained (doesn't borrow the `Ui`).
struct SpinnerStyle {
    glyphs: Vec<String>,
    glyph_rgb: Rgb,
    text_rgb: Rgb,
    dim_rgb: Rgb,
}

impl SpinnerStyle {
    /// Capture the colors + glyphs for a UI, or `None` when color/animation is
    /// disabled (piped output, `NO_COLOR`, `TERM=dumb`).
    fn for_ui(ui: &Ui) -> Option<SpinnerStyle> {
        if !ui.color {
            return None;
        }
        let pal = &ui.theme.palette;
        let mut glyphs = ui.theme.voice.spinner_glyphs.clone();
        if glyphs.is_empty() {
            glyphs.push("✶".to_string());
        }
        Some(SpinnerStyle {
            glyphs,
            glyph_rgb: pal.title.rgb(),
            text_rgb: pal.text.rgb(),
            dim_rgb: pal.muted.rgb(),
        })
    }
}

impl Spinner {
    /// Start spinning, cycling through `verbs`. Returns a no-op spinner when
    /// color/animation is disabled (e.g. piped output) or there's nothing to show.
    pub fn start(ui: &Ui, verbs: Vec<String>) -> Self {
        Self::start_with_target(ui, verbs, None)
    }

    /// Like [`Spinner::start`] but pins a `target` (file/command/query) after the
    /// verb, so a running tool shows *what* it's working on alongside the timer.
    pub fn start_with_target(ui: &Ui, verbs: Vec<String>, target: Option<String>) -> Self {
        let Some(style) = SpinnerStyle::for_ui(ui).filter(|_| !verbs.is_empty()) else {
            return Spinner { inner: None };
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle =
            thread::spawn(move || run_spinner(&stop_thread, &verbs, target.as_deref(), &style));
        Spinner {
            inner: Some(Inner { stop, handle }),
        }
    }

    /// Stop the spinner, clear its line, and restore the cursor.
    pub fn stop(self) {
        if let Some(inner) = self.inner {
            inner.stop.store(true, Ordering::Relaxed);
            let _ = inner.handle.join();
        }
    }
}

/// Build one rendered spinner frame: `glyph  verb… target  (elapsed)`, fully
/// painted. The `target` (a file/command/query) is shown dimmed after the verb
/// when present, so a running tool says *what* it's acting on.
fn spinner_frame(
    style: &SpinnerStyle,
    verbs: &[String],
    target: Option<&str>,
    start: Instant,
    frame: usize,
    verb_idx: usize,
) -> String {
    let glyph = &style.glyphs[frame % style.glyphs.len()];
    let verb = match target {
        Some(t) if !t.is_empty() => format!("{}… {}", verbs[verb_idx], t),
        _ => format!("{}…", verbs[verb_idx]),
    };
    format!(
        "{}  {}  {}",
        paint(glyph, style.glyph_rgb),
        paint(&verb, style.text_rgb),
        paint(&format!("({})", elapsed(start)), style.dim_rgb),
    )
}

fn run_spinner(stop: &AtomicBool, verbs: &[String], target: Option<&str>, style: &SpinnerStyle) {
    let mut out = io::stdout();
    let start = Instant::now();
    let mut s = seed();
    let mut verb_idx = (xorshift(&mut s) as usize) % verbs.len();
    let mut frame = 0usize;

    let _ = write!(out, "\x1b[?25l"); // hide cursor
    let _ = out.flush();

    while !stop.load(Ordering::Relaxed) {
        if frame > 0 && frame % 16 == 0 {
            verb_idx = (verb_idx + 1) % verbs.len();
        }
        let line = spinner_frame(style, verbs, target, start, frame, verb_idx);
        let _ = write!(out, "\r{line}\x1b[K");
        let _ = out.flush();
        frame += 1;
        thread::sleep(Duration::from_millis(110));
    }

    let _ = write!(out, "\r\x1b[K\x1b[?25h"); // clear line, show cursor
    let _ = out.flush();
}

/// A spinner driven a single frame at a time from an async loop, for the live
/// composer (where a background thread writing to stdout would fight the
/// composer for the cursor). It produces a status line on demand instead of
/// owning a thread; the caller decides when to draw and advance it.
pub(crate) struct LiveSpinner {
    style: SpinnerStyle,
    verbs: Vec<String>,
    /// An optional target (a file path, command, query…) shown after the verb so
    /// the indicator reads e.g. `Reading the trail guide… src/lib.rs (3s)`.
    target: Option<String>,
    start: Instant,
    frame: usize,
    verb_idx: usize,
}

impl LiveSpinner {
    /// A spinner cycling `verbs`, or `None` when there's nothing to animate.
    pub(crate) fn new(ui: &Ui, verbs: Vec<String>) -> Option<Self> {
        Self::with_target(ui, verbs, None)
    }

    /// Like [`LiveSpinner::new`] but pins a `target` (file/command/etc.) after the
    /// verb, so a running tool shows *what* it's working on alongside the timer.
    pub(crate) fn with_target(ui: &Ui, verbs: Vec<String>, target: Option<String>) -> Option<Self> {
        if verbs.is_empty() {
            return None;
        }
        let style = SpinnerStyle::for_ui(ui)?;
        let mut s = seed();
        let verb_idx = (xorshift(&mut s) as usize) % verbs.len();
        Some(LiveSpinner {
            style,
            verbs,
            target,
            start: Instant::now(),
            frame: 0,
            verb_idx,
        })
    }

    /// The current frame's status line (glyph + verb + target + elapsed), painted.
    pub(crate) fn line(&self) -> String {
        spinner_frame(
            &self.style,
            &self.verbs,
            self.target.as_deref(),
            self.start,
            self.frame,
            self.verb_idx,
        )
    }

    /// Advance one frame, rotating the verb on the same cadence as the thread.
    pub(crate) fn tick(&mut self) {
        self.frame += 1;
        if self.frame % 16 == 0 {
            self.verb_idx = (self.verb_idx + 1) % self.verbs.len();
        }
    }
}

/// Format an elapsed duration like `7s` or `1m07s`.
fn elapsed(start: Instant) -> String {
    let secs = start.elapsed().as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colored() -> Ui {
        Ui::with(true, Arc::new(Theme::default()))
    }

    #[test]
    fn wordmark_rows_are_aligned() {
        let rows = wordmark("OXEN TRAIL");
        assert_eq!(rows.len(), 5);
        let width = rows[0].chars().count();
        for row in &rows {
            assert_eq!(row.chars().count(), width, "rows must be equal width");
        }
        assert_eq!(width, 10 * 6 + 9);
    }

    #[test]
    fn no_color_paint_is_plain() {
        let ui = Ui::plain();
        assert_eq!(ui.title("hi"), "hi");
        assert_eq!(ui.dim("trail"), "trail");
        assert!(!help(&ui).contains("\x1b["));
        assert!(!banner(&ui, "u", "m", "w", "s", 0).contains("\x1b["));
        assert!(!death_screen(&ui, "abc123").contains("\x1b["));
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1)); // Unix epoch
        assert_eq!(civil_from_days(18_993), (2022, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1)); // non-leap year
        assert_eq!(civil_from_days(-719_162), (1, 1, 1));
    }

    #[test]
    fn banner_shows_a_live_date_not_the_static_flavor() {
        let ui = colored();
        let out = banner(&ui, "u", "m", "w", "s", 0);
        // The static flavor year (1848) must be replaced by today's real date.
        assert!(out.contains(&today()));
        assert!(!out.contains("March 21, 1848"));
    }

    #[test]
    fn weather_is_one_of_the_known_conditions() {
        const CONDITIONS: [&str; 10] = [
            "warm", "hot", "cool", "cold", "freezing", "rainy", "snowy", "foggy", "windy", "clear",
        ];
        assert!(CONDITIONS.contains(&weather()));
    }

    #[test]
    fn set_departing_updates_first_flavor_row_and_banner() {
        let mut ui = Ui::plain();
        // The default Oregon Trail theme ships a "Departing" flavor row.
        let (label, _) = ui.departing().expect("default theme has a flavor row");
        assert_eq!(label, "Departing");

        let returned = ui.set_departing("Fort Laramie, Wyoming");
        assert_eq!(returned, "Departing");
        assert_eq!(ui.departing(), Some(("Departing", "Fort Laramie, Wyoming")));
        // The banner reflects the new location.
        assert!(banner(&ui, "u", "m", "w", "s", 0).contains("Fort Laramie, Wyoming"));
    }

    #[test]
    fn banner_shows_live_token_count() {
        let ui = Ui::plain();
        let b = banner(&ui, "u", "m", "w", "s", 1234);
        // The live cumulative count replaces the static flavor value.
        assert!(b.contains("Total tokens used"));
        assert!(b.contains("1234 tokens"));
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
    fn elapsed_formats_minutes() {
        assert_eq!(elapsed(Instant::now() - Duration::from_secs(7)), "7s");
        assert_eq!(elapsed(Instant::now() - Duration::from_secs(67)), "1m07s");
    }

    #[test]
    fn death_screen_has_a_real_cause_and_resume_hint() {
        let ui = Ui::plain();
        let screen = death_screen(&ui, "sess-42");
        assert!(Theme::default()
            .voice
            .deaths
            .iter()
            .any(|d| screen.contains(d)));
        assert!(screen.contains("oxen-harness --resume sess-42"));
    }

    #[test]
    fn banner_includes_active_theme_name() {
        let ui = Ui::plain();
        let b = banner(&ui, "host", "model", "ws", "sess", 0);
        assert!(b.contains("Oregon Trail"));
        assert!(b.contains("model"));
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
    fn progress_bar_tracks_fraction_and_handles_unknown() {
        let ui = Ui::plain();
        let half = progress_bar(&ui, Some(0.5), "2.5 GB / 5.0 GB");
        assert!(half.contains("50%"));
        assert!(half.contains("2.5 GB / 5.0 GB"));
        assert!(half.contains('▰') && half.contains('▱'));
        let unknown = progress_bar(&ui, None, "downloading");
        assert!(unknown.contains("?%"));
    }

    #[test]
    fn models_table_lists_rows_and_disk_usage() {
        let ui = Ui::plain();
        let rows = [
            ModelRow {
                id: "qwen3-8b",
                params: "8B",
                size: "5.0 GB".to_string(),
                installed: true,
                note: "all-rounder",
            },
            ModelRow {
                id: "qwen3-32b",
                params: "32B",
                size: "20 GB".to_string(),
                installed: false,
                note: "heaviest",
            },
        ];
        let table = models_table(&ui, &rows, "5.0 GB", "/home/me/.oxen-harness/models");
        assert!(table.contains("qwen3-8b"));
        assert!(table.contains("● on disk"));
        assert!(table.contains("○ not yet"));
        assert!(table.contains("5.0 GB"));
        assert!(!table.contains("\x1b["));
    }

    #[test]
    fn split_ground_separates_trailing_terrain() {
        let (body, ground) = split_ground("  |__|  ^^,~^^`");
        assert_eq!(body, "  |__|  ");
        assert_eq!(ground, "^^,~^^`");
        let (body, ground) = split_ground("no terrain here");
        assert_eq!(body, "no terrain here");
        assert_eq!(ground, "");
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
