//! Visual theme for the `oxen-harness` REPL.
//!
//! The look borrows the *structure* of modern coding CLIs (Claude Code style:
//! a welcome box, in-place spinners with status verbs, transparent tool lines)
//! and the *voice* of the 1980s **Oregon Trail** game — because Oxen pull the
//! wagons on the trail, and Oxen.ai powers this one. All the silly trail
//! phrases live here so the rest of the CLI stays readable.
//!
//! Color is emitted as 24-bit ("truecolor") ANSI and is disabled automatically
//! when stdout is not a TTY, when `NO_COLOR` is set, or for `TERM=dumb`, so
//! piped output stays clean.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// An RGB color for 24-bit ANSI.
type Rgb = (u8, u8, u8);

// The Oregon-Trail-on-a-CRT palette: tan title text, saddle brown flourishes,
// prairie green, parchment cream, faded trail dust, and tombstone red.
const TAN: Rgb = (240, 190, 140);
const BROWN: Rgb = (170, 110, 60);
const GREEN: Rgb = (96, 176, 96);
const CREAM: Rgb = (236, 226, 206);
const DIM: Rgb = (150, 140, 125);
const RED: Rgb = (205, 84, 72);
const SKY: Rgb = (120, 178, 214);

/// Whether and how to render color. Cheap to copy; pass it around freely.
#[derive(Clone, Copy, Debug)]
pub struct Ui {
    color: bool,
}

impl Ui {
    /// A color-disabled UI (used for non-TTY output and in tests).
    #[cfg(test)]
    pub fn plain() -> Self {
        Ui { color: false }
    }

