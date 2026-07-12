//! Full-screen themed compositions: the startup banner (ASCII scene + block
//! wordmark + trail journal), the `/help` menu, and the tombstone exit screen.

use crate::almanac::{pick, today, weather};

use super::{flourish, Ui};

/// Build the full startup banner from the active theme.
///
/// `tokens_used` is the cumulative token count for the live session; it
/// replaces the value of any `flavor_bottom` row labelled "Total tokens used"
/// so the banner reflects real usage rather than static flavor text.
///
/// `cost_usd` is estimated all-time Oxen cloud spend across every model and
/// project (recorded input/output tokens at current catalog rates), rendered as
/// a "Total dollars spent" row in place of the old landmark row. `None` when
/// pricing is unavailable, shown as "—".
pub fn banner(
    ui: &Ui,
    base_url: &str,
    model: &str,
    workspace: &str,
    session: &str,
    tokens_used: usize,
    cost_usd: Option<f64>,
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
        // The live token count and dollars-spent rows are emitted unconditionally
        // below (so they show even for a theme loaded from disk that lacks them),
        // so any static copies carried by the theme are skipped here to avoid
        // duplicates. "Date" gets today's date so the journal opens on today.
        if label == "Next landmark" || label == "Total dollars spent" {
            let spent = cost_usd.map(format_usd).unwrap_or_else(|| "—".into());
            out.push_str(&journal_row(ui, "Total dollars spent", &spent));
            spend_rendered = true;
        } else if label == "Total tokens used" {
            // Rendered live after this loop — skip the static flavor copy.
        } else if label == "Date" {
            out.push_str(&journal_row(ui, label, &today()));
        } else if label == "Weather" {
            out.push_str(&journal_row(ui, label, weather()));
        } else {
            out.push_str(&journal_row(ui, label, value));
        }
    }

    // Always show the live all-time token count. Custom themes without the
    // standard landmark/spend slot still get the estimated cloud-spend row.
    out.push_str(&journal_row(
        ui,
        "Total tokens used",
        &format!("{tokens_used} tokens"),
    ));
    if !spend_rendered {
        let spent = cost_usd.map(format_usd).unwrap_or_else(|| "—".into());
        out.push_str(&journal_row(ui, "Total dollars spent", &spent));
    }

    out.push('\n');
    out.push_str(&format!("  {}\n", ui.dim(&v.bottom_hint)));
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
    out.push_str(&format!("\n  {}\n", ui.brown(&v.help_footer)));
    out
}

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
        assert!(!banner(&ui, "u", "m", "w", "s", 0, None).contains("\x1b["));
        assert!(!death_screen(&ui, "abc123").contains("\x1b["));
    }

    #[test]
    fn banner_shows_a_live_date_not_the_static_flavor() {
        let ui = colored();
        let out = banner(&ui, "u", "m", "w", "s", 0, None);
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
        assert!(banner(&ui, "u", "m", "w", "s", 0, None).contains("Fort Laramie, Wyoming"));
    }

    #[test]
    fn banner_shows_live_token_count() {
        let ui = Ui::plain();
        let b = banner(&ui, "u", "m", "w", "s", 1234, None);
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
        let b = banner(&ui, "u", "m", "w", "s", 555, Some(1.25));
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
        let priced = banner(&ui, "u", "m", "w", "s", 1234, Some(0.42));
        assert!(priced.contains("Total dollars spent"));
        assert!(priced.contains("$0.42"));
        let unavailable = banner(&ui, "u", "m", "w", "s", 0, None);
        assert!(unavailable.contains("Total dollars spent"));
    }

    #[test]
    fn banner_replaces_next_landmark_with_dollars_spent() {
        let b = banner(&Ui::plain(), "u", "m", "w", "s", 1234, Some(0.42));
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
        let b = banner(&ui, "host", "model", "ws", "sess", 0, None);
        assert!(b.contains("Oregon Trail"));
        assert!(b.contains("model"));
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
