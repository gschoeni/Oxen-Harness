//! The local model catalog: which models run here, from which weight files,
//! with which settings.
//!
//! Built-ins are embedded from `assets/catalog.json`; the user can add or
//! override entries in `~/.oxen-harness/local-media-models.json` (same
//! shape — an entry with a built-in's id replaces it). Each entry names the
//! engine that runs it and its weight files by role (`diffusion_model`,
//! `vae`, `llm` — the engine decides what a role means), and is turned into a
//! hub-shaped catalog row so the media tools validate and describe local
//! models with the code they already use for hosted ones.

use std::collections::BTreeMap;
use std::path::Path;

use harness_media::local::LOCAL_PREFIX;
use harness_media::{LocalInfo, MediaKind, MediaModel};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::LocalError;

const BUILTIN: &str = include_str!("../assets/catalog.json");

/// The user's additions and overrides, next to the LLM overlay.
pub const OVERLAY_FILE: &str = "local-media-models.json";

/// One weight file, addressed on Hugging Face.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeightFile {
    pub repo: String,
    /// Path inside the repo (`turbo/model.safetensors`).
    pub path: String,
    pub bytes: u64,
    /// Checked after download when present.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Default sampling settings, and the most steps a call may ask for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sampling {
    pub steps: u32,
    pub max_steps: u32,
    pub cfg_scale: f64,
    pub sampler: String,
}

/// The default output size; other aspect ratios keep its pixel count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// One local model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSpec {
    /// `local/…` (added when an overlay entry leaves it off).
    pub id: String,
    pub kind: MediaKind,
    /// The engine that runs it (`sd.cpp`).
    pub engine: String,
    pub display_name: String,
    #[serde(default)]
    pub developer: Option<String>,
    /// SPDX id; only commercially usable weights ship built in.
    pub license: String,
    #[serde(default)]
    pub summary: Option<String>,
    /// Weight files by role.
    pub files: BTreeMap<String, WeightFile>,
    pub sampling: Sampling,
    pub size: Size,
    /// How many reference images it takes (0 = text only).
    #[serde(default)]
    pub max_refs: u32,
    /// Extra command-line flags for the engine.
    #[serde(default)]
    pub engine_args: Vec<String>,
    /// Seconds per output at the defaults, measured on an M4 Max.
    #[serde(default)]
    pub typical_secs: Option<f64>,
}

#[derive(Deserialize)]
struct CatalogFile {
    #[serde(default)]
    models: Vec<ModelSpec>,
}

/// Aspect ratios a call can ask for by name.
pub const ASPECT_RATIOS: &[&str] = &["1:1", "4:3", "3:4", "3:2", "2:3", "16:9", "9:16", "21:9"];
/// Pixel dimensions must be multiples of this (every engine's patch size).
pub const SIZE_STEP: u32 = 16;
pub const MIN_SIDE: u32 = 256;
pub const MAX_SIDE: u32 = 2048;

/// The built-in catalog merged with the user's overlay in the harness
/// directory, if one exists. A malformed overlay is an error rather than
/// silently ignored: the user wrote it and should hear why it didn't load.
pub fn load() -> Result<Vec<ModelSpec>, LocalError> {
    let overlay = harness_config::paths::base_dir_unchecked().map(|d| d.join(OVERLAY_FILE));
    load_from(overlay.as_deref())
}

/// [`load`] with an explicit overlay path (`None` = built-ins only).
pub fn load_from(overlay: Option<&Path>) -> Result<Vec<ModelSpec>, LocalError> {
    let builtin: CatalogFile = serde_json::from_str(BUILTIN)
        .map_err(|e| LocalError::Catalog(format!("built-in catalog: {e}")))?;
    let mut models = builtin.models;
    if let Some(path) = overlay.filter(|p| p.exists()) {
        let text = std::fs::read_to_string(path)
            .map_err(|e| LocalError::Catalog(format!("{}: {e}", path.display())))?;
        let extra: CatalogFile = serde_json::from_str(&text)
            .map_err(|e| LocalError::Catalog(format!("{}: {e}", path.display())))?;
        for mut spec in extra.models {
            if !spec.id.starts_with(LOCAL_PREFIX) {
                spec.id = format!("{LOCAL_PREFIX}{}", spec.id);
            }
            match models.iter_mut().find(|m| m.id == spec.id) {
                Some(existing) => *existing = spec,
                None => models.push(spec),
            }
        }
    }
    Ok(models)
}

/// The settings one output is rendered with.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub cfg_scale: f64,
    pub sampler: String,
    pub seed: u64,
    pub negative_prompt: Option<String>,
}

impl ModelSpec {
    /// Whether classifier-free guidance is on, which is what makes a
    /// negative prompt mean anything.
    fn uses_cfg(&self) -> bool {
        self.sampling.cfg_scale > 1.0
    }

