//! Configurable, shareable themes for oxen-harness.
//!
//! A [`Theme`] bundles a **palette** (named semantic colors) and a **voice**
//! (the prompt, spinner glyphs, "thinking" phrases, per-tool verbs, exit
//! messages, banner art, labels, and help text). Both the CLI and the desktop
//! app render from the active theme, so the entire personality of the harness
//! is data — not hardcoded.
//!
//! Themes serialize to a single self-contained TOML file (also readable as
//! JSON), which makes them trivial to **export, import, and share**. Files may
//! be *partial*: any field they omit falls back to the built-in default
//! (Oregon Trail), so a hand-written or model-generated theme can override just
//! a few colors or phrases. See [`Theme::from_toml_str`] / [`Theme::from_json_str`].
//!
//! The default ([`Theme::default`]) is the original **Oregon Trail** look.
//! [`builtins`] adds a couple more to switch between out of the box.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod builtins;
pub mod store;

pub use store::Store;

/// Errors loading, parsing, or persisting themes.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid theme TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("could not serialize theme: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("invalid theme JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not determine config directory")]
    NoConfigDir,
    #[error("no such theme: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
}

/// A 24-bit RGB color. Serializes as a `#rrggbb` hex string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// The `(r, g, b)` tuple used by the ANSI painters.
    pub fn rgb(&self) -> (u8, u8, u8) {
        (self.r, self.g, self.b)
    }

    /// The `#rrggbb` form used in theme files and CSS.
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Parse `#rrggbb` / `rrggbb` (also tolerates the 3-digit `#rgb` shorthand).
    pub fn parse(s: &str) -> Result<Self, String> {
        let h = s.trim().trim_start_matches('#');
        let expand = |c: char| {
            let d = c.to_digit(16).map(|v| v as u8);
            d.map(|v| v * 16 + v)
        };
        match h.len() {
            6 => {
                let n = u32::from_str_radix(h, 16).map_err(|_| format!("bad hex color: {s}"))?;
                Ok(Color::new((n >> 16) as u8, (n >> 8) as u8, n as u8))
            }
            3 => {
                let mut it = h.chars();
                let r = it.next().and_then(expand);
                let g = it.next().and_then(expand);
                let b = it.next().and_then(expand);
                match (r, g, b) {
                    (Some(r), Some(g), Some(b)) => Ok(Color::new(r, g, b)),
                    _ => Err(format!("bad hex color: {s}")),
                }
            }
            _ => Err(format!("expected #rrggbb, got: {s}")),
        }
    }
}

impl Serialize for Color {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Theme identity shown in listings and `export`ed files.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
}

/// The semantic color palette. CLI painters and the app CSS both read these.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Palette {
    /// Headings / wordmark (bolded).
    pub title: Color,
    /// Accents, success, and the prompt arrow.
    pub primary: Color,
    /// Dividers, labels, and flourishes.
    pub secondary: Color,
    /// Body text.
    pub text: Color,
    /// De-emphasized hints.
    pub muted: Color,
    /// Errors and the exit screen.
    pub danger: Color,
    /// Links.
    pub link: Color,
    /// App window background (desktop app only).
    pub background: Color,
    /// App panel/surface background (desktop app only).
    pub surface: Color,
    /// App borders (desktop app only).
    pub border: Color,
}

/// A single entry in the themed `/help` menu.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HelpItem {
    pub key: String,
    pub title: String,
    pub hint: String,
}

