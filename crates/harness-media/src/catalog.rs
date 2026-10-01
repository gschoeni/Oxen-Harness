//! The media half of the hub's model catalog (`GET /api/ai/models`).
//!
//! Every image/video model ships a `request_schema` — a JSON Schema of its
//! parameters with enums, ranges, defaults and descriptions — and a
//! `pricing` block. That is what lets the agent fill seedance / wan / kling
//! parameters itself, lets the tools validate a call before spending, and
//! lets the budget estimate a cost. The catalog is public and slow-moving,
//! so it is cached on disk for a day and read from the cache when the hub
//! is unreachable.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::refs::RefKind;
use crate::MediaKind;

/// How long a cached catalog is trusted before refetching.
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// One image or video model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaModel {
    pub id: String,
    pub kind: MediaKind,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub developer: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Input modalities the hub advertises (`text`, `image`, `video`, `audio`).
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub pricing: Value,
    #[serde(default)]
    pub request_schema: Value,
}

/// A request field that takes reference media.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub name: String,
    /// Whether the field is a list (several references) or one URI.
    pub array: bool,
}

/// Candidate field names per reference kind, in preference order. The
/// first one a model's schema has wins; a second image goes to a `tail`
/// slot when the primary is a single URI.
const IMAGE_SLOTS: &[&str] = &[
    "input_image",
    "input_images",
    "reference_images",
    "image",
    "image_url",
    "images",
    "first_frame_image",
    "start_image_url",
    "start_image",
];
const IMAGE_TAIL_SLOTS: &[&str] = &["tail_image_url", "end_image_url", "last_frame_image"];
const VIDEO_SLOTS: &[&str] = &[
    "input_video",
    "input_videos",
    "video_url",
    "video",
    "source_video",
];
const AUDIO_SLOTS: &[&str] = &["input_audios", "input_audio", "audio_url", "audio"];

impl MediaModel {
    pub fn properties(&self) -> Option<&Map<String, Value>> {
        self.request_schema.get("properties")?.as_object()
    }

    pub fn property(&self, name: &str) -> Option<&Value> {
        self.properties()?.get(name)
    }

    pub fn has_param(&self, name: &str) -> bool {
        self.property(name).is_some()
    }

    pub fn param_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .properties()
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    pub fn default_of(&self, name: &str) -> Option<&Value> {
        self.property(name)?.get("default")
    }

    fn is_array(&self, name: &str) -> bool {
        self.property(name)
            .map(|p| {
                p.get("type").and_then(Value::as_str) == Some("array") || p.get("items").is_some()
            })
            .unwrap_or(false)
    }

    /// The field(s) that take references of `kind`: the primary slot and,
    /// for images on a single-URI model, the tail-frame slot.
    pub fn slots(&self, kind: RefKind) -> Vec<Slot> {
        let candidates = match kind {
            RefKind::Image => IMAGE_SLOTS,
            RefKind::Video => VIDEO_SLOTS,
            RefKind::Audio => AUDIO_SLOTS,
        };
        let mut out = Vec::new();
        if let Some(name) = candidates.iter().find(|n| self.has_param(n)) {
            out.push(Slot {
                name: name.to_string(),
                array: self.is_array(name),
            });
        }
        if kind == RefKind::Image && out.first().is_some_and(|s| !s.array) {
            if let Some(name) = IMAGE_TAIL_SLOTS.iter().find(|n| self.has_param(n)) {
                out.push(Slot {
                    name: name.to_string(),
                    array: false,
                });
            }
        }
        out
    }

    /// Whether the model takes references of `kind` at all.
    pub fn accepts(&self, kind: RefKind) -> bool {
        !self.slots(kind).is_empty()
    }

