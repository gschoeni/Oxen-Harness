//! Built-in themes shipped with oxen-harness.
//!
//! [`Theme::default`](crate::Theme::default) is Oregon Trail. This module adds a
//! couple more so users can switch immediately and see how a theme reshapes the
//! whole CLI/app. Each is built by overlaying a small patch on the default, so
//! anything not mentioned keeps the (well-tested) default behavior.

use std::collections::BTreeMap;

use crate::{Color, Theme};

/// Every built-in theme, default first.
pub fn all() -> Vec<Theme> {
    vec![Theme::default(), midnight(), synthwave()]
}

/// The built-in theme names, default first.
pub fn names() -> Vec<String> {
    all().into_iter().map(|t| t.meta.name).collect()
}

/// Look up a built-in by display name or slug (case-insensitive).
pub fn by_name(name: &str) -> Option<Theme> {
    let want = crate::store::slug(name);
    all()
        .into_iter()
        .find(|t| crate::store::slug(&t.meta.name) == want)
}

fn verbs(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(k, vs)| (k.to_string(), vs.iter().map(|v| v.to_string()).collect()))
        .collect()
}

fn lines(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|x| x.to_string()).collect()
}

/// A calm, modern dark theme — cool blues and slate.
fn midnight() -> Theme {
    let mut t = Theme::default();
    t.meta.name = "Midnight".into();
    t.meta.author = "oxen-harness".into();
    t.meta.description = "A calm, modern dark theme — cool blues over slate.".into();

    t.palette = crate::Palette {
        title: Color::new(140, 190, 255),
        primary: Color::new(99, 179, 237),
        secondary: Color::new(110, 122, 150),
        text: Color::new(222, 230, 244),
        muted: Color::new(122, 132, 156),
        danger: Color::new(240, 113, 120),
        link: Color::new(129, 200, 190),
        background: Color::new(13, 16, 23),
        surface: Color::new(20, 25, 36),
        border: Color::new(34, 41, 58),
    };

    t.voice.prompt_icon = "🌙".into();
    t.voice.prompt_label = "night ❯".into();
    t.voice.spinner_glyphs = lines(&["·", "∶", "∴", "∷", "✦", "✧"]);
    t.voice.thinking = lines(&[
        "Thinking it through",
        "Tracing the call graph",
        "Weighing the options",
        "Reading the room",
        "Connecting the dots",
        "Composing a plan",
        "Sketching an approach",
        "Letting it compile in my head",
    ]);
    t.voice.tool_verbs = verbs(&[
        ("read_file", &["Reading", "Skimming the file"]),
        ("write_file", &["Writing", "Saving the file"]),
        ("edit_file", &["Editing", "Refactoring"]),
        ("find_files", &["Searching", "Globbing"]),
        ("search_files", &["Grepping", "Scanning"]),
        ("run_shell", &["Running", "Executing"]),
        ("git", &["Working with git"]),
        ("web_search", &["Searching the web"]),
        ("ask_user_question", &["Checking in with you"]),
        ("default", &["Working"]),
    ]);
    t.voice.deaths = lines(&[
        "Session ended. Sleep well.",
        "Lights out.",
        "Closing the laptop. Goodnight.",
        "Powering down for the night.",
        "The terminal goes quiet.",
    ]);
    t.voice.subtitle = "an open source agentic coding harness · powered by Oxen.ai".into();
    t.voice.pre_tagline = "～ the ～".into();
    t.voice.flavor_top = vec![["Session".into(), "started".into()]];
    t.voice.flavor_bottom = vec![["Status".into(), "ready".into()]];
    t.voice.bottom_hint = "Type /help for commands · Ctrl-D to exit".into();
    t.voice.label_provider = "Provider".into();
    t.voice.label_model = "Model".into();
    t.voice.label_workspace = "Workspace".into();
    t.voice.label_session = "Session".into();
    t.voice.label_disk_used = "Disk used:".into();
    t.voice.label_models_dir = "Models dir:".into();
    t.voice.resume_message = "Your session was saved. Resume it with:".into();
    t.voice.exit_art = lines(&[
        r"      .  *  .   .       ",
        r"   *   .    ___    .  * ",
        r"  .   .    /   \    .   ",
        r"      *    \   /   *    ",
        r"   .    .   \_/    .  . ",
    ]);
    t.voice.exit_ground = "· · · · · · · · · · · · · · · · ·".into();
    t.voice.progress_icon = "🌙".into();
    t
}

