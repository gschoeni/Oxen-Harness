//! The [`Theme`] aggregate — identity, palette, voice, and style — plus the
//! partial-override loading that lets a theme file specify only the fields it
//! wants to change.
//!
//! Every loader ([`Theme::from_toml_str`], [`Theme::from_json_str`],
//! [`Theme::from_model_output`]) deep-merges the input over the default theme, so
//! omissions inherit Oregon Trail and any file — however sparse — yields a
//! complete, valid theme.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Color, Style, ThemeError};

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

impl Default for Theme {
    fn default() -> Self {
        crate::builtins::oregon_trail()
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
    fn theme_carries_schema_version() {
        assert_eq!(Theme::default().schema_version, THEME_SCHEMA_VERSION);
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
