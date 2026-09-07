//! Full-screen themed compositions: the startup banner (ASCII scene + block
//! wordmark + trail journal + recent trails), the `/help` menu, and the
//! tombstone exit screen.

use crate::almanac::{pick, today};

use super::{flourish, Ui};

/// A previous session for this workspace, as the banner and the `/resume`
/// picker both show it. Built by [`crate::commands::resume::recent_trails`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentTrail {
    /// The session id `/resume` takes (matched by prefix).
    pub id: String,
    /// The session's first user message.
    pub title: String,
    /// How long ago it started, already humanized ("2h ago").
    pub age: String,
    /// Journal entries (messages) on record.
    pub entries: i64,
}

/// The live figures the banner shows alongside the theme's static flavor.
/// Bundled so the banner keeps one argument per *concern* rather than a
/// growing positional list.
#[derive(Default)]
pub struct BannerFacts<'a> {
    /// Cumulative all-time token count; replaces the "Total tokens used" row.
    pub tokens_used: usize,
    /// Estimated all-time Oxen cloud spend across every model and project.
    /// `None` when pricing hasn't landed yet — rendered as "—".
    pub cost_usd: Option<f64>,
    /// Today's reading for the "Weather" row; `None` renders "—", so a banner
    /// never waits on a reading that isn't ready.
    pub weather: Option<&'a str>,
    /// The last few sessions for this workspace, newest first. Empty hides the
    /// "Recent trails" block entirely.
    pub recent: &'a [RecentTrail],
}

/// One recent-session row: `2h ago · "fix the flaky watch test" · 41 entries`.
/// The banner and the `/resume` picker share it so the two can't drift.
pub fn trail_summary(trail: &RecentTrail) -> String {
    let title = crate::render::truncate(trail.title.trim(), 56);
    let unit = if trail.entries == 1 {
        "entry"
    } else {
        "entries"
    };
    format!("{} · \"{title}\" · {} {unit}", trail.age, trail.entries)
}

/// A coarse "how long ago" label for `secs` seconds in the past, from "just
/// now" up to years. Deliberately one unit wide — the banner wants a glanceable
/// age, not a duration.
pub fn relative_age(secs: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    match secs.max(0) {
        s if s < MINUTE => "just now".to_string(),
        s if s < HOUR => format!("{}m ago", s / MINUTE),
        s if s < DAY => format!("{}h ago", s / HOUR),
        s if s < MONTH => format!("{}d ago", s / DAY),
        s if s < YEAR => format!("{}mo ago", s / MONTH),
        s => format!("{}y ago", s / YEAR),
    }
}

/// Build the full startup banner from the active theme.
///
/// The live figures — token count, spend, weather, and the recent trails for
/// this workspace — come in as [`BannerFacts`]; everything else is theme voice.
pub fn banner(
    ui: &Ui,
    base_url: &str,
    model: &str,
    workspace: &str,
    session: &str,
    facts: &BannerFacts<'_>,
) -> String {
    let v = &ui.theme().voice;
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
    out.push_str(&journal_row(ui, "Theme", &ui.theme().meta.name));
    let mut spend_rendered = false;
    for [label, value] in &v.flavor_bottom {
        // A few rows carry live state, substituted for the static flavor value.
        // The live token count is emitted below. Landmark/spend slots are
        // replaced in place; a fallback spend row is added below for custom
        // themes without either slot. "Date" always opens on today.
        if label == "Next landmark" || label == "Total dollars spent" {
            let spent = facts.cost_usd.map(format_usd).unwrap_or_else(|| "—".into());
            out.push_str(&journal_row(ui, "Total dollars spent", &spent));
            spend_rendered = true;
        } else if label == "Total tokens used" {
            // Rendered live after this loop — skip the static flavor copy.
        } else if label == "Date" {
            out.push_str(&journal_row(ui, label, &today()));
        } else if label == "Weather" {
            // A reading that isn't in hand yet is "—": the banner never blocks
            // on one.
            out.push_str(&journal_row(ui, label, facts.weather.unwrap_or("—")));
        } else {
            out.push_str(&journal_row(ui, label, value));
        }
    }

    // Always show the live all-time token count. Custom themes without the
    // standard landmark/spend slot still get the estimated cloud-spend row.
    out.push_str(&journal_row(
        ui,
        "Total tokens used",
        &format!("{} tokens", facts.tokens_used),
    ));
    if !spend_rendered {
        let spent = facts.cost_usd.map(format_usd).unwrap_or_else(|| "—".into());
        out.push_str(&journal_row(ui, "Total dollars spent", &spent));
    }

    // The trails already blazed in this workspace — one keystroke from being
    // picked back up. Nothing to show on a first visit, so the block is
    // omitted entirely rather than printing an empty heading.
    if !facts.recent.is_empty() {
        out.push('\n');
        out.push_str(&format!("  {}\n", ui.brown("Recent trails")));
        for trail in facts.recent {
            out.push_str(&format!(
                "  {} {}\n",
                ui.green("↺"),
                ui.cream(&trail_summary(trail)),
            ));
        }
    }

    out.push('\n');
    out.push_str(&format!(
        "  {} {}\n",
        ui.dim(&v.bottom_hint),
        ui.dim("· /resume to pick up a trail"),
    ));
    out
}