    /// Whether the prompt addresses references as `@Image1` / `@Video1`
    /// (seedance, kling, nano-banana) rather than by position.
    pub fn prompt_uses_at_refs(&self) -> bool {
        let text = self
            .property("prompt")
            .and_then(|p| p.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let refs_text = self
            .properties()
            .map(|props| {
                props
                    .values()
                    .filter_map(|p| p.get("description").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        text.contains("@Image") || refs_text.contains("@Image") || refs_text.contains("@Video")
    }

    /// `$0.01/image`, `$0.08–0.17/s`, `priced per image (quality × size)`.
    pub fn price_label(&self) -> String {
        let p = &self.pricing;
        match self.kind {
            MediaKind::Image => {
                if let Some(grid) = p.get("cost_per_image_grid").and_then(Value::as_object) {
                    let (lo, hi) = grid_range(grid);
                    if let (Some(lo), Some(hi)) = (lo, hi) {
                        return if (lo - hi).abs() < 1e-9 {
                            format!("{}/image", usd(lo))
                        } else {
                            format!("{}–{}/image", usd(lo), usd(hi))
                        };
                    }
                }
                match p.get("cost_per_image").and_then(Value::as_f64) {
                    Some(c) => format!("{}/image", usd(c)),
                    None => "unpriced".to_string(),
                }
            }
            MediaKind::Video => {
                if let Some(by_res) = p
                    .get("cost_per_second_by_resolution")
                    .and_then(Value::as_object)
                {
                    let mut costs: Vec<f64> = by_res.values().filter_map(Value::as_f64).collect();
                    costs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    if let (Some(lo), Some(hi)) = (costs.first(), costs.last()) {
                        return if (lo - hi).abs() < 1e-9 {
                            format!("{}/s", usd(*lo))
                        } else {
                            format!("{}–{}/s", usd(*lo), usd(*hi))
                        };
                    }
                }
                match p.get("cost_per_second").and_then(Value::as_f64) {
                    Some(c) => format!("{}/s", usd(c)),
                    None => "unpriced".to_string(),
                }
            }
        }
    }

    /// `id — $0.01/image — text+image → image — summary`
    pub fn one_line(&self) -> String {
        let inputs = if self.inputs.is_empty() {
            String::new()
        } else {
            format!(" — in: {}", self.inputs.join("+"))
        };
        let blurb = self
            .summary
            .as_deref()
            .or(self.description.as_deref())
            .map(|s| {
                let s = s.trim();
                let short: String = s.chars().take(110).collect();
                if short.len() < s.len() {
                    format!(" — {short}…")
                } else {
                    format!(" — {short}")
                }
            })
            .unwrap_or_default();
        format!("{} — {}{inputs}{blurb}", self.id, self.price_label())
    }

    /// A compact, model-readable rendering of the request schema: one line
    /// per parameter with type, choices or range, default, and description.
    pub fn schema_summary(&self) -> String {
        let Some(props) = self.properties() else {
            return "  (no parameters documented)".to_string();
        };
        let required: Vec<&str> = self
            .request_schema
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut ordered: Vec<(&String, &Value)> = props.iter().collect();
        ordered.sort_by_key(|(name, v)| {
            (
                v.get("x-order").and_then(Value::as_i64).unwrap_or(i64::MAX),
                (*name).clone(),
            )
        });
        let mut out = String::new();
        for (name, v) in ordered {
            let ty = v
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or(if v.get("items").is_some() {
                    "array"
                } else {
                    "any"
                });
            let mut spec = ty.to_string();
            if let Some(items) = v.get("items") {
                if let Some(t) = items.get("type").and_then(Value::as_str) {
                    spec = format!("array of {t}");
                }
            }
            if let Some(e) = v.get("enum").and_then(Value::as_array) {
                let choices: Vec<String> = e.iter().map(value_short).collect();
                spec = format!("one of {}", choices.join("|"));
            }
            let mut range = String::new();
            if let (Some(min), Some(max)) = (v.get("minimum"), v.get("maximum")) {
                range = format!(" {}–{}", value_short(min), value_short(max));
            } else if let Some(min) = v.get("minimum") {
                range = format!(" ≥{}", value_short(min));
            } else if let Some(max) = v.get("maximum") {
                range = format!(" ≤{}", value_short(max));
            }
            let default = v
                .get("default")
                .filter(|d| !d.is_null())
                .map(|d| format!(" (default {})", value_short(d)))
                .unwrap_or_default();
            let req = if required.contains(&name.as_str()) {
                " REQUIRED"
            } else {
                ""
            };
            let desc = v
                .get("description")
                .and_then(Value::as_str)
                .map(|d| format!(" — {}", d.trim()))
                .unwrap_or_default();
            out.push_str(&format!("  {name}: {spec}{range}{default}{req}{desc}\n"));
        }
        out.trim_end().to_string()
    }
}

fn value_short(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn usd(c: f64) -> String {
    crate::budget::fmt_usd(c)
}

fn grid_range(grid: &Map<String, Value>) -> (Option<f64>, Option<f64>) {
    let mut costs = Vec::new();
    for tier in grid.values() {
        match tier {
            Value::Object(m) => costs.extend(m.values().filter_map(Value::as_f64)),
            Value::Number(n) => costs.extend(n.as_f64()),
            _ => {}
        }
    }
    costs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (costs.first().copied(), costs.last().copied())
}

/// Estimate the cost of one output for `model` with `params` (the request
/// body minus references). `None` when the catalog carries no usable price.
pub fn estimate_cost(model: &MediaModel, params: &Map<String, Value>) -> Option<f64> {
    let p = &model.pricing;
    let param_or_default = |name: &str| -> Option<Value> {
        params
            .get(name)
            .cloned()
            .or_else(|| model.default_of(name).cloned())
    };
    match model.kind {
        MediaKind::Image => {
            if let Some(grid) = p.get("cost_per_image_grid").and_then(Value::as_object) {
                let quality = param_or_default("quality")
                    .and_then(|v| v.as_str().map(str::to_string))
                    .or_else(|| {
                        (grid.len() == 1)
                            .then(|| grid.keys().next().cloned())
                            .flatten()
                    });
                let resolution = param_or_default("resolution")
                    .map(|v| value_short(&v))
                    .or_else(|| param_or_default("size").map(|v| value_short(&v)));
                if let Some(tier) = quality.and_then(|q| lookup_ci(grid, &q)) {
                    match tier {
                        Value::Object(m) => {
                            if let Some(res) = resolution.as_deref() {
                                if let Some(c) = lookup_ci(m, res).and_then(Value::as_f64) {
                                    return Some(c);
                                }
                            }
                        }
                        Value::Number(n) => return n.as_f64(),
                        _ => {}
                    }
                }
            }
            p.get("cost_per_image").and_then(Value::as_f64)
        }
        MediaKind::Video => {
            let per_second = {
                let with_audio = param_or_default("generate_audio")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let audio_rate = if with_audio {
                    p.get("cost_per_second_with_audio").and_then(Value::as_f64)
                } else {
                    None
                };
                audio_rate
                    .or_else(|| {
                        let res = param_or_default("resolution").map(|v| value_short(&v))?;
                        p.get("cost_per_second_by_resolution")
                            .and_then(Value::as_object)
                            .and_then(|m| lookup_ci(m, &res))
                            .and_then(Value::as_f64)
                    })
                    .or_else(|| p.get("cost_per_second").and_then(Value::as_f64))?
            };
            let duration = duration_secs(model, params)?;
            Some(per_second * duration)
        }
    }
}

/// The duration a video request will bill for: the requested one, else the
/// schema default, and when the default means "auto" (`-1`, `"auto"`) the
/// schema maximum — overestimating asks the user; underestimating spends.
pub fn duration_secs(model: &MediaModel, params: &Map<String, Value>) -> Option<f64> {
    let numeric = |v: &Value| -> Option<f64> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    };
    if let Some(d) = params.get("duration").and_then(numeric) {
        if d > 0.0 {
            return Some(d);
        }
    }
    if let Some(d) = model.default_of("duration").and_then(numeric) {
        if d > 0.0 {
            return Some(d);
        }
    }
    let max = model
        .property("duration")
        .and_then(|p| p.get("maximum"))
        .and_then(numeric);
    let enum_max = model
        .property("duration")
        .and_then(|p| p.get("enum"))
        .and_then(Value::as_array)
        .and_then(|e| {
            e.iter()
                .filter_map(numeric)
                .fold(None, |acc: Option<f64>, x| {
                    Some(acc.map_or(x, |a| a.max(x)))
                })
        });
    max.or(enum_max).or(if model.has_param("duration") {
        None
    } else {
        Some(5.0)
    })
}

fn lookup_ci<'a>(m: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    m.get(key).or_else(|| {
        m.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    })
}

/// The cached/fetched catalog.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    models: Vec<MediaModel>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    fetched_at: u64,
    data: Vec<Value>,
}