    /// The request schema the agent reads and the tools validate against,
    /// in the hub's shape.
    pub fn request_schema(&self) -> Value {
        let mut props = Map::new();
        props.insert(
            "prompt".into(),
            json!({"type": "string", "description": "What to generate."}),
        );
        props.insert(
            "aspect_ratio".into(),
            json!({
                "type": "string",
                "enum": ASPECT_RATIOS,
                "default": "1:1",
                "description": format!(
                    "Shape of the image at about {:.1} megapixels.",
                    (self.size.width * self.size.height) as f64 / 1e6
                ),
            }),
        );
        for (side, default) in [("width", self.size.width), ("height", self.size.height)] {
            props.insert(
                side.into(),
                json!({
                    "type": "integer",
                    "minimum": MIN_SIDE,
                    "maximum": MAX_SIDE,
                    "default": default,
                    "description": format!(
                        "Exact {side} in pixels (a multiple of {SIZE_STEP}); overrides \
                         aspect_ratio. Smaller is faster."
                    ),
                }),
            );
        }
        props.insert(
            "steps".into(),
            json!({
                "type": "integer",
                "minimum": 1,
                "maximum": self.sampling.max_steps,
                "default": self.sampling.steps,
                "description": "Sampling steps; time grows linearly with them.",
            }),
        );
        props.insert(
            "seed".into(),
            json!({"type": "integer", "minimum": 0, "description": "Fix to reproduce a result."}),
        );
        if self.uses_cfg() {
            props.insert(
                "cfg_scale".into(),
                json!({
                    "type": "number",
                    "minimum": 1.0,
                    "maximum": 12.0,
                    "default": self.sampling.cfg_scale,
                    "description": "How strictly to follow the prompt.",
                }),
            );
            props.insert(
                "negative_prompt".into(),
                json!({"type": "string", "description": "What to keep out of the image."}),
            );
        }
        if self.max_refs > 0 {
            props.insert(
                "input_images".into(),
                json!({
                    "type": "array",
                    "items": {"type": "string"},
                    "maxItems": self.max_refs,
                    "description": format!(
                        "Up to {} reference images to edit or combine; refer to them in \
                         the prompt as reference image 1, 2, …",
                        self.max_refs
                    ),
                }),
            );
        }
        json!({"type": "object", "properties": props, "required": ["prompt"]})
    }

    /// The catalog row the media tools see.
    pub fn to_media_model(&self, installed: bool, missing_bytes: u64) -> MediaModel {
        let mut inputs = vec!["text".to_string()];
        if self.max_refs > 0 {
            inputs.push("image".to_string());
        }
        MediaModel {
            id: self.id.clone(),
            kind: self.kind,
            display_name: Some(self.display_name.clone()),
            developer: self.developer.clone(),
            summary: self.summary.clone(),
            description: None,
            inputs,
            pricing: Value::Null,
            request_schema: self.request_schema(),
            local: Some(LocalInfo {
                installed,
                engine: self.engine.clone(),
                license: self.license.clone(),
                download_bytes: missing_bytes,
                typical_secs: self.typical_secs,
            }),
        }
    }

    /// Resolve a call's parameters (already validated against
    /// [`Self::request_schema`]) into concrete settings. `random_seed` is used
    /// when the call fixed none, so every output records the seed it used.
    pub fn settings(
        &self,
        params: &Map<String, Value>,
        random_seed: u64,
    ) -> Result<Settings, LocalError> {
        let int = |name: &str| params.get(name).and_then(Value::as_u64);
        let (width, height) = match (int("width"), int("height")) {
            (Some(w), Some(h)) => (w as u32, h as u32),
            (w, h) => {
                let ratio = params
                    .get("aspect_ratio")
                    .and_then(Value::as_str)
                    .unwrap_or("1:1");
                let (aw, ah) = dims_for(ratio, self.size).ok_or_else(|| {
                    LocalError::Invalid(format!(
                        "aspect_ratio must be one of {}",
                        ASPECT_RATIOS.join(", ")
                    ))
                })?;
                // One side given: keep the ratio's other side.
                match (w, h) {
                    (Some(w), None) => (w as u32, snap(w as f64 * ah as f64 / aw as f64)),
                    (None, Some(h)) => (snap(h as f64 * aw as f64 / ah as f64), h as u32),
                    _ => (aw, ah),
                }
            }
        };
        for (side, v) in [("width", width), ("height", height)] {
            if !(MIN_SIDE..=MAX_SIDE).contains(&v) || v % SIZE_STEP != 0 {
                return Err(LocalError::Invalid(format!(
                    "{side} must be a multiple of {SIZE_STEP} between {MIN_SIDE} and \
                     {MAX_SIDE} (got {v})"
                )));
            }
        }
        let steps = int("steps").map_or(self.sampling.steps, |s| s as u32);
        if steps == 0 || steps > self.sampling.max_steps {
            return Err(LocalError::Invalid(format!(
                "steps must be 1–{} for {}",
                self.sampling.max_steps, self.id
            )));
        }
        let cfg_scale = if self.uses_cfg() {
            params
                .get("cfg_scale")
                .and_then(Value::as_f64)
                .unwrap_or(self.sampling.cfg_scale)
        } else {
            self.sampling.cfg_scale
        };
        Ok(Settings {
            width,
            height,
            steps,
            cfg_scale,
            sampler: self.sampling.sampler.clone(),
            seed: int("seed").unwrap_or(random_seed),
            negative_prompt: params
                .get("negative_prompt")
                .and_then(Value::as_str)
                .filter(|s| self.uses_cfg() && !s.trim().is_empty())
                .map(str::to_string),
        })
    }