/// Format a US-dollar amount for the banner's spend readout. Sub-cent totals
/// show extra precision (e.g. `$0.0042`) so early usage isn't shown as `$0.00`;
/// larger amounts use standard two-decimal currency (mirrors the desktop UI).
pub(crate) fn format_usd(amount: f64) -> String {
    if amount > 0.0 && amount < 0.01 {
        format!("${amount:.4}")
    } else {
        format!("${amount:.2}")
    }
}

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

fn journal_row(ui: &Ui, label: &str, value: &str) -> String {
    // Right-align labels in a column wide enough for the longest one
    // ("Total dollars spent", 19 chars) so every colon lines up.
    format!(
        "  {} {}\n",
        ui.brown(&format!("{label:>19} :")),
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
    let v = &ui.theme().voice;
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
    out.push_str(&format!("\n  {}\n", ui.title("Keys on the trail")));
    for (key, what) in KEY_HELP {
        out.push_str(&format!(
            "    {}  {}\n",
            ui.accent(&format!("{key:<12}")),
            ui.dim(what)
        ));
    }
    out.push_str(&format!("\n  {}\n", ui.brown(&v.help_footer)));
    out
}

/// The composer's keys and the trail-keeping commands, the same in every
/// theme (a theme changes the voice, not the controls).
const KEY_HELP: &[(&str, &str)] = &[
    ("Enter", "send · mid-turn: steer the running turn"),
    (
        "Ctrl+Q",
        "queue as a follow-up (runs after this turn) · Alt+↑ pulls it back",
    ),
    (
        "Esc",
        "cancel the running turn · Esc Esc at idle: rewind picker",
    ),
    ("Ctrl+O", "expand the last tool result in full"),
    ("Ctrl+G", "edit the draft in $EDITOR"),
    ("@path", "complete a workspace path to mention it"),
    ("Tab", "complete a /command or its argument"),
    (
        "/resume",
        "pick up an earlier trail · /fork copies · /rewind goes back",
    ),
    (
        "/plan",
        "read-only planning; /plan approve executes the plan file",
    ),
    ("Ctrl-C", "clear the draft, then confirm, then exit"),
];

/// A tombstone "game over" screen shown when the user ends the session — a
/// random cause of death from the theme, engraved alongside the resume command
/// so the pioneer can pick the trail back up where they left off.
pub fn death_screen(ui: &Ui, session: &str) -> String {
    let v = &ui.theme().voice;
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

#[cfg(test)]
mod tests {
    use super::*;
    use harness_theme::Theme;
    use std::sync::Arc;

    fn colored() -> Ui {
        Ui::with(true, Arc::new(Theme::default()))
    }

    /// Banner facts carrying only the live figures a test cares about.
    fn facts<'a>(tokens_used: usize, cost_usd: Option<f64>) -> BannerFacts<'a> {
        BannerFacts {
            tokens_used,
            cost_usd,
            ..BannerFacts::default()
        }
    }

    fn trail(id: &str, title: &str, age: &str, entries: i64) -> RecentTrail {
        RecentTrail {
            id: id.into(),
            title: title.into(),
            age: age.into(),
            entries,
        }
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
    fn no_color_screens_are_plain() {
        let ui = Ui::plain();
        assert!(!help(&ui).contains("\x1b["));
        assert!(!banner(&ui, "u", "m", "w", "s", &BannerFacts::default()).contains("\x1b["));
        assert!(!death_screen(&ui, "abc123").contains("\x1b["));
    }

    #[test]
    fn banner_shows_a_live_date_not_the_static_flavor() {
        let ui = colored();
        let out = banner(&ui, "u", "m", "w", "s", &BannerFacts::default());
        // The static flavor year (1848) must be replaced by today's real date.
        assert!(out.contains(&today()));
        assert!(!out.contains("March 21, 1848"));
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
        assert!(banner(&ui, "u", "m", "w", "s", &BannerFacts::default())
            .contains("Fort Laramie, Wyoming"));
    }

    #[test]
    fn banner_shows_live_token_count() {
        let ui = Ui::plain();
        let b = banner(&ui, "u", "m", "w", "s", &facts(1234, None));
        // The live cumulative count replaces the static flavor value.
        assert!(b.contains("Total tokens used"));
        assert!(b.contains("1234 tokens"));
    }

    #[test]
    fn banner_shows_token_and_dollar_rows_even_without_theme_flavor() {
        // A theme loaded from disk may not carry "Total tokens used" /
        // "Total dollars spent" flavor rows; the banner must still show both.
        let mut theme = Theme::default();
        theme.voice.flavor_bottom.clear();
        let ui = Ui::with(false, Arc::new(theme));
        let b = banner(&ui, "u", "m", "w", "s", &facts(555, Some(1.25)));
        assert!(b.contains("Total tokens used"));
        assert!(b.contains("555 tokens"));
        assert!(b.contains("Total dollars spent"));
        assert!(b.contains("$1.25"));
        // No duplicate rows.
        assert_eq!(b.matches("Total tokens used").count(), 1);
        assert_eq!(b.matches("Total dollars spent").count(), 1);
    }

    #[test]
    fn banner_shows_dollars_spent() {
        let ui = Ui::plain();
        // A known cost renders as a currency row; unavailable renders as "—".
        let priced = banner(&ui, "u", "m", "w", "s", &facts(1234, Some(0.42)));
        assert!(priced.contains("Total dollars spent"));
        assert!(priced.contains("$0.42"));
        let unavailable = banner(&ui, "u", "m", "w", "s", &BannerFacts::default());
        assert!(unavailable.contains("Total dollars spent"));
    }

    #[test]
    fn banner_replaces_next_landmark_with_dollars_spent() {
        let b = banner(&Ui::plain(), "u", "m", "w", "s", &facts(1234, Some(0.42)));
        assert!(!b.contains("Next landmark"));
        assert_eq!(b.matches("Total dollars spent").count(), 1);
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
        let b = banner(&ui, "host", "model", "ws", "sess", &BannerFacts::default());
        assert!(b.contains("Oregon Trail"));
        assert!(b.contains("model"));
    }

    #[test]
    fn banner_renders_without_a_weather_reading() {
        // Time-to-first-prompt never waits on a reading: the banner takes one
        // (from a theme with a Weather row) and prints "—" when it has none.
        let mut theme = Theme::default();
        theme.voice.flavor_bottom = vec![["Weather".to_string(), "Fair".to_string()]];
        let ui = Ui::with(false, Arc::new(theme));

        let cold = banner(&ui, "u", "m", "w", "s", &BannerFacts::default());
        let row = cold
            .lines()
            .find(|l| l.contains("Weather"))
            .expect("the theme has a Weather row");
        assert!(row.contains('—'), "absent weather renders a dash: {row}");
        assert!(!row.contains("Fair"), "static flavor is replaced: {row}");

        let warm = banner(
            &ui,
            "u",
            "m",
            "w",
            "s",
            &BannerFacts {
                weather: Some("blizzard"),
                ..BannerFacts::default()
            },
        );
        assert!(warm.contains("blizzard"), "{warm}");
    }

    #[test]
    fn banner_lists_recent_trails_and_hints_resume() {
        let ui = Ui::plain();
        let recent = [
            trail("abc12345", "fix the flaky watch test", "2h ago", 41),
            trail("def67890", "port the picker", "3d ago", 1),
        ];
        let b = banner(
            &ui,
            "u",
            "m",
            "w",
            "s",
            &BannerFacts {
                recent: &recent,
                ..BannerFacts::default()
            },
        );
        assert!(b.contains("Recent trails"), "{b}");
        assert!(
            b.contains("↺ 2h ago · \"fix the flaky watch test\" · 41 entries"),
            "{b}"
        );
        // A one-message trail reads "1 entry", not "1 entries".
        assert!(b.contains("· 1 entry"), "{b}");
        assert!(b.contains("/resume"), "the hint mentions /resume: {b}");

        // No history for this workspace: no heading, no empty block.
        let empty = banner(&ui, "u", "m", "w", "s", &BannerFacts::default());
        assert!(!empty.contains("Recent trails"), "{empty}");
    }

    #[test]
    fn relative_age_picks_one_coarse_unit() {
        assert_eq!(relative_age(0), "just now");
        assert_eq!(relative_age(-5), "just now");
        assert_eq!(relative_age(59), "just now");
        assert_eq!(relative_age(60), "1m ago");
        assert_eq!(relative_age(90 * 60), "1h ago");
        assert_eq!(relative_age(2 * 3600), "2h ago");
        assert_eq!(relative_age(3 * 86_400), "3d ago");
        assert_eq!(relative_age(45 * 86_400), "1mo ago");
        assert_eq!(relative_age(400 * 86_400), "1y ago");
    }

    #[test]
    fn trail_summary_clips_a_long_title() {
        let long = trail("id", &"x".repeat(200), "5m ago", 7);
        let row = trail_summary(&long);
        assert!(row.starts_with("5m ago · \""), "{row}");
        assert!(row.contains('…'), "long titles are clipped: {row}");
        assert!(row.ends_with("· 7 entries"), "{row}");
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