/// All the words and art that give a theme its personality.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Voice {
    /// Emoji/glyph before the prompt label (e.g. `🐂`).
    pub prompt_icon: String,
    /// The prompt label itself (e.g. `trail ❯`).
    pub prompt_label: String,
    /// Spinner animation frames.
    pub spinner_glyphs: Vec<String>,
    /// Phrases shown while the model is thinking.
    pub thinking: Vec<String>,
    /// Per-tool spinner verbs; the `default` key is the fallback.
    pub tool_verbs: BTreeMap<String, Vec<String>>,
    /// Flavorful "you died"/quit lines (one is chosen at random).
    pub deaths: Vec<String>,

    /// Big block-letter wordmark (A–Z and spaces render; others blank).
    pub wordmark: String,
    /// ASCII art scene printed above the wordmark.
    pub banner_art: Vec<String>,
    /// Small line above the wordmark (e.g. `～ The ～`).
    pub pre_tagline: String,
    /// One-line description under the wordmark.
    pub subtitle: String,
    /// Decorative `[label, value]` rows shown before the live session rows.
    pub flavor_top: Vec<[String; 2]>,
    /// Decorative `[label, value]` rows shown after the live session rows.
    pub flavor_bottom: Vec<[String; 2]>,
    /// Hint line at the bottom of the banner.
    pub bottom_hint: String,

    /// Label for the provider/base-url banner row.
    pub label_provider: String,
    /// Label for the model banner row.
    pub label_model: String,
    /// Label for the workspace banner row.
    pub label_workspace: String,
    /// Label for the session-id banner row.
    pub label_session: String,
    /// Label for total disk usage in the models table.
    pub label_disk_used: String,
    /// Label for the models directory in the models table.
    pub label_models_dir: String,

    /// Header above the `/help` menu.
    pub help_header: String,
    /// Footer/prompt below the `/help` menu.
    pub help_footer: String,
    /// The `/help` menu entries.
    pub help_items: Vec<HelpItem>,

    /// ASCII art for the exit/"game over" screen (e.g. a tombstone).
    pub exit_art: Vec<String>,
    /// A decorative ground line under the exit art.
    pub exit_ground: String,
    /// Line shown above the resume command on exit.
    pub resume_message: String,

    /// Icon shown at the head of the download progress bar.
    pub progress_icon: String,
}

/// Visual styling for the desktop app — typography and framing. The CLI is
/// bound to the terminal's own font, so these fields shape the desktop UI only;
/// the store maps them onto CSS tokens so the same components can render as a
/// chunky 8-bit trail, a hairline newspaper, a soft Apple-style app, or a neon
/// synth grid. Font families name faces the app bundles (see `pixel.css`):
/// `PixelHead`, `PixelRead`, `Playfair`, `Masthead`, `Orbitron`, `PlexSans`,
/// `PlexMono` — plus system stacks like `-apple-system` and `Georgia`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Style {
    /// CSS font-family stack for the wordmark, headings, and micro-labels.
    pub font_display: String,
    /// CSS font-family stack for body / message text.
    pub font_body: String,
    /// CSS font-family stack for code blocks and status readouts.
    pub font_mono: String,
    /// `"uppercase"` or `"none"` — applied to the wordmark and labels.
    pub display_transform: String,
    /// letter-spacing for display text + labels, e.g. `"0.02em"`, `"-0.01em"`.
    pub display_spacing: String,
    /// Corner radius for cards/inputs, a CSS length, e.g. `"3px"`, `"14px"`.
    pub radius: String,
    /// Border width for framed surfaces, e.g. `"2px"`, `"1px"`.
    pub border_width: String,
    /// Depth treatment: `"pixel"` (hard offset), `"soft"` (blurred), `"glow"`
    /// (neon accent), or `"none"` (flat).
    pub shadow: String,
    /// Hero layout for the empty state: `"pixel"` (a framed retro screen),
    /// `"newspaper"` (a masthead), or `"minimal"` (a clean app splash).
    pub hero: String,
    /// Which scene the `"pixel"` hero draws: `"trail"` (covered wagon), `"grid"`
    /// (synthwave outrun grid), or `"none"`. Ignored by other hero layouts.
    pub scene: String,
}

