//! Built-in themes shipped with oxen-harness.
//!
//! [`Theme::default`](crate::Theme::default) is Oregon Trail. This module adds a
//! couple more so users can switch immediately and see how a theme reshapes the
//! whole CLI/app. Each is built by overlaying a small patch on the default, so
//! anything not mentioned keeps the (well-tested) default behavior.

use std::collections::BTreeMap;

use crate::{s, Color, HelpItem, Meta, Palette, Style, Theme, Voice, THEME_SCHEMA_VERSION};

/// Every built-in theme, default first.
pub fn all() -> Vec<Theme> {
    vec![
        Theme::default(),
        midnight(),
        synthwave(),
        new_york_times(),
        cupertino(),
    ]
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

/// Build a [`Style`] from its fields without the struct-literal noise.
#[allow(clippy::too_many_arguments)]
fn style(
    display: &str,
    body: &str,
    mono: &str,
    transform: &str,
    spacing: &str,
    radius: &str,
    border: &str,
    shadow: &str,
    hero: &str,
    scene: &str,
) -> Style {
    Style {
        font_display: display.into(),
        font_body: body.into(),
        font_mono: mono.into(),
        display_transform: transform.into(),
        display_spacing: spacing.into(),
        radius: radius.into(),
        border_width: border.into(),
        shadow: shadow.into(),
        hero: hero.into(),
        scene: scene.into(),
    }
}

// Common system stacks reused across themes.
const SYS_SANS: &str = "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Segoe UI\", Inter, Roboto, Helvetica, Arial, sans-serif";
const SYS_SERIF: &str = "Georgia, \"Times New Roman\", \"PT Serif\", serif";
const SYS_MONO: &str = "\"SF Mono\", ui-monospace, Menlo, Consolas, monospace";

/// The original Oregon-Trail-on-a-CRT theme — the harness default
/// ([`Theme::default`](crate::Theme::default) returns this).
pub(crate) fn oregon_trail() -> Theme {
    let schema_version = THEME_SCHEMA_VERSION;
    let mut tool_verbs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    tool_verbs.insert(
        s("read_file"),
        lines(&["Reading the trail guide", "Studying the worn map"]),
    );
    tool_verbs.insert(
        s("write_file"),
        lines(&["Writing in the journal", "Etching a new tombstone"]),
    );
    tool_verbs.insert(
        s("edit_file"),
        lines(&["Mending the wagon", "Patching the wagon canvas"]),
    );
    tool_verbs.insert(
        s("find_files"),
        lines(&["Scouting for landmarks", "Surveying the trail"]),
    );
    tool_verbs.insert(
        s("search_files"),
        lines(&["Hunting through the brush", "Tracking through the prairie"]),
    );
    tool_verbs.insert(
        s("run_shell"),
        lines(&["Yoking the oxen", "Setting the wagon in motion"]),
    );
    tool_verbs.insert(
        s("git"),
        lines(&["Caulking the wagon", "Fording the river"]),
    );
    tool_verbs.insert(
        s("web_search"),
        lines(&["Wiring the telegraph", "Asking at the telegraph office"]),
    );
    tool_verbs.insert(
        s("ask_user_question"),
        lines(&["Holding a trail council", "Consulting the wagon party"]),
    );
    tool_verbs.insert(s("default"), lines(&["Working the trail"]));

    Theme {
        schema_version,
        meta: Meta {
            name: s("Oregon Trail"),
            author: s("oxen-harness"),
            description: s("1980s Oregon Trail on a CRT: tan titles, saddle brown, prairie green."),
        },
        palette: Palette {
            title: Color::new(240, 190, 140),
            primary: Color::new(96, 176, 96),
            secondary: Color::new(170, 110, 60),
            text: Color::new(236, 226, 206),
            muted: Color::new(150, 140, 125),
            danger: Color::new(205, 84, 72),
            link: Color::new(120, 178, 214),
            background: Color::new(15, 17, 21),
            surface: Color::new(22, 25, 34),
            border: Color::new(38, 44, 58),
        },
        voice: Voice {
            prompt_icon: s("🐂"),
            prompt_label: s("trail ❯"),
            spinner_glyphs: lines(&["✶", "✸", "✺", "✹", "✷", "✦"]),
            thinking: lines(&[
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
            ]),
            tool_verbs,
            deaths: lines(&[
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
            ]),
            wordmark: s("OXEN TRAIL"),
            banner_art: lines(&[
                r"      /\          /\          /\         ",
                r"     /  \   /\   /  \   /\   /  \         ",
                r"  __/    \_/  \_/    \_/  \_/    \___     ",
                r"                  _______________",
                r"                ,'               '.___",
                r"   ____________,'    Oxen.ai        '.__",
                r"  |  ~   ~   ~  |~   ~   ~   ~   ~    |  '.",
                r"  |_____________|____________________|____\",
                r"        (O)                    (O)",
                r"^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`",
            ]),
            pre_tagline: s("～ The ～"),
            subtitle: s("an open source agentic coding trail · powered by Oxen.ai"),
            flavor_top: vec![[s("Departing"), s("Independence, Missouri · 1848")]],
            // The franchise's iconic status panel. These are flavor (static), but
            // they recreate the screen the moment a fresh trail begins — and feed
            // both the CLI banner footer and the desktop hero's status panel.
            flavor_bottom: vec![
                [s("Date"), s("March 21, 1848")],
                [s("Next landmark"), s("128000 tokens")],
                [s("Total tokens used"), s("0 tokens")],
            ],
            bottom_hint: s("Send a message to begin on your trail"),
            label_provider: s("Provider"),
            label_model: s("Model"),
            label_workspace: s("Wagon (workspace)"),
            label_session: s("Trail journal"),
            label_disk_used: s("Supplies on hand (disk used):"),
            label_models_dir: s("Wagon stores (dir):"),
            help_header: s("You may:"),
            help_footer: s("What is your choice?"),
            help_items: vec![
                HelpItem {
                    key: s("1."),
                    title: s("Travel the trail"),
                    hint: s("— just type what you want done"),
                },
                HelpItem {
                    key: s("2."),
                    title: s("Learn about the trail"),
                    hint: s("— /help"),
                },
                HelpItem {
                    key: s("3."),
                    title: s("See the Oregon Top Ten"),
                    hint: s("— /export [path]  (save the journey as JSONL)"),
                },
                HelpItem {
                    key: s("4."),
                    title: s("Trade your oxen"),
                    hint: s("— /model [name]  (any new id is saved to the catalog)"),
                },
                HelpItem {
                    key: s("5."),
                    title: s("Change your colors"),
                    hint: s("— /theme  (select, create, import, export)"),
                },
                HelpItem {
                    key: s("6."),
                    title: s("Pack the wagon"),
                    hint: s("— /queue add <msg> … then /queue run"),
                },
                HelpItem {
                    key: s("7."),
                    title: s("Set the wagon rolling"),
                    hint: s("— /loop run [name]  (work until the gate is green)"),
                },
                HelpItem {
                    key: s("8."),
                    title: s("Inspect the wagon"),
                    hint: s("— /code-review [branch]  (find → verify → report on your changes)"),
                },
                HelpItem {
                    key: s("9."),
                    title: s("Choose your departure"),
                    hint: s(
                        "— /location <place>  (set where the trail begins; /departing works too)",
                    ),
                },
                HelpItem {
                    key: s("10."),
                    title: s("Check your know-how"),
                    hint: s("— /skills  (the workflows the agent has learned)"),
                },
                HelpItem {
                    key: s("11."),
                    title: s("Show your papers"),
                    hint: s("— /auth  (set your Oxen API key)"),
                },
                HelpItem {
                    key: s("12."),
                    title: s("Lighten the load"),
                    hint: s("— /compression [off|audit|on]  (context compression)"),
                },
                HelpItem {
                    key: s("13."),
                    title: s("Press on after a mishap"),
                    hint: s("— /retry  (continue a turn that died mid-trail)"),
                },
                HelpItem {
                    key: s("14."),
                    title: s("Make camp / End"),
                    hint: s("— /exit  (or Ctrl-D)"),
                },
            ],
            exit_art: lines(&[
                r"        _______________        ",
                r"      .'               '.      ",
                r"     /                   \     ",
                r"    /       R. I. P.       \    ",
                r"    |                      |    ",
                r"    |      here lies a     |    ",
                r"    |    weary  pioneer    |    ",
                r"    |                      |    ",
                r"    |                      |    ",
            ]),
            exit_ground: s("^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`^~`"),
            resume_message: s("Your trail journal was saved. Resume this expedition with:"),
            progress_icon: s("🐂"),
        },
        style: Style::default(),
    }
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
    // Calm and modern: IBM Plex, soft shadows, generous radius, a clean splash.
    t.style = style(
        "\"PlexSans\", -apple-system, sans-serif",
        "\"PlexSans\", -apple-system, sans-serif",
        "\"PlexMono\", ui-monospace, monospace",
        "none",
        "-0.01em",
        "10px",
        "1px",
        "soft",
        "minimal",
        "trail",
    );
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
    // Neon retro: wide-tracked Orbitron caps, VT323 readouts, accent glow. The
    // pixel hero stays — a recolored neon wagon is right at home on the grid.
    t.style = style(
        "\"Orbitron\", \"PixelHead\", sans-serif",
        SYS_SANS,
        "\"PixelRead\", \"SF Mono\", monospace",
        "uppercase",
        "0.12em",
        "2px",
        "2px",
        "glow",
        "pixel",
        "grid",
    );
    t
}

/// A broadsheet newspaper — hairline rules, high-contrast serif, a blackletter
/// masthead. "All the code that's fit to commit."
fn new_york_times() -> Theme {
    let mut t = Theme::default();
    t.meta.name = "New York Times".into();
    t.meta.author = "oxen-harness".into();
    t.meta.description =
        "A broadsheet newspaper — hairline rules, high-contrast serif, blackletter masthead."
            .into();

    t.palette = crate::Palette {
        title: Color::new(20, 18, 16),
        primary: Color::new(150, 42, 36), // a restrained press red for accents
        secondary: Color::new(94, 90, 82),
        text: Color::new(26, 24, 22),
        muted: Color::new(120, 114, 104),
        danger: Color::new(150, 42, 36),
        link: Color::new(42, 70, 130),
        background: Color::new(247, 245, 238), // newsprint
        surface: Color::new(241, 238, 230),
        border: Color::new(210, 205, 194),
    };

    t.voice.prompt_icon = "📰".into();
    t.voice.prompt_label = "file ❯".into();
    t.voice.spinner_glyphs = lines(&["·", "‥", "…", "—", "–", "·"]);
    t.voice.thinking = lines(&[
        "Going to press",
        "Checking the facts",
        "Consulting the wire",
        "Editing the copy",
        "Setting the type",
        "Proofing the galley",
        "Calling the newsroom",
        "Holding the front page",
    ]);
    t.voice.tool_verbs = verbs(&[
        (
            "read_file",
            &["Reading the wire", "Reviewing the clippings"],
        ),
        ("write_file", &["Filing the story", "Typing the copy"]),
        ("edit_file", &["Editing the copy", "Marking up the galley"]),
        (
            "find_files",
            &["Combing the archives", "Searching the morgue"],
        ),
        ("search_files", &["Chasing the lead", "Working the beat"]),
        ("run_shell", &["Running the presses", "Going to press"]),
        ("git", &["Putting the edition to bed"]),
        (
            "web_search",
            &["Calling the newsroom", "Working the sources"],
        ),
        ("ask_user_question", &["Interviewing the editor"]),
        ("default", &["On the beat"]),
    ]);
    t.voice.deaths = lines(&[
        "STOP THE PRESSES.",
        "The edition has gone to bed.",
        "The presses have stopped.",
        "30. — the end.",
        "The newsroom has gone dark.",
    ]);
    t.voice.wordmark = "The Oxen Times".into();
    t.voice.pre_tagline = "Est. 1848 · Independence, MO".into();
    t.voice.subtitle = "All the code that's fit to commit".into();
    t.voice.flavor_top = vec![[s("Vol. CLXXIII"), s("No. 1 · Late Edition")]];
    t.voice.flavor_bottom = vec![
        [s("Weather"), s("Fair, scattered merges")],
        [s("Markets"), s("Diffs up, bugs down")],
        [s("Index"), s("Tests · Commits · Reviews")],
    ];
    t.voice.bottom_hint = "Read all about it — type to file a story".into();
    t.voice.label_provider = "Bureau".into();
    t.voice.label_model = "Correspondent (model)".into();
    t.voice.label_workspace = "Desk (workspace)".into();
    t.voice.label_session = "Edition".into();
    t.voice.resume_message = "Your edition was saved. Resume the run with:".into();
    t.voice.exit_art = lines(&[
        r"  ┌───────────────────────┐  ",
        r"  │   — THE OXEN TIMES —   │  ",
        r"  │      F I N A L         │  ",
        r"  └───────────────────────┘  ",
    ]);
    t.voice.exit_ground = "────────────────────────────".into();
    t.voice.progress_icon = "📰".into();
    t.style = style(
        "\"Playfair\", Georgia, serif",
        SYS_SERIF,
        SYS_MONO,
        "none",
        "0",
        "0px",
        "1px",
        "none",
        "newspaper",
        "none",
    );
    t
}

/// Modern, sleek, and minimal — system SF, soft neutrals, generous radius.
fn cupertino() -> Theme {
    let mut t = Theme::default();
    t.meta.name = "Cupertino".into();
    t.meta.author = "oxen-harness".into();
    t.meta.description = "Modern, sleek, and minimal — soft neutrals and the system font.".into();

    t.palette = crate::Palette {
        title: Color::new(28, 28, 30),
        primary: Color::new(0, 122, 255), // system blue
        secondary: Color::new(142, 142, 147),
        text: Color::new(28, 28, 30),
        muted: Color::new(142, 142, 147),
        danger: Color::new(255, 59, 48),
        link: Color::new(0, 122, 255),
        background: Color::new(255, 255, 255),
        surface: Color::new(245, 245, 247),
        border: Color::new(210, 210, 215),
    };

    t.voice.prompt_icon = "✦".into();
    t.voice.prompt_label = "ask ❯".into();
    t.voice.spinner_glyphs = lines(&["◜", "◠", "◝", "◞", "◡", "◟"]);
    t.voice.thinking = lines(&[
        "Thinking",
        "Working on it",
        "Putting it together",
        "Looking into it",
        "Just a moment",
        "Almost there",
    ]);
    t.voice.tool_verbs = verbs(&[
        ("read_file", &["Reading"]),
        ("write_file", &["Writing"]),
        ("edit_file", &["Editing"]),
        ("find_files", &["Finding files"]),
        ("search_files", &["Searching"]),
        ("run_shell", &["Running"]),
        ("git", &["Working with git"]),
        ("web_search", &["Searching the web"]),
        ("ask_user_question", &["Checking in"]),
        ("default", &["Working"]),
    ]);
    t.voice.deaths = lines(&[
        "Session ended.",
        "See you soon.",
        "That's a wrap.",
        "Signing off.",
        "Until next time.",
    ]);
    t.voice.wordmark = "Oxen Harness".into();
    t.voice.pre_tagline = "".into();
    t.voice.subtitle = "Your agentic coding companion".into();
    t.voice.flavor_top = vec![];
    t.voice.flavor_bottom = vec![];
    t.voice.bottom_hint = "What should we build?".into();
    t.voice.label_provider = "Provider".into();
    t.voice.label_model = "Model".into();
    t.voice.label_workspace = "Workspace".into();
    t.voice.label_session = "Session".into();
    t.voice.resume_message = "Your session was saved. Resume it with:".into();
    t.voice.exit_art = lines(&[r"   ·  ·  ·   ", r"   see you    ", r"   ·  ·  ·   "]);
    t.voice.exit_ground = "".into();
    t.voice.progress_icon = "✦".into();
    t.style = style(
        "-apple-system, BlinkMacSystemFont, \"SF Pro Display\", sans-serif",
        SYS_SANS,
        SYS_MONO,
        "none",
        "-0.02em",
        "14px",
        "1px",
        "soft",
        "minimal",
        "none",
    );
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