    /// Detect terminal capabilities for stdout.
    pub fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let dumb = std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);
        Ui {
            color: io::stdout().is_terminal() && !no_color && !dumb,
        }
    }

    fn paint(&self, text: &str, rgb: Rgb) -> String {
        if self.color {
            paint(text, rgb)
        } else {
            text.to_string()
        }
    }

    fn bold(&self, text: &str, rgb: Rgb) -> String {
        if self.color {
            let (r, g, b) = rgb;
            format!("\x1b[1;38;2;{r};{g};{b}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    // Semantic colors used across the CLI.
    pub fn title(&self, s: &str) -> String {
        self.bold(s, TAN)
    }
    pub fn brown(&self, s: &str) -> String {
        self.paint(s, BROWN)
    }
    pub fn green(&self, s: &str) -> String {
        self.paint(s, GREEN)
    }
    pub fn cream(&self, s: &str) -> String {
        self.paint(s, CREAM)
    }
    pub fn dim(&self, s: &str) -> String {
        self.paint(s, DIM)
    }
    pub fn red(&self, s: &str) -> String {
        self.paint(s, RED)
    }
    pub fn accent(&self, s: &str) -> String {
        self.bold(s, GREEN)
    }

    // Inline markdown styles.
    pub fn strong(&self, s: &str) -> String {
        self.bold(s, CREAM)
    }
    pub fn em(&self, s: &str) -> String {
        if self.color {
            let (r, g, b) = CREAM;
            format!("\x1b[3;38;2;{r};{g};{b}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn code(&self, s: &str) -> String {
        self.paint(s, GREEN)
    }
    pub fn link(&self, s: &str) -> String {
        if self.color {
            let (r, g, b) = SKY;
            format!("\x1b[4;38;2;{r};{g};{b}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}

fn paint(text: &str, rgb: Rgb) -> String {
    let (r, g, b) = rgb;
    format!("\x1b[38;2;{r};{g};{b}m{text}\x1b[0m")
}

/// The REPL input prompt (mirrors the game's "What is your choice?").
pub fn prompt(ui: &Ui) -> String {
    format!("{} {} ", ui.brown("🐂"), ui.accent("trail ❯"))
}

// ===========================================================================
// Trail phrases — the whole point. Gerund verbs animate next to the spinner.
// ===========================================================================

/// Phrases shown while the model is thinking.
pub const THINKING: &[&str] = &[
    "Sizing up the situation",
    "Consulting the trail guide",
    "Fording the river",
    "Caulking the wagon to float across",
    "Yoking the oxen",
    "Scouting the trail ahead",
    "Greasing the wagon axles",
    "Rationing the supplies",
    "Checking the wagon for snakes",
    "Reading the worn trail map",
    "Asking a fellow traveler for directions",
    "Resting the weary oxen",
    "Setting a steady pace",
    "Trading pelts at the fort",
    "Looking for a shallow place to ford",
    "Counting the remaining bullets",
    "Pressing onward to Oregon",
    "Watching for buffalo",
    "Waiting for the river to drop",
];

/// Pick the gerund-verb pool that fits a tool, so the spinner stays on-theme.
pub fn tool_verbs(tool: &str) -> &'static [&'static str] {
    match tool {
        "read_file" => &["Reading the trail guide", "Studying the worn map"],
        "write_file" => &["Writing in the journal", "Etching a new tombstone"],
        "edit_file" => &["Mending the wagon", "Patching the wagon canvas"],
        "find_files" => &["Scouting for landmarks", "Surveying the trail"],
        "search_files" => &["Hunting through the brush", "Tracking through the prairie"],
        "run_shell" => &["Yoking the oxen", "Setting the wagon in motion"],
        "git" => &["Caulking the wagon", "Fording the river"],
        _ => &["Working the trail"],
    }
}

/// The authentic Oregon Trail ways to die (1985 set + a few from other
/// versions: diphtheria, pneumonia, blizzards, starvation, gunshots).
const DEATHS: &[&str] = &[
    "You have died of dysentery.",
    "You have died of cholera.",
    "You have died of typhoid fever.",
    "You have died of measles.",
    "You have died of diphtheria.",
    "You have died of a fever.",
    "You have died of exhaustion.",
    "You have died of a snakebite.",
    "You have died of pneumonia.",
    "You have died of a broken leg.",
    "You have died of a broken arm.",
    "You have drowned while fording the river.",
    "You have starved to death.",
    "You have frozen to death in a blizzard.",
    "You were accidentally shot while hunting.",
    "Your oxen wandered off and you were stranded.",
    "Thieves raided your wagon in the night.",
    "A wagon wheel broke and you were left on the trail.",
];

// ===========================================================================
// Pseudo-random selection (no `rand` dependency — a tiny time-seeded xorshift).
// ===========================================================================

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

fn pick(pool: &[&'static str]) -> &'static str {
    if pool.is_empty() {
        return "";
    }
    let mut s = seed();
    let idx = (xorshift(&mut s) as usize) % pool.len();
    pool[idx]
}

/// A flavorful "you died" line for a real agent error.
pub fn death() -> &'static str {
    pick(DEATHS)
}

/// A tombstone "game over" screen shown when the user ends the session — a
/// random, authentic Oregon Trail cause of death engraved on a gravestone.
///
/// The session id is engraved alongside the resume command so the pioneer can
/// pick the trail back up where they left off.
pub fn death_screen(ui: &Ui, session: &str) -> String {
    let cause = pick(DEATHS);
    let stone = [
        r"        _______________        ",
        r"      .'               '.      ",
        r"     /                   \     ",
        r"    /       R. I. P.       \    ",
        r"    |                      |    ",
        r"    |      here lies a     |    ",
        r"    |    weary  pioneer    |    ",
        r"    |                      |    ",
        r"    |                      |    ",
    ];

    let mut out = String::from("\n");
    for line in stone {
        out.push_str(&format!("  {}\n", ui.dim(line)));
    }
    // The grassy mound the stone sits in.
    out.push_str(&format!(
        "  {}\n",
        ui.green("^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`")
    ));
    out.push('\n');
    out.push_str(&format!("  {}\n", ui.red(cause)));
    out.push('\n');
    out.push_str(&format!(
        "  {}\n",
        ui.dim("Your trail journal was saved. Resume this expedition with:")
    ));
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

/// 6-wide, 5-tall block glyphs for the letters in "OXEN TRAIL".
fn glyph(ch: char) -> [&'static str; 5] {
    match ch.to_ascii_uppercase() {
        'O' => ["██████", "██  ██", "██  ██", "██  ██", "██████"],
        'X' => ["██  ██", " ████ ", "  ██  ", " ████ ", "██  ██"],
        'E' => ["██████", "██    ", "█████ ", "██    ", "██████"],
        'N' => ["██  ██", "███ ██", "██████", "██ ███", "██  ██"],
        'T' => ["██████", "  ██  ", "  ██  ", "  ██  ", "  ██  "],
        'R' => ["█████ ", "██  ██", "█████ ", "██ ██ ", "██  ██"],
        'A' => [" ████ ", "██  ██", "██████", "██  ██", "██  ██"],
        'I' => ["██████", "  ██  ", "  ██  ", "  ██  ", "██████"],
        'L' => ["██    ", "██    ", "██    ", "██    ", "██████"],
        _ => ["      ", "      ", "      ", "      ", "      "],
    }
}

/// The covered-wagon scene printed above the wordmark.
const WAGON: &str = r#"                  _______________
                ,'               '.___
   ____________,'    Oxen.ai        '.__
  |  ~   ~   ~  |~   ~   ~   ~   ~    |  '.
  |_____________|____________________|____\
        (O)                    (O)
^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`"#;

/// Build the full startup banner.
pub fn banner(ui: &Ui, base_url: &str, model: &str, workspace: &str, session: &str) -> String {
    let mut out = String::new();
    out.push('\n');

    // Wagon scene: cream wagon riding a green prairie horizon.
    for line in WAGON.lines() {
        let (wagon, ground) = split_ground(line);
        out.push_str(&ui.cream(wagon));
        out.push_str(&ui.green(ground));
        out.push('\n');
    }
    out.push('\n');

    // "THE OXEN TRAIL" wordmark.
    out.push_str(&format!("    {}\n", ui.brown("～ The ～")));
    for row in wordmark("OXEN TRAIL") {
        out.push_str(&format!("  {}\n", ui.title(&row)));
    }
    out.push_str(&format!(
        "  {}\n",
        ui.dim("an open source agentic coding trail · powered by Oxen.ai")
    ));

    out.push_str(&flourish(ui));
    out.push('\n');

    // The "size up the situation" trail journal.
    out.push_str(&journal_row(
        ui,
        "Departing",
        "Independence, Missouri · 1848",
    ));
    out.push_str(&journal_row(
        ui,
        "Provider",
        &format!("Oxen.ai · {base_url}"),
    ));
    out.push_str(&journal_row(ui, "Oxen (model)", model));
    out.push_str(&journal_row(ui, "Wagon (workspace)", workspace));
    out.push_str(&journal_row(ui, "Trail journal", session));
    out.push_str(&journal_row(
        ui,
        "Weather",
        "fair  ·  Health: good  ·  Pace: steady",
    ));

    out.push('\n');
    out.push_str(&format!(
        "  {}\n",
        ui.dim("Type /help to size up your options · Ctrl-D to make camp")
    ));
    out
}

/// The brown scrollwork divider, echoing the game's title screen.
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

/// Split a wagon-art line into its body and the trailing ground (`^`/`~`) run.
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

/// The themed `/help` menu, modeled on the Oregon Trail main menu.
pub fn help(ui: &Ui) -> String {
    let item = |n: &str, title: &str, hint: &str| {
        format!(
            "    {} {}  {}\n",
            ui.accent(n),
            ui.cream(title),
            ui.dim(hint)
        )
    };
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!("  {}\n\n", ui.title("You may:")));
    out.push_str(&item(
        "1.",
        "Travel the trail   ",
        "— just type what you want done",
    ));
    out.push_str(&item("2.", "Learn about the trail", "— /help"));
    out.push_str(&item(
        "3.",
        "See the Oregon Top Ten",
        "— /export [path]  (save the journey as JSONL)",
    ));
    out.push_str(&item("4.", "Trade your oxen     ", "— /model [name]"));
    out.push_str(&item("5.", "Make camp / End     ", "— /exit  (or Ctrl-D)"));
    out.push_str(&format!("\n  {}\n", ui.brown("What is your choice?")));
    out
}

// ===========================================================================
// Spinner — an in-place animated status line (no-op when color is disabled).
// ===========================================================================

/// An animated trail-status spinner running on a background thread.
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

impl Spinner {
    /// Start spinning, cycling through `verbs`. Returns a no-op spinner when
    /// color/animation is disabled (e.g. piped output).
    pub fn start(ui: &Ui, verbs: &'static [&'static str]) -> Self {
        if !ui.color || verbs.is_empty() {
            return Spinner { inner: None };
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let handle = thread::spawn(move || run_spinner(&stop_thread, verbs));
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

fn run_spinner(stop: &AtomicBool, verbs: &[&str]) {
    // Asterisk dingbats in the spirit of Claude Code's bespoke glyphs.
    const GLYPHS: &[&str] = &["✶", "✸", "✺", "✹", "✷", "✦"];
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
        let glyph = GLYPHS[frame % GLYPHS.len()];
        let line = format!(
            "{}  {}  {}",
            paint(glyph, TAN),
            paint(&format!("{}…", verbs[verb_idx]), CREAM),
            paint(&format!("({})", elapsed(start)), DIM),
        );
        let _ = write!(out, "\r{line}\x1b[K");
        let _ = out.flush();
        frame += 1;
        thread::sleep(Duration::from_millis(110));
    }

    let _ = write!(out, "\r\x1b[K\x1b[?25h"); // clear line, show cursor
    let _ = out.flush();
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

    #[test]
    fn wordmark_rows_are_aligned() {
        let rows = wordmark("OXEN TRAIL");
        assert_eq!(rows.len(), 5);
        let width = rows[0].chars().count();
        for row in &rows {
            assert_eq!(row.chars().count(), width, "rows must be equal width");
        }
        // 10 glyphs (incl. one space) * 6 wide + 9 single-space gutters.
        assert_eq!(width, 10 * 6 + 9);
    }

    #[test]
    fn no_color_paint_is_plain() {
        let ui = Ui { color: false };
        assert_eq!(ui.title("hi"), "hi");
        assert_eq!(ui.dim("trail"), "trail");
        assert!(!help(&ui).contains("\x1b["));
        assert!(!banner(&ui, "u", "m", "w", "s").contains("\x1b["));
        assert!(!death_screen(&ui, "abc123").contains("\x1b["));
    }

    #[test]
    fn color_paint_wraps_in_ansi() {
        let ui = Ui { color: true };
        let s = ui.green("go");
        assert!(s.starts_with("\x1b[38;2;"));
        assert!(s.ends_with("\x1b[0m"));
        assert!(s.contains("go"));
    }

    #[test]
    fn elapsed_formats_minutes() {
        assert_eq!(
            elapsed(Instant::now() - Duration::from_secs(7)),
            "7s".to_string()
        );
        assert_eq!(
            elapsed(Instant::now() - Duration::from_secs(67)),
            "1m07s".to_string()
        );
    }

    #[test]
    fn death_screen_has_tombstone_and_a_real_cause() {
        let ui = Ui { color: false };
        let screen = death_screen(&ui, "sess-42");
        assert!(screen.contains("R. I. P."));
        assert!(DEATHS.iter().any(|d| screen.contains(d)));
        // The resume hint surfaces the session id so the trail can be picked up.
        assert!(screen.contains("oxen-harness --resume sess-42"));
    }

    #[test]
    fn tool_verbs_are_themed_and_nonempty() {
        for tool in [
            "read_file",
            "write_file",
            "edit_file",
            "find_files",
            "search_files",
            "run_shell",
            "git",
            "wat",
        ] {
            assert!(!tool_verbs(tool).is_empty());
        }
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
}
