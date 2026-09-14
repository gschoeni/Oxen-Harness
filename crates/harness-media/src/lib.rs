//! Image and video generation as agent tools, over the Oxen.ai async queue.
//!
//! The model gets four tools:
//!
//! - `media_models` — search the hosted catalog (39 image / 80 video models at
//!   the time of writing) and read any model's request schema, so the agent
//!   can fill seedance / wan / kling / flux parameters itself.
//! - `generate_image` / `generate_video` — enqueue a generation, wait for it
//!   (images) or let it finish in the background (videos), download the
//!   result into the project's media library, and report it. Reference
//!   media the user dropped into the chat rides along as `refs` and lands in
//!   the right request field for the chosen model.
//! - `media_status` — what is in flight, and cancel.
//!
//! The pieces, mirroring `harness-preview`'s shape:
//!
//! - [`catalog`] — the media half of `GET /api/ai/models`, cached on disk,
//!   with per-model `request_schema` and pricing → cost estimates.
//! - [`queue`] — `POST /api/ai/queue`, polling, cancel, download.
//! - [`library`] — the per-project `generations/` folder and its
//!   append-only `manifest.jsonl`; a watch channel hosts forward as a
//!   whole-list event.
//! - [`refs`] — chip labels (`[Image #2]`) and paths → local files → data
//!   URIs, and the session registry both hosts stage attachments into.
//! - [`budget`] — the per-generation / per-run spend limits; anything under
//!   them runs, anything over asks the user through the host's
//!   [`MediaSink`].
//! - [`tools`] — the `TypedTool`s.
//!
//! Hosts build one [`MediaContext`] per session with [`session_tools`] and
//! register what it returns. There is no default-registry entry: the tools
//! need a session, a workspace, and a sink.

pub mod budget;
pub mod catalog;
pub mod events;
pub mod library;
pub mod queue;
pub mod refs;
pub mod tools;
pub mod upload;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use budget::{BudgetVerdict, MediaBudget, SpendEstimate};
pub use catalog::{Catalog, MediaModel};
pub use events::{Completion, CompletionFeed};
pub use library::{MediaItem, MediaLibrary, MediaStatus, MediaTick, MediaUpload, UploadStatus};
pub use queue::{GenerationRecord, QueueClient, QueueError};
pub use refs::{MediaRefs, RefKind, ResolvedRef};
pub use tools::{
    GenerateImageTool, GenerateVideoTool, MediaContext, MediaModelsTool, MediaStatusTool,
};
pub use upload::{HubUploader, UploadError, UploadedRef};

/// Tool names the model calls (and front ends special-case for rendering).
pub const GENERATE_IMAGE_TOOL: &str = "generate_image";
pub const GENERATE_VIDEO_TOOL: &str = "generate_video";
pub const MEDIA_MODELS_TOOL: &str = "media_models";
pub const MEDIA_STATUS_TOOL: &str = "media_status";

/// The User-Agent every hub request sends. Deliberately free of the word
/// "oxen": the hub treats such agents as the Oxen CLI and gates them on
/// its version (HTTP 426), which broke result downloads.
pub const USER_AGENT: &str = "harness-media/0.1";

/// The stable leading sentence of a generation tool's result when no API key
/// is configured, so both front ends can offer their "connect your account"
/// prompt (the same pattern as `harness_tools::web::WEB_SEARCH_NO_KEY`).
pub const MEDIA_NO_KEY: &str = "Media generation needs your Oxen API key.";

/// What a model produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }

    /// The catalog endpoint that serves this kind.
    pub fn endpoint(self) -> &'static str {
        match self {
            Self::Image => "/images/generate",
            Self::Video => "/videos/generate",
        }
    }

    pub fn from_endpoint(endpoint: &str) -> Option<Self> {
        match endpoint {
            "/images/generate" => Some(Self::Image),
            "/videos/generate" => Some(Self::Video),
            _ => None,
        }
    }

    /// The API's `media_type` string for this kind.
    pub fn from_media_type(media_type: &str) -> Option<Self> {
        match media_type {
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            _ => None,
        }
    }
}

impl std::fmt::Display for MediaKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where generated files go by default, relative to the project root.
pub const DEFAULT_OUTPUT_DIR: &str = "generations";
/// The cheap draft image model a fresh install reaches for.
pub const DEFAULT_IMAGE_MODEL: &str = "black-forest-labs-flux-2-klein-4b";
/// The default video model: fast, 720p, priced per second.
pub const DEFAULT_VIDEO_MODEL: &str = "bytedance-seedance-2-0-fast-text-to-video";
/// Below this estimated cost per output, a generation runs without asking.
pub const DEFAULT_PER_GENERATION_USD: f64 = 0.25;
/// Below this estimated cost per tool call, a run proceeds without asking.
pub const DEFAULT_PER_RUN_USD: f64 = 1.0;