impl Catalog {
    /// Parse the hub's `{"data": [...]}` (or a bare array), keeping only
    /// image/video models.
    pub fn from_json(v: &Value) -> Self {
        let raw = v
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default();
        Self::from_raw(&raw)
    }

    fn from_raw(raw: &[Value]) -> Self {
        let mut models: Vec<MediaModel> = raw.iter().filter_map(parse_model).collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Self { models }
    }

    pub fn models(&self) -> &[MediaModel] {
        &self.models
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&MediaModel> {
        let id = id.trim();
        self.models
            .iter()
            .find(|m| m.id == id)
            .or_else(|| self.models.iter().find(|m| m.id.eq_ignore_ascii_case(id)))
    }

    /// Models of `kind` matching every word of `query` (in id, name,
    /// developer, summary), best matches first.
    pub fn search(&self, kind: Option<MediaKind>, query: Option<&str>) -> Vec<&MediaModel> {
        let words: Vec<String> = query
            .unwrap_or("")
            .split_whitespace()
            .map(|w| w.to_ascii_lowercase())
            .collect();
        let mut hits: Vec<(i32, &MediaModel)> = self
            .models
            .iter()
            .filter(|m| kind.is_none_or(|k| m.kind == k))
            .filter_map(|m| {
                let hay = format!(
                    "{} {} {} {}",
                    m.id,
                    m.display_name.as_deref().unwrap_or(""),
                    m.developer.as_deref().unwrap_or(""),
                    m.summary.as_deref().unwrap_or("")
                )
                .to_ascii_lowercase();
                let mut score = 0;
                for w in &words {
                    if m.id.to_ascii_lowercase().contains(w) {
                        score += 3;
                    } else if hay.contains(w) {
                        score += 1;
                    } else {
                        return None;
                    }
                }
                Some((score, m))
            })
            .collect();
        hits.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then_with(|| a.id.cmp(&b.id)));
        hits.into_iter().map(|(_, m)| m).collect()
    }

