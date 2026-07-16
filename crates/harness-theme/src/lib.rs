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

pub mod builtins;
mod color;
pub mod spinner;
pub mod store;
mod style;
mod theme;

pub use color::Color;
pub use store::Store;
pub use style::Style;
pub use theme::{HelpItem, Meta, Palette, Theme, Voice, THEME_SCHEMA_VERSION};

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

/// `str` → owned `String`. A one-character helper that keeps the built-in theme
/// builders and style defaults readable amid hundreds of string fields.
pub(crate) fn s(x: &str) -> String {
    x.to_string()
}