/// The user's media preferences (`~/.oxen-harness/media.json`; persisted by
/// `harness_runtime::media`). Every field has a default so a fresh install
/// works, and none is a secret — the API key is the connection's.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaPrefs {
    /// The image model used when the agent doesn't name one.
    #[serde(default = "MediaPrefs::default_image_model")]
    pub default_image_model: String,
    /// The video model used when the agent doesn't name one.
    #[serde(default = "MediaPrefs::default_video_model")]
    pub default_video_model: String,
    /// Project-relative folder generated files are saved under.
    #[serde(default = "MediaPrefs::default_output_dir")]
    pub output_dir: String,
    /// A single output whose estimate is at or under this runs without
    /// asking. `None` means every generation asks first.
    #[serde(default = "MediaPrefs::default_per_generation")]
    pub per_generation_usd: Option<f64>,
    /// A tool call whose total estimate is at or under this runs without
    /// asking. `None` means every run asks first.
    #[serde(default = "MediaPrefs::default_per_run")]
    pub per_run_usd: Option<f64>,
    /// `namespace/repo` on the hub that also keeps a copy of every
    /// generation (the queue's `target_repo`), under the output folder.
    /// `None` keeps files only in the project.
    #[serde(default)]
    pub hub_repo: Option<String>,
    /// When the project is an Oxen repo, add and commit each finished batch.
    #[serde(default)]
    pub commit_with_oxen: bool,
}

impl MediaPrefs {
    fn default_image_model() -> String {
        DEFAULT_IMAGE_MODEL.to_string()
    }
    fn default_video_model() -> String {
        DEFAULT_VIDEO_MODEL.to_string()
    }
    fn default_output_dir() -> String {
        DEFAULT_OUTPUT_DIR.to_string()
    }
    fn default_per_generation() -> Option<f64> {
        Some(DEFAULT_PER_GENERATION_USD)
    }
    fn default_per_run() -> Option<f64> {
        Some(DEFAULT_PER_RUN_USD)
    }

    /// The hub repo split into `(namespace, repo)`, when set and well-formed.
    pub fn hub_target(&self) -> Option<(String, String)> {
        let raw = self.hub_repo.as_deref()?.trim().trim_matches('/');
        let (ns, repo) = raw.split_once('/')?;
        (!ns.is_empty() && !repo.is_empty() && !repo.contains('/'))
            .then(|| (ns.to_string(), repo.to_string()))
    }

    /// The budget these preferences describe.
    pub fn budget(&self) -> MediaBudget {
        MediaBudget {
            per_generation_usd: self.per_generation_usd,
            per_run_usd: self.per_run_usd,
        }
    }

    /// The output folder, normalized: relative, `/`-separated, no `..`.
    /// Anything unusable falls back to the default so a bad setting can
    /// never write outside the project.
    pub fn output_dir_rel(&self) -> String {
        let raw = self.output_dir.trim().trim_matches('/').replace('\\', "/");
        let clean = raw
            .split('/')
            .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
            .collect::<Vec<_>>()
            .join("/");
        if clean.is_empty() || PathBuf::from(&self.output_dir).is_absolute() {
            DEFAULT_OUTPUT_DIR.to_string()
        } else {
            clean
        }
    }
}

impl Default for MediaPrefs {
    fn default() -> Self {
        Self {
            default_image_model: Self::default_image_model(),
            default_video_model: Self::default_video_model(),
            output_dir: Self::default_output_dir(),
            per_generation_usd: Self::default_per_generation(),
            per_run_usd: Self::default_per_run(),
            hub_repo: None,
            commit_with_oxen: false,
        }
    }
}

/// How the tools reach the hub: the `/api/ai` base URL the LLM client uses
/// and the one API key the user configured for the harness.
#[derive(Debug, Clone)]
pub struct MediaApi {
    pub base_url: String,
    pub api_key: String,
}

/// The host side of a spend decision. Returns `Ok(Some(true))` to proceed,
/// `Ok(Some(false))` when the user declined, and `Ok(None)` when there is no
/// interactive user to ask (a subagent, a piped session) — the tool then
/// refuses rather than spending.
#[async_trait]
pub trait MediaSink: Send + Sync {
    async fn confirm_spend(
        &self,
        estimate: &SpendEstimate,
    ) -> Result<Option<bool>, harness_tools::ToolError>;
}