impl Default for Style {
    /// The Oregon Trail look: pixel display face, hard-edged framing.
    fn default() -> Self {
        Style {
            font_display: s("\"PixelHead\", \"Courier New\", monospace"),
            font_body: s(
                "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Segoe UI\", Inter, Roboto, Helvetica, Arial, sans-serif",
            ),
            font_mono: s("\"PixelRead\", \"SF Mono\", ui-monospace, monospace"),
            display_transform: s("uppercase"),
            display_spacing: s("0.02em"),
            radius: s("3px"),
            border_width: s("2px"),
            shadow: s("pixel"),
            hero: s("pixel"),
            scene: s("trail"),
        }
    }
}

impl Style {
    /// Replace any out-of-range enum value or malformed CSS length with the
    /// default for that field, so a hand-written or model-generated theme can't
    /// quietly produce broken UI (an unknown `shadow`, a `radius` of `"huge"`).
    fn sanitize(&mut self) {
        let d = Style::default();
        clamp(
            &mut self.display_transform,
            &["uppercase", "lowercase", "capitalize", "none"],
            &d.display_transform,
        );
        clamp(
            &mut self.shadow,
            &["pixel", "soft", "glow", "none"],
            &d.shadow,
        );
        clamp(&mut self.hero, &["pixel", "newspaper", "minimal"], &d.hero);
        clamp(&mut self.scene, &["trail", "grid", "none"], &d.scene);
        if !is_css_length(&self.radius) {
            self.radius = d.radius;
        }
        if !is_css_length(&self.border_width) {
            self.border_width = d.border_width;
        }
        if !is_css_length(&self.display_spacing) {
            self.display_spacing = d.display_spacing;
        }
    }
}

/// If `value` isn't one of `allowed`, reset it to `fallback`.
fn clamp(value: &mut String, allowed: &[&str], fallback: &str) {
    if !allowed.contains(&value.as_str()) {
        *value = fallback.to_string();
    }
}

/// Whether `s` is a plausible CSS length: `"0"`, or a (possibly signed/decimal)
/// number followed by a known unit. Deliberately conservative — anything it
/// rejects falls back to a safe default rather than reaching the stylesheet.
fn is_css_length(s: &str) -> bool {
    let s = s.trim();
    if s == "0" {
        return true;
    }
    const UNITS: [&str; 13] = [
        "px", "em", "rem", "%", "vh", "vw", "vmin", "vmax", "ch", "ex", "pt", "cm", "mm",
    ];
    let Some(unit_start) = UNITS
        .iter()
        .filter(|u| s.ends_with(*u))
        .max_by_key(|u| u.len())
        .map(|u| s.len() - u.len())
    else {
        return false;
    };
    let num = &s[..unit_start];
    !num.is_empty() && num.parse::<f64>().is_ok()
}

/// Current theme file schema version. Bump when the shape changes
/// incompatibly; files written before versioning read back as this default.
pub const THEME_SCHEMA_VERSION: u32 = 1;

fn default_theme_schema_version() -> u32 {
    THEME_SCHEMA_VERSION
}

/// A complete theme: identity, palette, voice, and visual style.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Theme {
    /// File-format version (see [`THEME_SCHEMA_VERSION`]).
    #[serde(default = "default_theme_schema_version")]
    pub schema_version: u32,
    pub meta: Meta,
    pub palette: Palette,
    pub voice: Voice,
    #[serde(default)]
    pub style: Style,
}

impl Theme {
    /// Spinner verbs for a tool, falling back to the `default` pool.
    pub fn tool_verbs(&self, tool: &str) -> Vec<String> {
        self.voice
            .tool_verbs
            .get(tool)
            .filter(|v| !v.is_empty())
            .or_else(|| self.voice.tool_verbs.get("default"))
            .cloned()
            .unwrap_or_else(|| vec!["Working".to_string()])
    }

    /// Serialize to a pretty TOML document (for `export` / saving).
    pub fn to_toml(&self) -> Result<String, ThemeError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Load a theme from TOML, layering it over the default so partial files
    /// (overriding only some fields) work.
    pub fn from_toml_str(s: &str) -> Result<Theme, ThemeError> {
        let patch: serde_json::Value = toml::from_str(s)?;
        Self::from_patch(patch)
    }