    /// Models of `kind` that take references of `input`, for a "try one of
    /// these instead" hint.
    pub fn accepting(&self, kind: MediaKind, input: RefKind) -> Vec<&MediaModel> {
        self.models
            .iter()
            .filter(|m| m.kind == kind && m.accepts(input))
            .collect()
    }

    /// Load the catalog: the disk cache when fresh, else a fetch from
    /// `{base_url}/models` (cached afterwards); a stale cache when the hub
    /// is unreachable. `api_key` is optional — the endpoint is public.
    pub async fn load(
        http: &reqwest::Client,
        base_url: &str,
        api_key: Option<&str>,
        cache: Option<&Path>,
    ) -> Result<Self, String> {
        if let Some((age, cached)) = cache.and_then(read_cache) {
            if age < CACHE_TTL {
                return Ok(cached);
            }
        }
        match Self::refresh(http, base_url, api_key, cache).await {
            Ok(catalog) => Ok(catalog),
            Err(e) => match cache.and_then(read_cache) {
                Some((_, stale)) => {
                    tracing::warn!("media catalog fetch failed ({e}); using the cached copy");
                    Ok(stale)
                }
                None => Err(e),
            },
        }
    }

    /// Fetch the catalog from the hub regardless of the cache's age, and
    /// rewrite the cache. Unlike [`Catalog::load`] a failed fetch is an
    /// error: someone asking for the newest models must not be handed the
    /// old list as if it were fresh.
    pub async fn refresh(
        http: &reqwest::Client,
        base_url: &str,
        api_key: Option<&str>,
        cache: Option<&Path>,
    ) -> Result<Self, String> {
        let (catalog, raw) = Self::fetch(http, base_url, api_key).await?;
        if let Some(path) = cache {
            write_cache(path, raw);
        }
        Ok(catalog)
    }