/// A [`MediaSink`] that asks through the host's existing question picker
/// (`ask_user_question`'s [`harness_tools::QuestionAsker`]), so both front
/// ends get a spend prompt with no new UI.
pub struct AskerSpendConfirm(pub Arc<dyn harness_tools::QuestionAsker>);

#[async_trait]
impl MediaSink for AskerSpendConfirm {
    async fn confirm_spend(
        &self,
        estimate: &SpendEstimate,
    ) -> Result<Option<bool>, harness_tools::ToolError> {
        let question = harness_tools::Question {
            question: estimate.question(),
            header: "Spend".to_string(),
            options: vec![
                harness_tools::Choice {
                    label: "Generate".to_string(),
                    description: format!("Run it ({})", estimate.total_label()),
                },
                harness_tools::Choice {
                    label: "Skip".to_string(),
                    description: "Don't generate; the agent will adjust".to_string(),
                },
            ],
            multi_select: false,
        };
        let Some(answers) = self.0.ask(std::slice::from_ref(&question)).await? else {
            return Ok(None);
        };
        let picked = answers
            .first()
            .and_then(|a| a.selected.first())
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        Ok(Some(
            picked == "generate" || picked == "yes" || picked == "y" || picked == "ok",
        ))
    }
}

/// A sink for hosts with no user to ask: every over-budget run is refused.
pub struct NoConfirmSink;

#[async_trait]
impl MediaSink for NoConfirmSink {
    async fn confirm_spend(
        &self,
        _estimate: &SpendEstimate,
    ) -> Result<Option<bool>, harness_tools::ToolError> {
        Ok(None)
    }
}

/// Build the session's tool set. Register all four; the prompt gates on
/// `generate_image` being present.
pub fn session_tools(
    ctx: MediaContext,
) -> (
    MediaModelsTool,
    GenerateImageTool,
    GenerateVideoTool,
    MediaStatusTool,
) {
    let ctx = Arc::new(ctx);
    (
        MediaModelsTool { ctx: ctx.clone() },
        GenerateImageTool { ctx: ctx.clone() },
        GenerateVideoTool { ctx: ctx.clone() },
        MediaStatusTool { ctx },
    )
}

/// `~/.oxen-harness/cache/media-models-<host>.json` — the cached media
/// catalog for one hub (a local dev hub and hub.oxen.ai serve different
/// catalogs). Regenerable, so it lives under `cache/`.
pub fn catalog_cache_path(base_url: &str) -> Option<PathBuf> {
    let host: String = base_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("hub")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    harness_config::paths::cache_dir()
        .ok()
        .map(|d| d.join(format!("media-models-{host}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_dir_never_escapes_the_project() {
        let mut prefs = MediaPrefs::default();
        assert_eq!(prefs.output_dir_rel(), "generations");
        prefs.output_dir = "../../etc".into();
        assert_eq!(prefs.output_dir_rel(), "etc");
        prefs.output_dir = "/abs/path".into();
        assert_eq!(prefs.output_dir_rel(), "generations");
        prefs.output_dir = " assets/gen/ ".into();
        assert_eq!(prefs.output_dir_rel(), "assets/gen");
        prefs.output_dir = "..".into();
        assert_eq!(prefs.output_dir_rel(), "generations");
    }

    #[test]
    fn prefs_round_trip_with_defaults_filled_in() {
        let parsed: MediaPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, MediaPrefs::default());
        let parsed: MediaPrefs =
            serde_json::from_str(r#"{"per_run_usd": null, "output_dir": "art"}"#).unwrap();
        assert_eq!(parsed.per_run_usd, None);
        assert_eq!(parsed.output_dir, "art");
        assert_eq!(parsed.per_generation_usd, Some(DEFAULT_PER_GENERATION_USD));
    }

    #[test]
    fn hub_target_parses_namespace_and_repo() {
        let mut prefs = MediaPrefs::default();
        assert_eq!(prefs.hub_target(), None);
        prefs.hub_repo = Some(" ox/generations/ ".into());
        assert_eq!(
            prefs.hub_target(),
            Some(("ox".into(), "generations".into()))
        );
        prefs.hub_repo = Some("just-a-name".into());
        assert_eq!(prefs.hub_target(), None);
    }

    #[test]
    fn kinds_map_to_endpoints() {
        assert_eq!(
            MediaKind::from_endpoint("/images/generate"),
            Some(MediaKind::Image)
        );
        assert_eq!(
            MediaKind::from_endpoint("/videos/generate"),
            Some(MediaKind::Video)
        );
        assert_eq!(MediaKind::from_endpoint("/chat/completions"), None);
        assert_eq!(MediaKind::Video.endpoint(), "/videos/generate");
    }
}