    /// Load a theme from JSON, with the same partial-override semantics.
    pub fn from_json_str(s: &str) -> Result<Theme, ThemeError> {
        let patch: serde_json::Value = serde_json::from_str(s)?;
        Self::from_patch(patch)
    }

    fn from_patch(patch: serde_json::Value) -> Result<Theme, ThemeError> {
        let base = serde_json::to_value(Theme::default())?;
        let merged = deep_merge(base, patch);
        let mut theme: Theme = serde_json::from_value(merged)?;
        // Loaded/imported themes (incl. model-generated ones) may carry invalid
        // style values; clamp them so they can't break the UI.
        theme.style.sanitize();
        Ok(theme)
    }

    /// Parse a theme from a model's raw response: tolerate surrounding prose and
    /// ```` ```toml ```` / ```` ```json ```` fences, try TOML then JSON, and give
    /// a theme a fallback name *only if the model omitted one* (so a nameless
    /// generation doesn't silently shadow a built-in). Used by `/theme new`.
    pub fn from_model_output(raw: &str) -> Result<Theme, ThemeError> {
        let cleaned = strip_fences(raw);
        let patch = parse_patch_value(cleaned).ok_or_else(|| {
            ThemeError::Invalid("could not parse a theme from the model output".to_string())
        })?;
        let has_name = patch
            .pointer("/meta/name")
            .and_then(|n| n.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let mut theme = Self::from_patch(patch)?;
        if !has_name {
            theme.meta.name = "Custom Theme".to_string();
        }
        Ok(theme)
    }

    /// The full system prompt for asking a model to design a theme.
    pub fn generation_system_prompt() -> String {
        format!(
            "You are a master terminal-UI theme designer for the oxen-harness \
             coding agent. Given a brief, output a single complete theme as a \
             TOML document and NOTHING else — no explanations, no markdown code \
             fences.\n\n{}",
            Self::schema_doc()
        )
    }

    /// A schema description + example, handed to a model when generating a
    /// theme from a natural-language "vibe".
    pub fn schema_doc() -> String {
        let example = Theme::default()
            .to_toml()
            .unwrap_or_else(|_| "<error>".to_string());
        format!(
            "A theme is a TOML document with `[meta]`, `[palette]`, `[voice]`, \
             and `[style]` sections.\n\n\
             - Colors in `[palette]` are `#rrggbb` hex strings: title, primary, \
             secondary, text, muted, danger, link (terminal foreground colors), \
             plus background, surface, border (used by the desktop app).\n\
             - `[voice]` holds the personality: prompt_icon/prompt_label, \
             spinner_glyphs, thinking (phrases), tool_verbs (a table keyed by \
             tool name with a `default` fallback), deaths (quit lines), wordmark \
             (UPPERCASE block letters), banner_art (lines of ASCII art), \
             subtitle, flavor_top/flavor_bottom ([label, value] rows), help_items, \
             exit_art, and assorted labels.\n\
             - `[style]` shapes the desktop app's typography and framing: \
             font_display/font_body/font_mono (CSS font-family stacks; bundled \
             faces are PixelHead, PixelRead, Playfair, Masthead, Orbitron, \
             PlexSans, PlexMono, plus system stacks), display_transform \
             (uppercase|none), display_spacing (letter-spacing), radius, \
             border_width, shadow (pixel|soft|glow|none), hero \
             (pixel|newspaper|minimal), and scene (trail|grid|none — the art the \
             pixel hero draws).\n\n\
             Any omitted field inherits the default. Here is the default theme as \
             a complete reference:\n\n```toml\n{example}\n```"
        )
    }
}

/// Drop a leading/trailing ```` ``` ```` fence (optionally tagged `toml`/`json`).
fn strip_fences(raw: &str) -> &str {
    let t = raw.trim();
    let t = t
        .strip_prefix("```toml")
        .or_else(|| t.strip_prefix("```json"))
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim()
}

/// Parse model output into a patch `Value`, trying TOML then a JSON object
/// embedded anywhere in the text.
fn parse_patch_value(cleaned: &str) -> Option<serde_json::Value> {
    if let Ok(v) = toml::from_str::<serde_json::Value>(cleaned) {
        if v.is_object() {
            return Some(v);
        }
    }
    if let (Some(start), Some(end)) = (cleaned.find('{'), cleaned.rfind('}')) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cleaned[start..=end]) {
            return Some(v);
        }
    }
    None
}