    /// Total bytes of its weight files.
    pub fn total_bytes(&self) -> u64 {
        self.files.values().map(|f| f.bytes).sum()
    }
}

/// Width × height for `ratio` at the pixel count of `base`, snapped to the
/// size step.
fn dims_for(ratio: &str, base: Size) -> Option<(u32, u32)> {
    if !ASPECT_RATIOS.contains(&ratio) {
        return None;
    }
    let (a, b) = ratio.split_once(':')?;
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    let area = base.width as f64 * base.height as f64;
    let width = (area * a / b).sqrt();
    Some((snap(width), snap(area / width)))
}

fn snap(v: f64) -> u32 {
    let step = SIZE_STEP as f64;
    ((v / step).round() * step).clamp(MIN_SIDE as f64, MAX_SIDE as f64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtins() -> Vec<ModelSpec> {
        load_from(None).expect("built-in catalog parses")
    }

    fn spec(id: &str) -> ModelSpec {
        builtins()
            .into_iter()
            .find(|m| m.id == id)
            .expect("built-in model")
    }

    #[test]
    fn builtins_are_local_commercial_image_models() {
        let models = builtins();
        assert_eq!(models.len(), 3);
        for m in &models {
            assert!(m.id.starts_with(LOCAL_PREFIX), "{}", m.id);
            assert_eq!(m.kind, MediaKind::Image);
            assert_eq!(
                m.license, "Apache-2.0",
                "{} must be commercially usable",
                m.id
            );
            assert!(m.files.contains_key("diffusion_model"), "{}", m.id);
            assert!(m.files.values().all(|f| f.sha256.is_some()), "{}", m.id);
        }
    }

    #[test]
    fn overlay_overrides_and_adds_with_the_local_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(OVERLAY_FILE);
        let mut klein = spec("local/flux-2-klein-4b");
        klein.typical_secs = Some(5.0);
        let mut extra = klein.clone();
        extra.id = "my-model".into();
        std::fs::write(&path, json!({ "models": [klein, extra] }).to_string()).expect("write");
        let models = load_from(Some(&path)).expect("overlay loads");
        assert_eq!(models.len(), 4);
        let k = models.iter().find(|m| m.id == "local/flux-2-klein-4b");
        assert_eq!(k.and_then(|m| m.typical_secs), Some(5.0));
        assert!(models.iter().any(|m| m.id == "local/my-model"));
    }

    #[test]
    fn a_broken_overlay_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(OVERLAY_FILE);
        std::fs::write(&path, "{ not json").expect("write");
        let err = load_from(Some(&path)).expect_err("malformed overlay");
        assert!(err.to_string().contains(OVERLAY_FILE), "{err}");
    }

    #[test]
    fn settings_keep_the_pixel_count_across_aspect_ratios() {
        let s = spec("local/iris-3b-turbo");
        let get = |p: Value| {
            s.settings(p.as_object().expect("object"), 7)
                .expect("valid settings")
        };
        let square = get(json!({}));
        assert_eq!(
            (square.width, square.height, square.steps, square.seed),
            (1024, 1024, 4, 7)
        );
        let wide = get(json!({"aspect_ratio": "16:9"}));
        assert_eq!((wide.width, wide.height), (1360, 768));
        assert_eq!(wide.width % SIZE_STEP, 0);
        let tall_by_width = get(json!({"aspect_ratio": "9:16", "width": 576}));
        assert_eq!((tall_by_width.width, tall_by_width.height), (576, 1024));
        let exact = get(json!({"width": 768, "height": 512, "seed": 3, "steps": 2}));
        assert_eq!(
            (exact.width, exact.height, exact.seed, exact.steps),
            (768, 512, 3, 2)
        );
    }

    #[test]
    fn settings_reject_bad_sizes_and_steps() {
        let s = spec("local/flux-2-klein-4b");
        let bad = |p: Value| s.settings(p.as_object().expect("object"), 0).is_err();
        assert!(bad(json!({"width": 1000, "height": 1024})));
        assert!(bad(json!({"width": 128, "height": 128})));
        assert!(bad(json!({"steps": 99})));
        assert!(bad(json!({"aspect_ratio": "5:4"})));
    }

    #[test]
    fn schema_offers_references_only_to_models_that_take_them() {
        let klein = spec("local/flux-2-klein-4b").to_media_model(true, 0);
        let iris = spec("local/iris-3b-turbo").to_media_model(false, 8_472_370_864);
        assert!(klein.has_param("input_images"));
        assert!(klein.accepts(harness_media::RefKind::Image));
        assert!(!iris.accepts(harness_media::RefKind::Image));
        assert!(
            iris.one_line().contains("not installed"),
            "{}",
            iris.one_line()
        );
        assert_eq!(klein.price_label(), "free, runs on this machine");
        // CFG-free models don't pretend a negative prompt does anything.
        assert!(!klein.has_param("negative_prompt"));
    }
}