/// A loud, retro synthwave theme — hot magenta + cyan on deep purple.
fn synthwave() -> Theme {
    let mut t = Theme::default();
    t.meta.name = "Synthwave".into();
    t.meta.author = "oxen-harness".into();
    t.meta.description = "Retro 80s synthwave — neon magenta and cyan on deep purple.".into();

    t.palette = crate::Palette {
        title: Color::new(255, 113, 206),
        primary: Color::new(1, 205, 254),
        secondary: Color::new(185, 103, 255),
        text: Color::new(239, 233, 255),
        muted: Color::new(150, 130, 190),
        danger: Color::new(255, 92, 122),
        link: Color::new(5, 255, 161),
        background: Color::new(20, 9, 38),
        surface: Color::new(31, 16, 54),
        border: Color::new(58, 32, 92),
    };

    t.voice.prompt_icon = "🌆".into();
    t.voice.prompt_label = "ride ❯".into();
    t.voice.spinner_glyphs = lines(&["◜", "◝", "◞", "◟", "◆", "◇"]);
    t.voice.thinking = lines(&[
        "Riding the grid",
        "Cruising the neon highway",
        "Spinning up the synths",
        "Chasing the sunset",
        "Overclocking the dream",
        "Surfing the data stream",
        "Warming the cassette deck",
        "Boosting the turbo",
    ]);
    t.voice.tool_verbs = verbs(&[
        ("read_file", &["Loading the tape", "Reading the data"]),
        ("write_file", &["Burning to disk", "Saving the track"]),
        ("edit_file", &["Remixing", "Tuning the synth"]),
        ("find_files", &["Scanning the grid", "Sweeping the radar"]),
        ("search_files", &["Hunting the signal", "Tracing the beat"]),
        ("run_shell", &["Hitting the gas", "Engaging turbo"]),
        ("git", &["Syncing the mainframe"]),
        ("web_search", &["Dialing the modem", "Pinging the net"]),
        ("ask_user_question", &["Hailing the driver"]),
        ("default", &["Cruising"]),
    ]);
    t.voice.deaths = lines(&[
        "GAME OVER. Insert coin to continue.",
        "The grid powered down.",
        "You ran out of fuel on the neon highway.",
        "The sunset faded to black.",
        "CONNECTION LOST.",
    ]);
    t.voice.subtitle = "an open source agentic coding machine · powered by Oxen.ai".into();
    t.voice.pre_tagline = "～ THE ～".into();
    t.voice.flavor_top = vec![["Location".into(), "Neo Grid City · 1986".into()]];
    t.voice.flavor_bottom = vec![["Vibe".into(), "max  ·  Turbo: on  ·  Speed: 88mph".into()]];
    t.voice.bottom_hint = "Type /help to see the menu · Ctrl-D to power down".into();
    t.voice.label_provider = "Network".into();
    t.voice.label_model = "Engine (model)".into();
    t.voice.label_workspace = "Garage (workspace)".into();
    t.voice.label_session = "Save state".into();
    t.voice.label_disk_used = "Cartridge space used:".into();
    t.voice.label_models_dir = "Cartridge slot (dir):".into();
    t.voice.resume_message = "Save state stored. Continue the ride with:".into();
    t.voice.exit_art = lines(&[
        r"   ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄   ",
        r"   █  G A M E  O V E R  █   ",
        r"   ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀   ",
    ]);
    t.voice.exit_ground = "═══════════════════════════════".into();
    t.voice.progress_icon = "🚗".into();
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_distinct_and_serializable() {
        let all = all();
        assert!(all.len() >= 3);
        for t in &all {
            // Every built-in must serialize and round-trip.
            let toml = t.to_toml().unwrap();
            assert_eq!(Theme::from_toml_str(&toml).unwrap(), *t);
        }
        // Names are unique.
        let mut names = names();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len());
    }

    #[test]
    fn by_name_is_case_and_slug_insensitive() {
        assert_eq!(by_name("midnight").unwrap().meta.name, "Midnight");
        assert_eq!(by_name("SYNTHWAVE").unwrap().meta.name, "Synthwave");
        assert_eq!(by_name("Oregon Trail").unwrap().meta.name, "Oregon Trail");
        assert!(by_name("does-not-exist").is_none());
    }
}