/// Recursively merge `patch` onto `base`: objects merge key-by-key; any other
/// value (array, string, number, null) from `patch` replaces `base`.
fn deep_merge(base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match (base, patch) {
        (Value::Object(mut b), Value::Object(p)) => {
            for (k, pv) in p {
                let bv = b.remove(&k).unwrap_or(Value::Null);
                b.insert(k, deep_merge(bv, pv));
            }
            Value::Object(b)
        }
        (_, patch) => patch,
    }
}

impl Default for Theme {
    fn default() -> Self {
        oregon_trail()
    }
}

fn s(x: &str) -> String {
    x.to_string()
}

fn list(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|x| s(x)).collect()
}

/// The original Oregon-Trail-on-a-CRT theme — the default.
fn oregon_trail() -> Theme {
    let schema_version = THEME_SCHEMA_VERSION;
    let mut tool_verbs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    tool_verbs.insert(
        s("read_file"),
        list(&["Reading the trail guide", "Studying the worn map"]),
    );
    tool_verbs.insert(
        s("write_file"),
        list(&["Writing in the journal", "Etching a new tombstone"]),
    );
    tool_verbs.insert(
        s("edit_file"),
        list(&["Mending the wagon", "Patching the wagon canvas"]),
    );
    tool_verbs.insert(
        s("find_files"),
        list(&["Scouting for landmarks", "Surveying the trail"]),
    );
    tool_verbs.insert(
        s("search_files"),
        list(&["Hunting through the brush", "Tracking through the prairie"]),
    );
    tool_verbs.insert(
        s("run_shell"),
        list(&["Yoking the oxen", "Setting the wagon in motion"]),
    );
    tool_verbs.insert(s("git"), list(&["Caulking the wagon", "Fording the river"]));
    tool_verbs.insert(
        s("web_search"),
        list(&["Wiring the telegraph", "Asking at the telegraph office"]),
    );
    tool_verbs.insert(
        s("ask_user_question"),
        list(&["Holding a trail council", "Consulting the wagon party"]),
    );
    tool_verbs.insert(s("default"), list(&["Working the trail"]));

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
            spinner_glyphs: list(&["✶", "✸", "✺", "✹", "✷", "✦"]),
            thinking: list(&[
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
            deaths: list(&[
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
            banner_art: list(&[
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
                [s("Weather"), s("warm")],
                [s("Health"), s("good")],
                [s("Food"), s("1009 pounds")],
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
                    hint: s("— /model [name]"),
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
                    title: s("Choose your departure"),
                    hint: s("— /departing <place>  (set where the trail begins)"),
                },
                HelpItem {
                    key: s("9."),
                    title: s("Make camp / End"),
                    hint: s("— /exit  (or Ctrl-D)"),
                },
            ],
            exit_art: list(&[
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_style_values_are_clamped_on_load() {
        // A model-generated theme with bad enum + length values.
        let toml = r#"
            [meta]
            name = "Bad"
            author = "model"
            description = "x"
            [style]
            shadow = "explode"
            hero = "spaceship"
            scene = "lava"
            display_transform = "rainbow"
            radius = "huge"
            border_width = "2px"
        "#;
        let theme = Theme::from_toml_str(toml).unwrap();
        let d = Style::default();
        assert_eq!(theme.style.shadow, d.shadow);
        assert_eq!(theme.style.hero, d.hero);
        assert_eq!(theme.style.scene, d.scene);
        assert_eq!(theme.style.display_transform, d.display_transform);
        assert_eq!(theme.style.radius, d.radius); // "huge" rejected
        assert_eq!(theme.style.border_width, "2px"); // valid, preserved
    }

    #[test]
    fn css_length_validator_accepts_lengths_and_rejects_junk() {
        for ok in ["0", "3px", "0.02em", "-0.01em", "100%", "1.5rem"] {
            assert!(is_css_length(ok), "{ok} should be valid");
        }
        for bad in ["", "huge", "px", "10", "3 px", "red"] {
            assert!(!is_css_length(bad), "{bad} should be invalid");
        }
    }

    #[test]
    fn theme_carries_schema_version() {
        assert_eq!(Theme::default().schema_version, THEME_SCHEMA_VERSION);
    }

    #[test]
    fn color_round_trips_through_hex() {
        let c = Color::new(240, 190, 140);
        assert_eq!(c.hex(), "#f0be8c");
        assert_eq!(Color::parse("#f0be8c").unwrap(), c);
        assert_eq!(Color::parse("f0be8c").unwrap(), c);
        assert_eq!(Color::parse("#fff").unwrap(), Color::new(255, 255, 255));
        assert!(Color::parse("nope").is_err());
    }

    #[test]
    fn default_theme_round_trips_through_toml() {
        let theme = Theme::default();
        let toml = theme.to_toml().unwrap();
        let back = Theme::from_toml_str(&toml).unwrap();
        assert_eq!(theme, back);
    }

    #[test]
    fn partial_toml_overrides_only_named_fields() {
        let patch = r##"
            [meta]
            name = "My Trail"

            [palette]
            primary = "#ff00aa"

            [voice]
            prompt_label = "go ❯"
        "##;
        let theme = Theme::from_toml_str(patch).unwrap();
        assert_eq!(theme.meta.name, "My Trail");
        assert_eq!(theme.palette.primary, Color::new(255, 0, 170));
        assert_eq!(theme.voice.prompt_label, "go ❯");
        // Untouched fields inherit the Oregon Trail default.
        assert_eq!(theme.palette.title, Theme::default().palette.title);
        assert!(theme
            .voice
            .thinking
            .contains(&"Fording the river".to_string()));
    }

    #[test]
    fn partial_json_is_accepted_too() {
        let theme = Theme::from_json_str(r#"{"meta":{"name":"JSON Theme"}}"#).unwrap();
        assert_eq!(theme.meta.name, "JSON Theme");
        assert_eq!(theme.palette, Theme::default().palette);
    }

    #[test]
    fn tool_verbs_fall_back_to_default() {
        let theme = Theme::default();
        assert!(!theme.tool_verbs("read_file").is_empty());
        assert_eq!(
            theme.tool_verbs("nonexistent"),
            vec!["Working the trail".to_string()]
        );
    }

    #[test]
    fn schema_doc_mentions_sections() {
        let doc = Theme::schema_doc();
        assert!(doc.contains("[palette]"));
        assert!(doc.contains("[voice]"));
        assert!(doc.contains("```toml"));
    }

    #[test]
    fn from_model_output_handles_fences_prose_and_json() {
        let toml = "```toml\n[meta]\nname = \"Test\"\n[palette]\nprimary = \"#112233\"\n```";
        let t = Theme::from_model_output(toml).unwrap();
        assert_eq!(t.meta.name, "Test");
        assert_eq!(t.palette.primary, Color::new(0x11, 0x22, 0x33));
        assert!(!t.voice.thinking.is_empty());

        let json = "Here you go!\n{\"meta\":{\"name\":\"JSON One\"}}";
        assert_eq!(
            Theme::from_model_output(json).unwrap().meta.name,
            "JSON One"
        );

        // A theme that omits its name gets a fallback (so it can't silently
        // shadow a built-in like Oregon Trail).
        let unnamed = "[palette]\nprimary = \"#010203\"";
        assert_eq!(
            Theme::from_model_output(unnamed).unwrap().meta.name,
            "Custom Theme"
        );

        assert!(Theme::from_model_output("not a theme at all").is_err());
    }
}