    async fn fetch(
        http: &reqwest::Client,
        base_url: &str,
        api_key: Option<&str>,
    ) -> Result<(Self, Vec<Value>), String> {
        let url = format!("{}/models", base_url.trim_end_matches('/'));
        let mut req = http.get(&url).timeout(Duration::from_secs(30));
        if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
            req = req.bearer_auth(key);
        }
        let res = harness_http::send_with_retry(req, &harness_http::Backoff::brief(), true)
            .await
            .map_err(|e| format!("{url}: {e}"))?;
        if !res.status().is_success() {
            return Err(format!("{url}: HTTP {}", res.status()));
        }
        let v: Value = res.json().await.map_err(|e| format!("{url}: {e}"))?;
        let raw: Vec<Value> = v
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|m| {
                m.get("endpoint")
                    .and_then(Value::as_str)
                    .and_then(MediaKind::from_endpoint)
                    .is_some()
            })
            .collect();
        Ok((Self::from_raw(&raw), raw))
    }
}

fn parse_model(v: &Value) -> Option<MediaModel> {
    let id = v.get("id")?.as_str()?.to_string();
    let kind = MediaKind::from_endpoint(v.get("endpoint")?.as_str()?)?;
    let str_field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let inputs = v
        .get("capabilities")
        .and_then(|c| c.get("input"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(MediaModel {
        id,
        kind,
        display_name: str_field("display_name").or_else(|| str_field("name")),
        developer: str_field("developer"),
        summary: str_field("summary"),
        description: str_field("description"),
        inputs,
        pricing: v.get("pricing").cloned().unwrap_or(Value::Null),
        request_schema: v.get("request_schema").cloned().unwrap_or(Value::Null),
    })
}

fn read_cache(path: &Path) -> Option<(Duration, Catalog)> {
    let text = std::fs::read_to_string(path).ok()?;
    let file: CacheFile = serde_json::from_str(&text).ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let age = Duration::from_secs(now.saturating_sub(file.fetched_at));
    Some((age, Catalog::from_raw(&file.data)))
}

fn write_cache(path: &Path, data: Vec<Value>) {
    let fetched_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file = CacheFile { fetched_at, data };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(&file) {
        // A unique temp name: two sessions (or two pickers on one settings
        // page) refreshing at once must not interleave into one file.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp: PathBuf = path.with_extension(format!("{}-{nonce}.tmp", std::process::id()));
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({"data": [
            {"id": "flux-mini", "endpoint": "/images/generate", "summary": "Fast drafts",
             "developer": "bfl", "capabilities": {"input": ["text", "image"]},
             "pricing": {"method": "per_image", "cost_per_image": 0.01},
             "request_schema": {"required": ["prompt"], "properties": {
                "prompt": {"type": "string", "x-order": 0, "description": "Use @Image1 to refer to references."},
                "input_image": {"type": "array", "items": {"type": "string", "format": "uri"}, "x-order": 1},
                "aspect_ratio": {"type": "string", "enum": ["1:1", "16:9"], "default": "16:9", "x-order": 2},
                "seed": {"type": "integer", "x-order": 3}}}},
            {"id": "gpt-grid", "endpoint": "/images/generate",
             "pricing": {"method": "per_image", "cost_per_image": null,
                         "cost_per_image_grid": {"high": {"1K": 0.13, "2K": 0.29}, "low": {"1K": 0.004, "2K": 0.009}}},
             "request_schema": {"properties": {
                "prompt": {"type": "string"},
                "quality": {"type": "string", "enum": ["low", "high"], "default": "high"},
                "resolution": {"type": "string", "enum": ["1K", "2K"], "default": "2K"}}}},
            {"id": "seedance-fast", "endpoint": "/videos/generate", "capabilities": {"input": ["text","image","video","audio"]},
             "pricing": {"method": "per_video_output_second", "cost_per_second": 0.173,
                         "cost_per_second_by_resolution": {"480p": 0.077, "720p": 0.173}},
             "request_schema": {"properties": {
                "prompt": {"type": "string", "description": "@Image1, @Video1"},
                "input_images": {"type": "array", "items": {"type": "string"}},
                "input_videos": {"type": "array", "items": {"type": "string"}},
                "input_audios": {"type": "array", "items": {"type": "string"}},
                "duration": {"type": "integer", "minimum": 4, "maximum": 15, "default": -1},
                "resolution": {"type": "string", "enum": ["480p", "720p"], "default": "720p"}}}},
            {"id": "veo", "endpoint": "/videos/generate",
             "pricing": {"cost_per_second": 0.13, "cost_per_second_by_resolution": {"720p": 0.13, "1080p": 0.156}},
             "request_schema": {"properties": {
                "prompt": {"type": "string"},
                "input_image": {"type": "string"},
                "tail_image_url": {"type": "string"},
                "duration": {"type": "integer", "minimum": 4, "maximum": 8, "default": 8},
                "resolution": {"type": "string", "default": "720p"},
                "generate_audio": {"type": "boolean", "default": true}}}},
            {"id": "claude-x", "endpoint": "/chat/completions"}
        ]})
    }

    #[test]
    fn parses_only_media_models() {
        let c = Catalog::from_json(&sample());
        assert_eq!(c.models().len(), 4);
        assert!(c.get("claude-x").is_none());
        let flux = c.get("FLUX-MINI").unwrap();
        assert_eq!(flux.kind, MediaKind::Image);
        assert_eq!(flux.inputs, vec!["text", "image"]);
        assert_eq!(flux.price_label(), "$0.01/image");
        assert!(flux.prompt_uses_at_refs());
        assert_eq!(
            flux.param_names(),
            vec!["aspect_ratio", "input_image", "prompt", "seed"]
        );
    }

    #[test]
    fn slots_follow_the_schema() {
        let c = Catalog::from_json(&sample());
        let flux = c.get("flux-mini").unwrap();
        assert_eq!(
            flux.slots(RefKind::Image),
            vec![Slot {
                name: "input_image".into(),
                array: true
            }]
        );
        assert!(!flux.accepts(RefKind::Video));
        let veo = c.get("veo").unwrap();
        assert_eq!(
            veo.slots(RefKind::Image),
            vec![
                Slot {
                    name: "input_image".into(),
                    array: false
                },
                Slot {
                    name: "tail_image_url".into(),
                    array: false
                }
            ]
        );
        let sd = c.get("seedance-fast").unwrap();
        assert_eq!(sd.slots(RefKind::Audio)[0].name, "input_audios");
        assert_eq!(c.accepting(MediaKind::Video, RefKind::Audio).len(), 1);
    }

    #[test]
    fn estimates_flat_grid_and_per_second_prices() {
        let c = Catalog::from_json(&sample());
        let none = Map::new();
        assert_eq!(
            estimate_cost(c.get("flux-mini").unwrap(), &none),
            Some(0.01)
        );

        let grid = c.get("gpt-grid").unwrap();
        assert_eq!(estimate_cost(grid, &none), Some(0.29)); // defaults: high, 2K
        let mut low = Map::new();
        low.insert("quality".into(), json!("low"));
        low.insert("resolution".into(), json!("1K"));
        assert_eq!(estimate_cost(grid, &low), Some(0.004));
        assert_eq!(grid.price_label(), "$0.0040–$0.29/image");

        let sd = c.get("seedance-fast").unwrap();
        // default duration is "auto" (-1) → bill for the maximum, at 720p.
        let est = estimate_cost(sd, &none).unwrap();
        assert!((est - 0.173 * 15.0).abs() < 1e-9, "{est}");
        let mut short = Map::new();
        short.insert("duration".into(), json!(5));
        short.insert("resolution".into(), json!("480p"));
        let est = estimate_cost(sd, &short).unwrap();
        assert!((est - 0.077 * 5.0).abs() < 1e-9, "{est}");
        assert_eq!(sd.price_label(), "$0.08–$0.17/s");

        let veo = c.get("veo").unwrap();
        let est = estimate_cost(veo, &none).unwrap();
        assert!((est - 0.13 * 8.0).abs() < 1e-9, "{est}");
    }

    #[test]
    fn search_ranks_id_hits_first() {
        let c = Catalog::from_json(&sample());
        let hits = c.search(Some(MediaKind::Video), Some("seedance"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "seedance-fast");
        assert_eq!(c.search(Some(MediaKind::Image), None).len(), 2);
        assert!(c.search(None, Some("nothing-here")).is_empty());
        let fast = c.search(None, Some("fast drafts"));
        assert_eq!(fast[0].id, "flux-mini");
    }

    #[test]
    fn schema_summary_is_compact_and_ordered() {
        let c = Catalog::from_json(&sample());
        let s = c.get("flux-mini").unwrap().schema_summary();
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].starts_with("  prompt: string REQUIRED"), "{s}");
        assert!(
            lines[1].starts_with("  input_image: array of string"),
            "{s}"
        );
        assert!(lines[2].contains("one of 1:1|16:9 (default 16:9)"), "{s}");
    }

    #[tokio::test]
    async fn load_uses_cache_then_fetch_then_stale() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache/media-models.json");
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/api/ai/models")
            .with_body(sample().to_string())
            .expect(1)
            .create_async()
            .await;
        let http = reqwest::Client::new();
        let base = format!("{}/api/ai", server.url());
        let c = Catalog::load(&http, &base, None, Some(&cache))
            .await
            .unwrap();
        assert_eq!(c.models().len(), 4);
        assert!(cache.exists());
        // Second load is served from cache: the mock's expect(1) holds.
        let c2 = Catalog::load(&http, &base, None, Some(&cache))
            .await
            .unwrap();
        assert_eq!(c2.models().len(), 4);
        m.assert_async().await;

        // An expired cache with a dead hub falls back to the stale copy.
        let text = std::fs::read_to_string(&cache).unwrap();
        let mut file: CacheFile = serde_json::from_str(&text).unwrap();
        file.fetched_at = 1;
        std::fs::write(&cache, serde_json::to_string(&file).unwrap()).unwrap();
        let c3 = Catalog::load(&http, "http://127.0.0.1:9/api/ai", None, Some(&cache))
            .await
            .unwrap();
        assert_eq!(c3.models().len(), 4);
    }

    #[tokio::test]
    async fn refresh_refetches_a_fresh_cache_and_reports_a_dead_hub() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache/media-models.json");
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/api/ai/models")
            .with_body(sample().to_string())
            .expect(2)
            .create_async()
            .await;
        let http = reqwest::Client::new();
        let base = format!("{}/api/ai", server.url());
        Catalog::load(&http, &base, None, Some(&cache))
            .await
            .unwrap();
        // The cache is fresh, yet a refresh goes back to the hub.
        let c = Catalog::refresh(&http, &base, None, Some(&cache))
            .await
            .unwrap();
        assert_eq!(c.models().len(), 4);
        m.assert_async().await;

        // No stale fallback: the caller hears that the hub was unreachable.
        let dead = Catalog::refresh(&http, "http://127.0.0.1:9/api/ai", None, Some(&cache)).await;
        assert!(dead.is_err());
    }
}
