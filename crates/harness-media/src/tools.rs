//! The model-facing tools: `media_models`, `generate_image`,
//! `generate_video`, `media_status`.
//!
//! A generation call: resolve the model (named, or the user's default) →
//! resolve references (chip labels, paths; labels mentioned in the prompt
//! are picked up automatically) into the model's own request fields →
//! build and validate the request against the catalog schema → estimate
//! the cost and consult the budget (asking the user through the host when
//! over) → enqueue → wait (images) or hand the wait to a background task
//! whose report lands as an aside (videos) → download into the library →
//! report paths, with the images themselves attached so the model can
//! look at what it made.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use harness_tools::{Asides, CallContext, Concurrency, ToolError, TypedTool};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::budget::{fmt_usd, BudgetVerdict, SpendEstimate};
use crate::catalog::{estimate_cost, Catalog, MediaModel};
use crate::library::{
    now_unix, slugify, MediaItem, MediaLibrary, MediaProgress, MediaSource, MediaStatus,
    SourceOrigin, StoredRef,
};
use crate::local::{LocalJob, LocalMedia, LocalMediaError, LocalProgress};
use crate::queue::{deadline_for, QueueClient, QueueError};
use crate::refs::{self, MediaRefs, RefKind, ResolvedRef};
use crate::{
    MediaApi, MediaKind, MediaPrefs, MediaSink, GENERATE_IMAGE_TOOL, GENERATE_VIDEO_TOOL,
    MEDIA_MODELS_TOOL, MEDIA_NO_KEY, MEDIA_STATUS_TOOL,
};

/// At most this many outputs per call (the hub's `num_generations` cap).
pub const MAX_COUNT: u32 = 4;

/// Everything the four tools share for one session.
/// Where generations go right now, as `(namespace, repo)`; see
/// [`MediaContext::with_destination`].
pub type HubTarget = Arc<dyn Fn() -> Option<(String, String)> + Send + Sync>;

pub struct MediaContext {
    pub session: String,
    pub root: PathBuf,
    pub prefs: MediaPrefs,
    /// `None` (or an empty key) → the generation tools answer with
    /// [`MEDIA_NO_KEY`] instead of calling the hub.
    pub api: Option<MediaApi>,
    pub refs: Arc<MediaRefs>,
    pub library: Arc<MediaLibrary>,
    pub sink: Arc<dyn MediaSink>,
    /// Where a background video's report goes; without it `wait: false`
    /// falls back to waiting inline.
    pub asides: Option<Asides>,
    /// On-device models; `None` when the host installed no provider.
    pub local: Option<Arc<dyn LocalMedia>>,
    batch_prefix: Option<String>,
    /// Re-reads the hub destination at each enqueue, so switching it applies
    /// to a chat that is already open; without it, `prefs.hub_repo` decides.
    destination: Option<HubTarget>,
    hub_catalog: tokio::sync::OnceCell<Arc<Catalog>>,
    feed: std::sync::OnceLock<Arc<crate::events::CompletionFeed>>,
    uploader: std::sync::OnceLock<Arc<crate::upload::HubUploader>>,
    http: reqwest::Client,
}

impl MediaContext {
    pub fn new(
        session: impl Into<String>,
        root: impl Into<PathBuf>,
        prefs: MediaPrefs,
        api: Option<MediaApi>,
        refs: Arc<MediaRefs>,
        library: Arc<MediaLibrary>,
        sink: Arc<dyn MediaSink>,
    ) -> Self {
        Self {
            session: session.into(),
            root: root.into(),
            prefs,
            api,
            refs,
            library,
            sink,
            asides: None,
            local: crate::local::installed(),
            batch_prefix: None,
            destination: None,
            hub_catalog: tokio::sync::OnceCell::new(),
            feed: std::sync::OnceLock::new(),
            uploader: std::sync::OnceLock::new(),
            http: harness_http::client(),
        }
    }

    /// Resolve the hub destination fresh for every generation.
    pub fn with_destination(mut self, resolve: HubTarget) -> Self {
        self.destination = Some(resolve);
        self
    }

    /// The `(namespace, repo)` generations and their references go to now;
    /// `None` leaves it to the hub (the account's `playground`).
    pub fn hub_target(&self) -> Option<(String, String)> {
        match &self.destination {
            Some(resolve) => resolve(),
            None => self.prefs.hub_target(),
        }
    }

    /// Associate exactly this node's paid jobs with its durable run record.
    pub fn with_batch_prefix(mut self, prefix: String) -> Self {
        self.batch_prefix = Some(prefix);
        self
    }

    /// Run one generation for a workflow node rather than a model tool
    /// call; `call` is whatever the workflow knows about who asked (at least
    /// the session), so the output still traces back.
    pub async fn generate_workflow(
        self: &Arc<Self>,
        kind: MediaKind,
        prompt: String,
        model: String,
        refs: Vec<String>,
        parameters: Map<String, Value>,
        call: &CallContext,
    ) -> Result<Vec<MediaItem>, ToolError> {
        let prefix = self.batch_prefix.as_ref().ok_or_else(|| {
            ToolError::Execution("workflow generation needs a unique batch prefix".into())
        })?;
        let report = run_generation(
            self,
            GenerateRequest {
                kind,
                prompt,
                model: Some(model),
                refs,
                typed: vec![],
                count: 1,
                name: None,
                parent: None,
                extra: parameters,
                wait: true,
            },
            call,
        )
        .await?;
        let items: Vec<_> = self
            .library
            .items()
            .into_iter()
            .filter(|item| item.batch.starts_with(&format!("{prefix}:")))
            .collect();
        if items.is_empty() {
            return Err(ToolError::Execution(report));
        }
        Ok(items)
    }

    pub fn with_asides(mut self, asides: Asides) -> Self {
        self.asides = Some(asides);
        self
    }

    /// Point the hosted catalog at a fixed copy (tests, offline).
    pub fn with_catalog(self, catalog: Catalog) -> Self {
        let _ = self.hub_catalog.set(Arc::new(catalog));
        self
    }

    /// Use `local` for on-device models instead of the process-wide one.
    pub fn with_local(mut self, local: Option<Arc<dyn LocalMedia>>) -> Self {
        self.local = local;
        self
    }

    fn api(&self) -> Option<&MediaApi> {
        self.api.as_ref().filter(|a| !a.api_key.trim().is_empty())
    }

    /// Hosted models plus local ones. The local half is read fresh every
    /// time (a model the user just pulled is usable at once); when the hub
    /// is unreachable the local models still answer.
    pub async fn catalog(&self) -> Result<Arc<Catalog>, ToolError> {
        let local = self.local.as_ref().map(|l| l.models()).unwrap_or_default();
        match self.hub_catalog().await {
            Ok(hub) if local.is_empty() => Ok(hub),
            Ok(hub) => Ok(Arc::new(hub.merged(local))),
            Err(e) if !local.is_empty() => {
                tracing::warn!("{e}; offering only local media models");
                Ok(Arc::new(Catalog::from_models(local)))
            }
            Err(e) => Err(e),
        }
    }

    async fn hub_catalog(&self) -> Result<Arc<Catalog>, ToolError> {
        self.hub_catalog
            .get_or_try_init(|| async {
                let base = self
                    .api
                    .as_ref()
                    .map(|a| a.base_url.clone())
                    .unwrap_or_else(|| harness_core::DEFAULT_BASE_URL.to_string());
                let key = self.api().map(|a| a.api_key.as_str());
                let cache = crate::catalog_cache_path(&base);
                Catalog::load(&self.http, &base, key, cache.as_deref())
                    .await
                    .map(Arc::new)
                    .map_err(|e| {
                        ToolError::Execution(format!("could not load the model catalog: {e}"))
                    })
            })
            .await
            .cloned()
    }

    fn queue(&self) -> Option<QueueClient> {
        self.api()
            .map(|a| QueueClient::new(&a.base_url, a.api_key.clone()))
    }

    /// The hub uploader for reference media. Each upload is told the
    /// current [`MediaContext::hub_target`]; without one it falls back to
    /// the account's playground.
    fn uploader(&self) -> Option<Arc<crate::upload::HubUploader>> {
        let api = self.api()?;
        Some(
            self.uploader
                .get_or_init(|| {
                    let api_root = crate::queue::api_root_from_base(&api.base_url);
                    Arc::new(crate::upload::HubUploader::new(
                        self.http.clone(),
                        api_root.clone(),
                        api.api_key.clone(),
                        None,
                        crate::upload::upload_cache_path(&api_root),
                    ))
                })
                .clone(),
        )
    }

    /// The session's completion feed, opened on first use (before the first
    /// enqueue, so no event is missed).
    fn feed(&self) -> Option<Arc<crate::events::CompletionFeed>> {
        let api = self.api()?;
        Some(
            self.feed
                .get_or_init(|| {
                    crate::events::CompletionFeed::start(
                        self.http.clone(),
                        crate::queue::api_root_from_base(&api.base_url),
                        api.api_key.clone(),
                    )
                })
                .clone(),
        )
    }
}

/// The "no key" result both front ends match on.
fn no_key_result() -> String {
    format!(
        "{MEDIA_NO_KEY} Ask the user to add their key in Settings → Connection \
         (or `OXEN_API_KEY`), then try again."
    )
}

// ---- media_models ---------------------------------------------------------

pub struct MediaModelsTool {
    pub(crate) ctx: Arc<MediaContext>,
}

/// Arguments to `media_models`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct MediaModelsArgs {
    /// Restrict to image or video models.
    #[serde(default)]
    pub kind: Option<MediaKind>,
    /// Words to match against model ids, names, and summaries
    /// (e.g. "seedance", "kling reference", "upscale").
    #[serde(default)]
    pub query: Option<String>,
    /// A model id: returns its full parameter schema and price instead
    /// of a listing. Do this before calling a model you haven't used.
    #[serde(default)]
    pub id: Option<String>,
}

#[async_trait]
impl TypedTool for MediaModelsTool {
    const NAME: &'static str = MEDIA_MODELS_TOOL;
    type Args = MediaModelsArgs;

    fn description(&self) -> &str {
        "Look up image and video generation models: hosted on Oxen.ai, plus \
         `local/` models that run free on this machine. With `query`/`kind`, \
         lists matching models with price and inputs; with `id`, returns that \
         model's exact request parameters (choices, ranges, defaults) so you can \
         fill `extra` correctly in generate_image/generate_video. Read a model's \
         schema before using one for the first time. Free — no generation happens."
    }

    async fn run(&self, args: MediaModelsArgs, _call: &CallContext) -> Result<String, ToolError> {
        let catalog = self.ctx.catalog().await?;
        if let Some(id) = args.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let Some(model) = catalog.get(id) else {
                let near = catalog
                    .search(None, Some(id))
                    .into_iter()
                    .take(5)
                    .map(|m| m.id.clone())
                    .collect::<Vec<_>>();
                return Err(ToolError::InvalidArguments(if near.is_empty() {
                    format!("no model `{id}` in the catalog; search with `query` first")
                } else {
                    format!("no model `{id}`; did you mean: {}", near.join(", "))
                }));
            };
            return Ok(describe_model(model));
        }
        let hits = catalog.search(args.kind, args.query.as_deref());
        if hits.is_empty() {
            return Ok(format!(
                "No {} models match {:?}. Try fewer words, or list everything with no query.",
                args.kind.map(|k| k.as_str()).unwrap_or("image or video"),
                args.query.unwrap_or_default()
            ));
        }
        let shown = hits.len().min(30);
        let mut out = format!(
            "{} model(s){}{}:\n",
            hits.len(),
            args.kind.map(|k| format!(" for {k}")).unwrap_or_default(),
            if hits.len() > shown {
                format!(" (showing {shown})")
            } else {
                String::new()
            }
        );
        for m in hits.iter().take(shown) {
            out.push_str("- ");
            out.push_str(&m.one_line());
            out.push('\n');
        }
        out.push_str("Defaults: image `");
        out.push_str(&self.ctx.prefs.default_image_model);
        out.push_str("`, video `");
        out.push_str(&self.ctx.prefs.default_video_model);
        out.push_str("`. Call media_models with `id` for a model's parameters.");
        Ok(out)
    }
}

fn describe_model(model: &MediaModel) -> String {
    let mut out = format!(
        "{} ({} model, {})",
        model.id,
        model.kind,
        model.price_label()
    );
    if let Some(dev) = &model.developer {
        out.push_str(&format!(" by {dev}"));
    }
    out.push('\n');
    if let Some(s) = model.summary.as_deref().or(model.description.as_deref()) {
        out.push_str(s.trim());
        out.push('\n');
    }
    if !model.inputs.is_empty() {
        out.push_str(&format!("Inputs: {}\n", model.inputs.join(", ")));
    }
    let mut takes = Vec::new();
    for kind in [RefKind::Image, RefKind::Video, RefKind::Audio] {
        let slots = model.slots(kind);
        if !slots.is_empty() {
            takes.push(format!(
                "{} → {}",
                kind.word().to_ascii_lowercase(),
                slots
                    .iter()
                    .map(|s| format!("{}{}", s.name, if s.array { "[]" } else { "" }))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if !takes.is_empty() {
        out.push_str(&format!(
            "References: pass them as `refs`; they land in {}.{}\n",
            takes.join("; "),
            if model.prompt_uses_at_refs() {
                " Address them in the prompt as @Image1, @Video1, @Audio1."
            } else {
                ""
            }
        ));
    }
    let defaults: Vec<String> = [RefKind::Image, RefKind::Video, RefKind::Audio]
        .into_iter()
        .flat_map(|k| model.slots(k))
        .map(|s| s.name)
        .collect();
    let others: Vec<String> = model
        .media_fields()
        .into_iter()
        .filter(|f| !defaults.contains(&f.name))
        .map(|f| format!("{}{}", f.name, if f.array { "[]" } else { "" }))
        .collect();
    if !others.is_empty() {
        out.push_str(&format!(
            "Other media fields: {}. Fill them in `extra` with the same labels or project \
             paths `refs` takes; they're uploaded the same way.\n",
            others.join(", ")
        ));
    }
    out.push_str("Parameters (typed ones go in the tool's own fields; the rest in `extra`):\n");
    out.push_str(&model.schema_summary());
    out
}

// ---- generate_image / generate_video ---------------------------------------

/// Arguments to `generate_image`.
// Field docs are the schema text the model reads, so the `[Image #N]` chip
// labels stay verbatim rather than being escaped for rustdoc.
#[allow(rustdoc::broken_intra_doc_links)]
#[derive(Deserialize, schemars::JsonSchema)]
pub struct GenerateImageArgs {
    /// What to generate, written for an image model: subject, style,
    /// composition, lighting, medium. Address reference images as
    /// @Image1, @Image2 (or by their [Image #N] label).
    pub prompt: String,
    /// Model id from media_models. Omit for the user's default image model.
    #[serde(default)]
    pub model: Option<String>,
    /// Reference media: the user's attachment labels ("[Image #1]") or
    /// project paths (an earlier generation). Labels mentioned in the
    /// prompt are included automatically.
    #[serde(default)]
    pub refs: Option<Vec<String>>,
    /// e.g. "1:1", "16:9", "9:16" — only values the model lists.
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// Fix the seed to reproduce or vary a result deterministically.
    #[serde(default)]
    pub seed: Option<u64>,
    /// How many variations to make in this call, 1–4 (each is billed).
    #[serde(default)]
    pub count: Option<u32>,
    /// A short name for the files (defaults to a slug of the prompt).
    #[serde(default)]
    pub name: Option<String>,
    /// The generation this varies, upscales, or edits (its project path),
    /// so the gallery shows lineage.
    #[serde(default)]
    pub parent: Option<String>,
    /// Any other parameter from the model's schema (see media_models),
    /// e.g. {"resolution": "2K", "quality": "low", "negative_prompt": "text"}.
    #[serde(default)]
    pub extra: Option<Map<String, Value>>,
}

/// Arguments to `generate_video`.
// Field docs are the schema text the model reads, so the `[Image #N]` chip
// labels stay verbatim rather than being escaped for rustdoc.
#[allow(rustdoc::broken_intra_doc_links)]
#[derive(Deserialize, schemars::JsonSchema)]
pub struct GenerateVideoArgs {
    /// What happens in the clip, written for a video model: subject,
    /// camera, motion, and for multi-beat clips a shot list with timings
    /// ("[0-3s] …, [3-6s] …"). Address references as @Image1 / @Video1 /
    /// @Audio1 (or by their [Image #N] labels).
    pub prompt: String,
    /// Model id from media_models. Omit for the user's default video model.
    #[serde(default)]
    pub model: Option<String>,
    /// Reference media: attachment labels ("[Image #1]", "[Video #1]",
    /// "[Audio #1]") or project paths. A first-frame image, a style/motion
    /// reference video, a soundtrack — placed in the model's matching
    /// fields. Labels mentioned in the prompt are included automatically.
    #[serde(default)]
    pub refs: Option<Vec<String>>,
    /// Clip length in seconds, within the model's range.
    #[serde(default)]
    pub duration: Option<f64>,
    /// e.g. "480p", "720p", "1080p" — only values the model lists. Lower is
    /// much cheaper; use it for drafts.
    #[serde(default)]
    pub resolution: Option<String>,
    /// e.g. "16:9", "9:16".
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// Generate a native soundtrack (models that support it; costs more).
    #[serde(default)]
    pub audio: Option<bool>,
    #[serde(default)]
    pub seed: Option<u64>,
    /// How many variations, 1–4 (each is billed).
    #[serde(default)]
    pub count: Option<u32>,
    /// A short name for the files (defaults to a slug of the prompt).
    #[serde(default)]
    pub name: Option<String>,
    /// The generation this extends, edits, or animates (its project path).
    #[serde(default)]
    pub parent: Option<String>,
    /// Any other parameter from the model's schema (see media_models),
    /// e.g. {"video_mode": "reference", "negative_prompt": "blurry"}.
    #[serde(default)]
    pub extra: Option<Map<String, Value>>,
    /// `false` (the default) queues the clip and returns at once; the
    /// result is delivered to you automatically when it finishes, so keep
    /// working and never poll. `true` waits for it (minutes).
    #[serde(default)]
    pub wait: Option<bool>,
}

/// The common request the two tools reduce to.
#[derive(Debug)]
struct GenerateRequest {
    kind: MediaKind,
    prompt: String,
    model: Option<String>,
    refs: Vec<String>,
    /// Typed params in tool-argument names → request field names.
    typed: Vec<(&'static str, Value)>,
    count: u32,
    name: Option<String>,
    parent: Option<String>,
    extra: Map<String, Value>,
    wait: bool,
}

pub struct GenerateImageTool {
    pub(crate) ctx: Arc<MediaContext>,
}

#[async_trait]
impl TypedTool for GenerateImageTool {
    const NAME: &'static str = GENERATE_IMAGE_TOOL;
    type Args = GenerateImageArgs;

    fn description(&self) -> &str {
        "Generate images with an Oxen.ai image model and save them into the \
         project's media library; you receive the images and can look at them. \
         Hosted models cost money per image (the tool checks the user's budget and \
         asks them when a call is over it); `local/` models run free on this \
         machine but slower. Use `refs` for reference/edit inputs the user \
         attached ([Image #N]) or earlier generations by path. Draft cheaply \
         (the default model, count 1–2), then render the chosen direction with \
         a stronger model. Read media_models with `id` before using unfamiliar \
         parameters in `extra`."
    }

    fn concurrency(&self) -> Concurrency {
        Concurrency::Exclusive
    }

    async fn run(&self, a: GenerateImageArgs, call: &CallContext) -> Result<String, ToolError> {
        let mut typed = Vec::new();
        if let Some(v) = a.aspect_ratio {
            typed.push(("aspect_ratio", Value::String(v)));
        }
        if let Some(v) = a.seed {
            typed.push(("seed", Value::from(v)));
        }
        run_generation(
            &self.ctx,
            GenerateRequest {
                kind: MediaKind::Image,
                prompt: a.prompt,
                model: a.model,
                refs: a.refs.unwrap_or_default(),
                typed,
                count: a.count.unwrap_or(1),
                name: a.name,
                parent: a.parent,
                extra: a.extra.unwrap_or_default(),
                wait: true,
            },
            call,
        )
        .await
    }
}

pub struct GenerateVideoTool {
    pub(crate) ctx: Arc<MediaContext>,
}

#[async_trait]
impl TypedTool for GenerateVideoTool {
    const NAME: &'static str = GENERATE_VIDEO_TOOL;
    type Args = GenerateVideoArgs;

    fn description(&self) -> &str {
        "Generate a video clip with an Oxen.ai video model and save it into the \
         project's media library. Billed per second of output (the tool checks \
         the user's budget and asks them when a call is over it). By default the \
         clip renders in the background and its result is delivered to you when \
         done — keep working, don't poll. Use `refs` for a first-frame image, a \
         reference video, or audio the user attached ([Image #N], [Video #N], \
         [Audio #N]), or earlier generations by path. You can't watch the \
         result; you get a poster frame when one can be extracted. Read \
         media_models with `id` before using a model's `extra` parameters."
    }

    fn concurrency(&self) -> Concurrency {
        Concurrency::Exclusive
    }

    async fn run(&self, a: GenerateVideoArgs, call: &CallContext) -> Result<String, ToolError> {
        let mut typed = Vec::new();
        if let Some(v) = a.duration {
            typed.push(("duration", Value::from(v)));
        }
        if let Some(v) = a.resolution {
            typed.push(("resolution", Value::String(v)));
        }
        if let Some(v) = a.aspect_ratio {
            typed.push(("aspect_ratio", Value::String(v)));
        }
        if let Some(v) = a.audio {
            typed.push(("generate_audio", Value::Bool(v)));
        }
        if let Some(v) = a.seed {
            typed.push(("seed", Value::from(v)));
        }
        run_generation(
            &self.ctx,
            GenerateRequest {
                kind: MediaKind::Video,
                prompt: a.prompt,
                model: a.model,
                refs: a.refs.unwrap_or_default(),
                typed,
                count: a.count.unwrap_or(1),
                name: a.name,
                parent: a.parent,
                extra: a.extra.unwrap_or_default(),
                wait: a.wait.unwrap_or(false),
            },
            call,
        )
        .await
    }
}

/// A prepared, validated request: everything but the spend decision.
#[derive(Debug)]
struct Prepared {
    model: MediaModel,
    /// The request body minus the prompt and references (added after the
    /// references are uploaded).
    body: Map<String, Value>,
    /// The parameters worth recording (no prompt, no reference data).
    params: Map<String, Value>,
    /// The references to upload, already checked against the model's slots.
    resolved: Vec<ResolvedRef>,
    /// Media fields the call filled through `extra`; their files are in
    /// `resolved` too, and go to these fields rather than the default slots.
    fields: Vec<MediaField>,
    notes: Vec<String>,
    per_output: Option<f64>,
    slug: String,
}

/// A media field named in `extra` (seedance's `input_face_images`, which
/// the default slots never target), in the order the call listed it.
#[derive(Debug)]
struct MediaField {
    name: String,
    array: bool,
    entries: Vec<FieldEntry>,
}

#[derive(Debug)]
enum FieldEntry {
    /// Already hosted (an http(s) or data: URI); sent as given.
    Hosted(String),
    /// A label or project file, uploaded with the other references.
    File(PathBuf),
}

impl MediaField {
    fn files(&self) -> impl Iterator<Item = &PathBuf> {
        self.entries.iter().filter_map(|e| match e {
            FieldEntry::File(p) => Some(p),
            FieldEntry::Hosted(_) => None,
        })
    }
}

async fn run_generation(
    ctx: &Arc<MediaContext>,
    req: GenerateRequest,
    call: &CallContext,
) -> Result<String, ToolError> {
    if ctx.queue().is_none() && ctx.local.is_none() {
        return Ok(no_key_result());
    }
    if req.prompt.trim().is_empty() {
        return Err(ToolError::InvalidArguments(
            "missing non-empty `prompt`".into(),
        ));
    }
    if req.count == 0 || req.count > MAX_COUNT {
        return Err(ToolError::InvalidArguments(format!(
            "`count` must be 1–{MAX_COUNT} (got {})",
            req.count
        )));
    }
    let catalog = ctx.catalog().await?;
    let prepared = prepare(ctx, &catalog, &req)?;
    if prepared.model.is_local() {
        return run_local(ctx, req, prepared, call).await;
    }
    let Some(queue) = ctx.queue() else {
        return Ok(no_key_result());
    };
    let count = req.count;

    // Budget: under the limits it runs; over (or unpriced) the user decides.
    let total = prepared.per_output.map(|c| c * count as f64);
    let verdict = ctx.prefs.budget().check(prepared.per_output, count);
    if let BudgetVerdict::Ask(reason) = verdict {
        let estimate = SpendEstimate {
            model: prepared.model.id.clone(),
            kind: req.kind,
            count,
            per_output_usd: prepared.per_output,
            total_usd: total,
            detail: request_detail(&prepared.params),
            reason: reason.clone(),
        };
        match ctx.sink.confirm_spend(&estimate).await? {
            Some(true) => {}
            Some(false) => {
                return Ok(format!(
                    "The user declined this generation ({}, {}). Ask what they'd like \
                     instead — a cheaper model, fewer outputs, a shorter clip — rather \
                     than retrying the same call.",
                    estimate.total_label(),
                    reason
                ))
            }
            None => {
                return Err(ToolError::Execution(format!(
                    "this generation needs the user's approval ({}; {}) and no user is \
                     available to ask — a subagent can't spend; hand it to the main agent",
                    estimate.total_label(),
                    reason
                )))
            }
        }
    }

    // References go to the hub first (progress rows in the media feed),
    // then land in the model's fields as URLs.
    let (urls, sources) = upload_refs(ctx, &prepared.resolved).await?;
    let mut body = prepared.body.clone();
    let prompt = place_refs(
        &prepared.model,
        &prepared.resolved,
        &prepared.fields,
        &urls,
        &req.prompt,
        &mut body,
    )?;
    if prepared.model.has_param("prompt") {
        body.insert("prompt".into(), Value::String(prompt.clone()));
    }

    // Enqueue.
    body.insert("model".into(), Value::String(prepared.model.id.clone()));
    body.insert("num_generations".into(), Value::from(count));
    if let Some((ns, repo)) = ctx.hub_target() {
        body.insert("target_namespace".into(), Value::String(ns));
        body.insert("target_repo".into(), Value::String(repo));
        body.insert(
            "target_directory".into(),
            Value::String(ctx.library.dir_rel().to_string()),
        );
    }
    // Subscribe before enqueueing so the completion event can't be missed.
    let feed = ctx.feed();
    let started = Instant::now();
    let ids = queue
        .enqueue(&Value::Object(body))
        .await
        .map_err(queue_error)?;
    let items = record_batch(ctx, &req, &prepared, call, &ids, &prompt, sources);
    finish_batch(
        ctx,
        &req,
        &prepared,
        items,
        Waiter::Hub { queue, feed },
        started,
        total,
    )
    .await
}

/// Run a generation on this machine: no key, no upload, no spend — the
/// references are read straight from disk and each output queues for the GPU.
async fn run_local(
    ctx: &Arc<MediaContext>,
    req: GenerateRequest,
    prepared: Prepared,
    call: &CallContext,
) -> Result<String, ToolError> {
    let provider = ctx.local.clone().ok_or_else(|| {
        ToolError::Execution(format!(
            "{} runs on this machine, and this host has no local media engine",
            prepared.model.id
        ))
    })?;
    if let Some(info) = prepared.model.local.as_ref().filter(|l| !l.installed) {
        return Err(ToolError::Execution(format!(
            "{} isn't installed yet ({:.1} GB to download). Ask the user to run \
             `oxen-harness media pull {}`, or pick an installed model from media_models.",
            prepared.model.id,
            info.download_bytes as f64 / 1e9,
            prepared.model.id
        )));
    }
    let mut prepared = prepared;
    let sources = store_sources(ctx, &prepared.resolved, &mut prepared.notes);
    let prompt = local_prompt(&prepared.resolved, &req.prompt);
    let started = Instant::now();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let ids: Vec<String> = (1..=req.count)
        .map(|i| format!("{}{stamp:x}-{i}", crate::local::LOCAL_JOB_PREFIX))
        .collect();
    let refs: Vec<PathBuf> = prepared.resolved.iter().map(|r| r.path.clone()).collect();
    let jobs = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let mut params = prepared.params.clone();
            // A fixed seed makes variations: each output steps it by one,
            // wrapping within the non-negative int64 range engines accept.
            if let Some(seed) = params.get("seed").and_then(Value::as_u64) {
                let stepped = seed.wrapping_add(i as u64) & i64::MAX as u64;
                params.insert("seed".into(), Value::from(stepped));
            }
            LocalJob {
                id: id.clone(),
                model: prepared.model.id.clone(),
                kind: req.kind,
                prompt: prompt.clone(),
                params,
                refs: refs.clone(),
            }
        })
        .collect();
    let items = record_batch(ctx, &req, &prepared, call, &ids, &prompt, sources);
    finish_batch(
        ctx,
        &req,
        &prepared,
        items,
        Waiter::Local { provider, jobs },
        started,
        None,
    )
    .await
}

/// Write the batch's rows (queued) into the library and return them.
fn record_batch(
    ctx: &MediaContext,
    req: &GenerateRequest,
    prepared: &Prepared,
    call: &CallContext,
    ids: &[String],
    prompt: &str,
    sources: Vec<MediaSource>,
) -> Vec<MediaItem> {
    let created_at = now_unix();
    let first = ids.first().map(String::as_str).unwrap_or("batch");
    let batch = format!("{created_at:x}-{}", &first[..first.len().min(8)]);
    let batch = match &ctx.batch_prefix {
        Some(prefix) => format!("{prefix}:{batch}"),
        None => batch,
    };
    let duration_secs = if req.kind == MediaKind::Video {
        crate::catalog::duration_secs(&prepared.model, &prepared.params)
    } else {
        None
    };
    let ref_paths: Vec<String> = sources.iter().map(|s| s.path.clone()).collect();
    let items: Vec<MediaItem> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| MediaItem {
            id: id.clone(),
            // A lane shares its parent's tools, so the call — not the
            // context the tools were built for — names the chat that asked.
            session: call.session.clone().unwrap_or_else(|| ctx.session.clone()),
            turn_seq: call.turn_seq,
            call_id: call.call_id.clone(),
            batch: batch.clone(),
            index: i as u32 + 1,
            kind: req.kind,
            model: prepared.model.id.clone(),
            prompt: prompt.to_string(),
            params: Value::Object(prepared.params.clone()),
            refs: ref_paths.clone(),
            path: None,
            poster: None,
            bytes: 0,
            width: None,
            height: None,
            duration_secs,
            cost_usd: prepared.per_output,
            status: MediaStatus::Queued,
            error: None,
            created_at,
            completed_at: None,
            parent: req.parent.clone(),
            seed: prepared.params.get("seed").cloned(),
            sources: sources.clone(),
            agent_prompt: (req.prompt != prompt).then(|| req.prompt.clone()),
            provider: None,
            progress: None,
        })
        .collect();
    for item in &items {
        if let Err(e) = ctx.library.record(item.clone()) {
            tracing::warn!("could not record generation {}: {e}", item.id);
            ctx.library.upsert(item.clone());
        }
    }
    ctx.library.clear_uploads(&ctx.session, false);
    items
}

/// Who a recorded batch is waiting on.
#[derive(Clone)]
enum Waiter {
    Hub {
        queue: QueueClient,
        feed: Option<Arc<crate::events::CompletionFeed>>,
    },
    /// One job per item, in item order.
    Local {
        provider: Arc<dyn LocalMedia>,
        jobs: Vec<LocalJob>,
    },
}

impl Waiter {
    /// The batch's outcomes, plus any warnings an engine raised on the way
    /// (memory was tight) for the report.
    async fn wait(
        &self,
        ctx: &Arc<MediaContext>,
        items: Vec<MediaItem>,
        slug: &str,
    ) -> (Vec<Outcome>, Vec<String>) {
        match self {
            Self::Hub { queue, feed } => (
                wait_all(ctx, queue, items, slug, feed.clone()).await,
                Vec::new(),
            ),
            // The GPU runs one job at a time, so outputs go in order.
            Self::Local { provider, jobs } => {
                let warnings = Arc::new(std::sync::Mutex::new(Vec::new()));
                let mut outcomes = Vec::new();
                for (item, job) in items.into_iter().zip(jobs.iter().cloned()) {
                    // A later output cancelled (media_status) while an
                    // earlier one rendered never reaches the engine.
                    let cancelled = ctx
                        .library
                        .items()
                        .iter()
                        .any(|i| i.id == item.id && i.status == MediaStatus::Cancelled);
                    outcomes.push(if cancelled {
                        Outcome {
                            item,
                            status: MediaStatus::Cancelled,
                            rel: None,
                            poster: None,
                            error: Some("cancelled".into()),
                        }
                    } else {
                        wait_local(ctx, provider.as_ref(), item, job, slug, warnings.clone()).await
                    });
                }
                // Poisoned only if a progress callback panicked; the
                // outcomes still stand without the warnings.
                let warnings = match warnings.lock() {
                    Ok(w) => w.clone(),
                    Err(_) => {
                        tracing::warn!("local warnings poisoned; the report omits them");
                        Vec::new()
                    }
                };
                (outcomes, warnings)
            }
        }
    }
}

/// Wait for a recorded batch — inline, or in the background with the
/// report delivered as an aside — and report it.
async fn finish_batch(
    ctx: &Arc<MediaContext>,
    req: &GenerateRequest,
    prepared: &Prepared,
    items: Vec<MediaItem>,
    waiter: Waiter,
    started: Instant,
    total: Option<f64>,
) -> Result<String, ToolError> {
    let slug = prepared.slug.clone();
    let model_id = prepared.model.id.clone();
    if !req.wait {
        if let Some(asides) = ctx.asides.clone() {
            let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
            let ctx2 = ctx.clone();
            let kind = req.kind;
            let slug2 = slug.clone();
            let model2 = model_id.clone();
            tokio::spawn(async move {
                let (outcomes, warnings) = waiter.wait(&ctx2, items, &slug2).await;
                commit_outputs(&ctx2, &outcomes, &slug2).await;
                let body = report(
                    kind,
                    &model2,
                    &outcomes,
                    started.elapsed(),
                    &warnings,
                    false,
                    ctx2.library.root(),
                );
                let done = outcomes
                    .iter()
                    .filter(|o| o.status == MediaStatus::Succeeded)
                    .count();
                asides.push(harness_tools::Aside {
                    kind: "media".into(),
                    title: format!(
                        "{kind} generation finished: {done}/{} ready ({slug2})",
                        outcomes.len()
                    ),
                    body,
                });
            });
            let est = total
                .map(|t| format!(", est. {}", fmt_usd(t)))
                .unwrap_or_default();
            let mut msg = format!(
                "Queued {} {} generation(s) with {} (ids {}{est}). The result will be \
                 delivered to you automatically when it finishes — keep working on other \
                 things and do not poll. media_status shows progress; the user sees the \
                 job in their media feed.",
                req.count,
                req.kind,
                model_id,
                ids.join(", ")
            );
            for n in &prepared.notes {
                msg.push_str(&format!("\nNote: {n}"));
            }
            return Ok(msg);
        }
    }

    let (outcomes, warnings) = waiter.wait(ctx, items, &slug).await;
    commit_outputs(ctx, &outcomes, &slug).await;
    let mut notes = prepared.notes.clone();
    notes.extend(warnings);
    Ok(report(
        req.kind,
        &model_id,
        &outcomes,
        started.elapsed(),
        &notes,
        true,
        ctx.library.root(),
    ))
}

/// Resolve the model, references, and parameters into a validated request.
fn prepare(
    ctx: &MediaContext,
    catalog: &Catalog,
    req: &GenerateRequest,
) -> Result<Prepared, ToolError> {
    let mut notes = Vec::new();
    let named = req
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    let model_id = match named {
        Some(id) => id.to_string(),
        None => default_model(ctx, catalog, req.kind, &mut notes),
    };
    let model_id = model_id.as_str();
    let model = catalog.get(model_id).cloned().ok_or_else(|| {
        let near = catalog
            .search(Some(req.kind), Some(model_id))
            .into_iter()
            .take(5)
            .map(|m| m.id.clone())
            .collect::<Vec<_>>();
        ToolError::InvalidArguments(if near.is_empty() {
            format!("no model `{model_id}` in the catalog — find one with media_models")
        } else {
            format!("no model `{model_id}`; did you mean: {}", near.join(", "))
        })
    })?;
    if model.kind != req.kind {
        return Err(ToolError::InvalidArguments(format!(
            "{} is a {} model; use generate_{} for it",
            model.id, model.kind, model.kind
        )));
    }

    // References: the explicit list plus any label the prompt mentions.
    let mut ref_names: Vec<String> = req.refs.iter().map(|r| r.trim().to_string()).collect();
    for label in ctx.refs.labels_in(&req.prompt) {
        if !ref_names
            .iter()
            .any(|r| refs::is_label(r) && ctx.refs.lookup(r) == ctx.refs.lookup(&label))
        {
            ref_names.push(label);
        }
    }
    let mut resolved = refs::resolve(&ref_names, &ctx.refs, &ctx.root)?;

    let mut body = Map::new();
    let mut params = Map::new();

    // Typed params: only the ones this model has; the rest are noted.
    for (name, value) in &req.typed {
        if !model.has_param(name) {
            notes.push(format!(
                "{} has no `{name}` parameter; it was ignored",
                model.id
            ));
            continue;
        }
        let value = coerce(&model, name, value.clone())?;
        params.insert((*name).to_string(), value);
    }
    // Extra params: must exist in the schema. A media field takes labels and
    // project paths like `refs` does, and they're uploaded the same way.
    let mut fields = Vec::new();
    for (name, value) in &req.extra {
        if name == "prompt" || name == "model" || name == "num_generations" {
            continue;
        }
        if let Some(slot) = model.media_field(name) {
            let (field, files) = read_media_field(ctx, &model, slot, value)?;
            for r in files {
                if !resolved.iter().any(|known| known.path == r.path) {
                    resolved.push(r);
                }
            }
            fields.push(field);
            continue;
        }
        if !model.has_param(name) {
            return Err(ToolError::InvalidArguments(format!(
                "{} has no `{name}` parameter. Its parameters: {}",
                model.id,
                model.param_names().join(", ")
            )));
        }
        let value = coerce(&model, name, value.clone())?;
        params.insert(name.clone(), value);
    }

    // The model must have a field for every kind of reference given —
    // checked now, before anything is uploaded.
    validate_slots(&model, &unclaimed(&resolved, &fields), catalog)?;
    for (k, v) in &params {
        body.insert(k.clone(), v.clone());
    }

    let per_output = estimate_cost(&model, &params);
    let slug = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(slugify)
        .unwrap_or_else(|| slugify(&req.prompt));
    Ok(Prepared {
        model,
        body,
        params,
        resolved,
        fields,
        notes,
        per_output,
        slug,
    })
}

/// Read a media field from `extra`: hosted URIs pass through, anything else
/// resolves as a reference (label or project path) and is checked against
/// the kind the field's name says it takes.
fn read_media_field(
    ctx: &MediaContext,
    model: &MediaModel,
    slot: crate::catalog::Slot,
    value: &Value,
) -> Result<(MediaField, Vec<ResolvedRef>), ToolError> {
    let name = &slot.name;
    let values: Vec<&str> = match value {
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => items
            .iter()
            .map(|v| {
                v.as_str().ok_or_else(|| {
                    ToolError::InvalidArguments(format!(
                        "`extra.{name}` takes labels, project paths, or URLs (got {v})"
                    ))
                })
            })
            .collect::<Result<_, _>>()?,
        other => {
            return Err(ToolError::InvalidArguments(format!(
                "`extra.{name}` takes labels, project paths, or URLs (got {other})"
            )))
        }
    };
    if !slot.array && values.len() > 1 {
        return Err(ToolError::InvalidArguments(format!(
            "`extra.{name}` takes one reference for {}; {} were given",
            model.id,
            values.len()
        )));
    }
    let mut entries = Vec::new();
    let mut files = Vec::new();
    for v in values {
        let v = v.trim();
        if is_hosted(v) {
            entries.push(FieldEntry::Hosted(v.to_string()));
            continue;
        }
        for r in refs::resolve(&[v.to_string()], &ctx.refs, &ctx.root)? {
            if let Some(want) = RefKind::from_field_name(name).filter(|k| *k != r.kind) {
                return Err(ToolError::InvalidArguments(format!(
                    "`extra.{name}` takes {} references; {v} is {}",
                    want.word().to_ascii_lowercase(),
                    r.kind.word().to_ascii_lowercase()
                )));
            }
            entries.push(FieldEntry::File(r.path.clone()));
            files.push(r);
        }
    }
    let field = MediaField {
        name: name.clone(),
        array: slot.array,
        entries,
    };
    Ok((field, files))
}

fn is_hosted(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ["http://", "https://", "data:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

/// The references no `extra` media field claimed: these go to the model's
/// default slots.
fn unclaimed(resolved: &[ResolvedRef], fields: &[MediaField]) -> Vec<ResolvedRef> {
    resolved
        .iter()
        .filter(|r| {
            !fields
                .iter()
                .flat_map(MediaField::files)
                .any(|p| *p == r.path)
        })
        .cloned()
        .collect()
}

/// The user's default model for `kind` — unless it is hosted and there is
/// no key to reach the hub, when an installed local model stands in (and a
/// note says so) rather than failing a call that could have run here.
fn default_model(
    ctx: &MediaContext,
    catalog: &Catalog,
    kind: MediaKind,
    notes: &mut Vec<String>,
) -> String {
    let preferred = match kind {
        MediaKind::Image => &ctx.prefs.default_image_model,
        MediaKind::Video => &ctx.prefs.default_video_model,
    };
    if crate::local::is_local_id(preferred) || ctx.api().is_some() {
        return preferred.clone();
    }
    // The fastest installed one: a stand-in should feel like a draft model.
    let installed = catalog
        .models()
        .iter()
        .filter(|m| m.kind == kind)
        .filter_map(|m| Some((m, m.local.as_ref().filter(|l| l.installed)?)))
        .min_by(|(_, a), (_, b)| {
            let secs = |l: &crate::LocalInfo| l.typical_secs.unwrap_or(f64::MAX);
            secs(a).total_cmp(&secs(b))
        })
        .map(|(m, _)| m);
    match installed {
        Some(m) => {
            notes.push(format!(
                "no Oxen API key is set, so the on-device model {} stood in for the \
                 default {preferred}",
                m.id
            ));
            m.id.clone()
        }
        None => preferred.clone(),
    }
}

/// Refuse early when the model has no field for a kind of reference the
/// call gave it, naming models that do.
fn validate_slots(
    model: &MediaModel,
    resolved: &[ResolvedRef],
    catalog: &Catalog,
) -> Result<(), ToolError> {
    for kind in [RefKind::Image, RefKind::Video, RefKind::Audio] {
        let count = resolved.iter().filter(|r| r.kind == kind).count();
        if count == 0 {
            continue;
        }
        let slots = model.slots(kind);
        if slots.is_empty() {
            let alternatives = catalog
                .accepting(model.kind, kind)
                .into_iter()
                .take(6)
                .map(|m| m.id.clone())
                .collect::<Vec<_>>();
            return Err(ToolError::InvalidArguments(format!(
                "{} doesn't take {} references. {}",
                model.id,
                kind.word().to_ascii_lowercase(),
                if alternatives.is_empty() {
                    format!("No {} model in the catalog does.", model.kind)
                } else {
                    format!("Models that do: {}", alternatives.join(", "))
                }
            )));
        }
        let primary = &slots[0];
        if !primary.array && count > 1 && slots.len() < 2 {
            return Err(ToolError::InvalidArguments(format!(
                "{} takes one {} reference (`{}`); {count} were given",
                model.id,
                kind.word().to_ascii_lowercase(),
                primary.name
            )));
        }
        if !primary.array && count > 2 {
            return Err(ToolError::InvalidArguments(format!(
                "{} takes at most two {} references (first and last frame); {count} were given",
                model.id,
                kind.word().to_ascii_lowercase()
            )));
        }
    }
    Ok(())
}

/// Copy each reference into the library for provenance, with no upload —
/// a local engine reads the originals from disk. A copy that fails doesn't
/// stop the generation, but the result says the lineage is incomplete.
fn store_sources(
    ctx: &MediaContext,
    resolved: &[ResolvedRef],
    notes: &mut Vec<String>,
) -> Vec<MediaSource> {
    resolved
        .iter()
        .filter_map(|r| match ctx.library.store_ref(&r.path) {
            Ok(stored) => Some(source_of(ctx, r, stored)),
            Err(e) => {
                notes.push(format!(
                    "{} could not be copied into the library ({e}), so the gallery won't \
                     show it as this image's source",
                    r.filename
                ));
                None
            }
        })
        .collect()
}

/// `[Image #2]` in the prompt → `reference image 1` (its position among the
/// references the engine gets).
fn local_prompt(resolved: &[ResolvedRef], prompt: &str) -> String {
    let mut prompt = prompt.to_string();
    for (i, r) in resolved.iter().enumerate() {
        if let Some(label) = &r.label {
            prompt = prompt.replace(label, &format!("reference image {}", i + 1));
        }
    }
    prompt
}

/// Upload every reference to the hub (progress rows in the media feed) and
/// copy it into the library. Returns each file's public URL by path, and
/// the provenance of each copy for the manifest.
async fn upload_refs(
    ctx: &Arc<MediaContext>,
    resolved: &[ResolvedRef],
) -> Result<(std::collections::HashMap<PathBuf, String>, Vec<MediaSource>), ToolError> {
    let mut urls = std::collections::HashMap::new();
    let mut sources = Vec::new();
    if resolved.is_empty() {
        return Ok((urls, sources));
    }
    let uploader = ctx
        .uploader()
        .ok_or_else(|| ToolError::Execution(no_key_result()))?;
    // A new batch replaces the last one's rows (a failed row stayed visible
    // until now so the user could read it).
    ctx.library.clear_uploads(&ctx.session, false);
    let started = now_unix();
    for (i, r) in resolved.iter().enumerate() {
        let total = std::fs::metadata(&r.path).map(|m| m.len()).unwrap_or(0);
        let row = crate::library::MediaUpload {
            id: format!("{}-{started:x}-{i}", ctx.session),
            session: ctx.session.clone(),
            label: r.label.clone(),
            filename: r.filename.clone(),
            kind: r.kind.word().to_ascii_lowercase(),
            bytes_sent: 0,
            bytes_total: total,
            status: crate::library::UploadStatus::Uploading,
            error: None,
            started_at: started,
        };
        ctx.library.set_upload(row.clone());
        // Progress: report on every percent, not every chunk.
        let library = ctx.library.clone();
        let progress_row = row.clone();
        let last = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let on_progress: Arc<dyn Fn(u64, u64) + Send + Sync> = Arc::new(move |sent, total| {
            let pct = (sent * 100).checked_div(total).unwrap_or(100);
            let prev = last.swap(pct, std::sync::atomic::Ordering::Relaxed);
            if pct != prev || sent == total {
                library.set_upload(crate::library::MediaUpload {
                    bytes_sent: sent,
                    bytes_total: total,
                    status: if sent >= total && total > 0 {
                        crate::library::UploadStatus::Presigning
                    } else {
                        crate::library::UploadStatus::Uploading
                    },
                    ..progress_row.clone()
                });
            }
        });
        match uploader
            .upload_into(ctx.hub_target(), &r.path, r.kind, on_progress)
            .await
        {
            Ok(uploaded) => {
                ctx.library.set_upload(crate::library::MediaUpload {
                    bytes_sent: uploaded.bytes,
                    bytes_total: uploaded.bytes,
                    status: if uploaded.reused {
                        crate::library::UploadStatus::Reused
                    } else {
                        crate::library::UploadStatus::Done
                    },
                    ..row
                });
                urls.insert(r.path.clone(), uploaded.url);
            }
            Err(e) => {
                let message = match &e {
                    crate::upload::UploadError::Unauthorized => format!(
                        "{MEDIA_NO_KEY} The hub rejected the configured key for uploads (401)."
                    ),
                    other => other.to_string(),
                };
                ctx.library.set_upload(crate::library::MediaUpload {
                    status: crate::library::UploadStatus::Failed,
                    error: Some(message.clone()),
                    ..row
                });
                return Err(ToolError::Execution(format!(
                    "could not upload reference {}: {message}",
                    r.filename
                )));
            }
        }
        match ctx.library.store_ref(&r.path) {
            Ok(stored) => sources.push(source_of(ctx, r, stored)),
            Err(e) => tracing::warn!("could not copy reference {}: {e}", r.path.display()),
        }
    }
    Ok((urls, sources))
}

/// Where a reference came from. An earlier output of this library wins
/// over how it was named (a chip the user made by dragging a generation
/// back into the chat still traces to that generation); otherwise a chip
/// label means the user attached it, and anything else is a project file.
fn source_of(ctx: &MediaContext, r: &ResolvedRef, stored: StoredRef) -> MediaSource {
    let root = ctx.root.canonicalize().unwrap_or_else(|_| ctx.root.clone());
    let canonical = r.path.canonicalize().unwrap_or_else(|_| r.path.clone());
    let relative = canonical
        .strip_prefix(&root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let generation = relative.as_deref().and_then(|rel| {
        ctx.library
            .items()
            .into_iter()
            .find(|i| i.path.as_deref() == Some(rel))
            .map(|i| i.id)
    });
    let origin = if generation.is_some() {
        SourceOrigin::Generation
    } else if r.label.is_some() {
        SourceOrigin::Attachment
    } else {
        SourceOrigin::File
    };
    MediaSource {
        path: stored.path,
        origin,
        label: r.label.clone(),
        source: relative.unwrap_or_else(|| r.path.to_string_lossy().into_owned()),
        generation,
        kind: r.kind.word().to_ascii_lowercase(),
        sha256: stored.sha256,
    }
}

/// One request field's references, as sent.
struct Placed<'a> {
    field: String,
    kind: Option<RefKind>,
    array: bool,
    /// Each URL, with the reference it came from when it was a file.
    values: Vec<(String, Option<&'a ResolvedRef>)>,
}

/// Put uploaded references into the model's fields and rewrite `[Image #N]`
/// mentions to the model's own convention. Fields named in `extra` take
/// their own files; the rest go to the default slots, which were validated
/// in [`validate_slots`], so a model that can't take a kind never gets here.
fn place_refs(
    model: &MediaModel,
    resolved: &[ResolvedRef],
    fields: &[MediaField],
    urls: &std::collections::HashMap<PathBuf, String>,
    prompt: &str,
    body: &mut Map<String, Value>,
) -> Result<String, ToolError> {
    let find = |path: &PathBuf| -> Result<(String, &ResolvedRef), ToolError> {
        let r = resolved
            .iter()
            .find(|r| r.path == *path)
            .ok_or_else(|| ToolError::Execution(format!("{} was not resolved", path.display())))?;
        let url = urls
            .get(path)
            .cloned()
            .ok_or_else(|| ToolError::Execution(format!("{} was not uploaded", r.filename)))?;
        Ok((url, r))
    };

    let mut placed: Vec<Placed> = Vec::new();
    for f in fields {
        let mut values = Vec::new();
        for entry in &f.entries {
            values.push(match entry {
                FieldEntry::Hosted(url) => (url.clone(), None),
                FieldEntry::File(path) => {
                    let (url, r) = find(path)?;
                    (url, Some(r))
                }
            });
        }
        let kind = RefKind::from_field_name(&f.name)
            .or_else(|| values.iter().find_map(|(_, r)| r.map(|r| r.kind)));
        placed.push(Placed {
            field: f.name.clone(),
            kind,
            array: f.array,
            values,
        });
    }
    let defaults = unclaimed(resolved, fields);
    for kind in [RefKind::Image, RefKind::Video, RefKind::Audio] {
        let of_kind: Vec<&ResolvedRef> = defaults.iter().filter(|r| r.kind == kind).collect();
        if of_kind.is_empty() {
            continue;
        }
        let slots = model.slots(kind);
        let Some(primary) = slots.first() else {
            return Err(ToolError::InvalidArguments(format!(
                "{} doesn't take {} references",
                model.id,
                kind.word().to_ascii_lowercase()
            )));
        };
        let mut values = Vec::new();
        for r in &of_kind {
            let (url, r) = find(&r.path)?;
            values.push((url, Some(r)));
        }
        if primary.array {
            placed.push(Placed {
                field: primary.name.clone(),
                kind: Some(kind),
                array: true,
                values,
            });
        } else {
            // A single-URI model: the first image is the first frame and a
            // second one the tail frame.
            for (slot, value) in slots.iter().zip(values) {
                placed.push(Placed {
                    field: slot.name.clone(),
                    kind: Some(kind),
                    array: false,
                    values: vec![value],
                });
            }
        }
    }

    for p in &placed {
        let urls: Vec<Value> = p
            .values
            .iter()
            .map(|(url, _)| Value::String(url.clone()))
            .collect();
        if p.array {
            body.insert(p.field.clone(), Value::Array(urls));
        } else if let Some(first) = urls.into_iter().next() {
            body.insert(p.field.clone(), first);
        }
    }

    // "[Image #2]" in the prompt → "@Image1" (its position among the images
    // sent, counted in the model's own field order) or "reference image 1".
    let mut prompt = prompt.to_string();
    let at_style = model.prompt_uses_at_refs();
    for kind in [RefKind::Image, RefKind::Video, RefKind::Audio] {
        let mut of_kind: Vec<&Placed> = placed.iter().filter(|p| p.kind == Some(kind)).collect();
        of_kind.sort_by_key(|p| model.ref_rank(&p.field));
        let sent = of_kind.iter().flat_map(|p| p.values.iter());
        for (i, (_, r)) in sent.enumerate() {
            if let Some(label) = r.and_then(|r| r.label.as_ref()) {
                let replacement = if at_style {
                    format!("@{}{}", kind.word(), i + 1)
                } else {
                    format!("reference {} {}", kind.word().to_ascii_lowercase(), i + 1)
                };
                prompt = prompt.replace(label, &replacement);
            }
        }
    }
    Ok(prompt)
}

/// Check a value against the schema property (enum, range, type) and
/// coerce the easy cases (a numeric duration into an integer field, a
/// number given as a string).
fn coerce(model: &MediaModel, name: &str, value: Value) -> Result<Value, ToolError> {
    let Some(prop) = model.property(name) else {
        return Ok(value);
    };
    // A union type (`["integer", "string"]`: seconds or "auto") takes a
    // value of any of its types as is, except that a number still becomes
    // an integer: the tool's duration is a float, and providers reject 5.0.
    let types = crate::catalog::schema_types(prop);
    let takes = |t: &str| types.contains(&t);
    let parsed = |s: &str| s.trim().parse::<f64>().ok();
    let value = match &value {
        Value::Number(n) if takes("integer") && !takes("number") => {
            Value::from(n.as_f64().unwrap_or(0.0).round() as i64)
        }
        Value::Number(n) if types == ["string"] => Value::String(n.to_string()),
        Value::String(s) if !takes("string") => match parsed(s) {
            Some(f) if takes("integer") => Value::from(f.round() as i64),
            Some(f) if takes("number") => Value::from(f),
            _ if takes("boolean") => Value::Bool(matches!(s.trim(), "true" | "yes" | "on")),
            _ => value,
        },
        _ => value,
    };
    if let Some(choices) = prop.get("enum").and_then(Value::as_array) {
        let matches = choices.iter().any(|c| match (c, &value) {
            (Value::String(a), Value::String(b)) => a.eq_ignore_ascii_case(b),
            (a, b) => a == b,
        });
        if !matches {
            return Err(ToolError::InvalidArguments(format!(
                "`{name}` must be one of {} for {} (got {})",
                choices
                    .iter()
                    .map(|c| match c {
                        Value::String(s) => s.clone(),
                        o => o.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                model.id,
                value
            )));
        }
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = prop.get("minimum").and_then(Value::as_f64) {
            if n < min {
                return Err(ToolError::InvalidArguments(format!(
                    "`{name}` must be ≥ {min} for {} (got {n})",
                    model.id
                )));
            }
        }
        if let Some(max) = prop.get("maximum").and_then(Value::as_f64) {
            if n > max {
                return Err(ToolError::InvalidArguments(format!(
                    "`{name}` must be ≤ {max} for {} (got {n})",
                    model.id
                )));
            }
        }
    }
    Ok(value)
}

/// `8s, 720p, 16:9` — the parameters a person would want in a spend prompt.
fn request_detail(params: &Map<String, Value>) -> String {
    let mut parts = Vec::new();
    if let Some(d) = params.get("duration").and_then(Value::as_f64) {
        parts.push(format!("{d}s"));
    }
    for key in ["resolution", "quality", "aspect_ratio", "size"] {
        if let Some(v) = params.get(key) {
            parts.push(match v {
                Value::String(s) => s.clone(),
                o => o.to_string(),
            });
        }
    }
    if params.get("generate_audio").and_then(Value::as_bool) == Some(true) {
        parts.push("with audio".into());
    }
    parts.join(", ")
}

fn queue_error(e: QueueError) -> ToolError {
    match e {
        QueueError::Unauthorized => ToolError::Execution(format!(
            "{MEDIA_NO_KEY} The hub rejected the configured key (401). Ask the user to \
             check Settings → Connection."
        )),
        other => ToolError::Execution(other.to_string()),
    }
}

/// One finished (or failed) output.
struct Outcome {
    item: MediaItem,
    status: MediaStatus,
    rel: Option<String>,
    poster: Option<String>,
    error: Option<String>,
}

/// Wait for every output of a batch concurrently.
async fn wait_all(
    ctx: &Arc<MediaContext>,
    queue: &QueueClient,
    items: Vec<MediaItem>,
    slug: &str,
    feed: Option<Arc<crate::events::CompletionFeed>>,
) -> Vec<Outcome> {
    let mut handles = Vec::new();
    for item in items {
        let ctx = ctx.clone();
        let queue = queue.clone();
        let slug = slug.to_string();
        let feed = feed.clone();
        handles.push(tokio::spawn(async move {
            wait_one(&ctx, &queue, item, &slug, feed).await
        }));
    }
    let mut outcomes = Vec::new();
    for h in handles {
        if let Ok(o) = h.await {
            outcomes.push(o);
        }
    }
    outcomes.sort_by_key(|o| o.item.index);
    outcomes
}

async fn wait_one(
    ctx: &MediaContext,
    queue: &QueueClient,
    mut item: MediaItem,
    slug: &str,
    feed: Option<Arc<crate::events::CompletionFeed>>,
) -> Outcome {
    let kind = item.kind;
    let lib = ctx.library.clone();
    let id = item.id.clone();
    let mut on_status = {
        let mut snapshot = item.clone();
        let lib = lib.clone();
        move |status: &str| {
            let next = MediaStatus::from_hub(status);
            if !next.is_terminal() && snapshot.status != next {
                snapshot.status = next;
                lib.upsert(snapshot.clone());
            }
        }
    };
    let result = queue
        .wait_with_feed(&id, kind, deadline_for(kind), feed, &mut on_status)
        .await;
    let record = match result {
        Ok(record) => record,
        Err(QueueError::Timeout {
            elapsed, status, ..
        }) => {
            let msg = format!("still {status} after {elapsed}s; cancelled");
            return failed(&lib, item, MediaStatus::TimedOut, msg);
        }
        Err(e) => return failed(&lib, item, MediaStatus::Failed, e.to_string()),
    };
    item.seed = record.seed.clone().or(item.seed.take());
    item.provider = match serde_json::to_value(&record) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("could not keep the hub record for {}: {e}", item.id);
            None
        }
    };
    let status = MediaStatus::from_hub(&record.status);
    if status != MediaStatus::Succeeded {
        let msg = record
            .error_message
            .clone()
            .unwrap_or_else(|| format!("generation {}", record.status));
        return failed(&lib, item, status, msg);
    }
    let Some(url) = record.result_url.clone() else {
        let msg = "succeeded but the hub returned no result URL".to_string();
        return failed(&lib, item, MediaStatus::Failed, msg);
    };
    let bytes = match queue.download(&url).await {
        Ok(b) => b,
        Err(e) => {
            return failed(
                &lib,
                item,
                MediaStatus::Failed,
                format!("download failed: {e}"),
            )
        }
    };
    save_output(ctx, item, &bytes, &url, slug).await
}

/// Write a row's terminal state to the manifest (and the live feed).
fn settle(lib: &MediaLibrary, item: &mut MediaItem, status: MediaStatus, error: Option<String>) {
    item.status = status;
    item.error = error;
    item.progress = None;
    item.completed_at = Some(now_unix());
    if let Err(e) = lib.record(item.clone()) {
        tracing::warn!("could not record generation {}: {e}", item.id);
        lib.upsert(item.clone());
    }
}

/// A row that ended without an output.
fn failed(lib: &MediaLibrary, mut item: MediaItem, status: MediaStatus, msg: String) -> Outcome {
    settle(lib, &mut item, status, Some(msg.clone()));
    Outcome {
        item,
        status,
        rel: None,
        poster: None,
        error: Some(msg),
    }
}

/// Save a finished output into the library, measure it, and settle its row.
/// `url` is only a hint for the file extension (empty for local outputs).
async fn save_output(
    ctx: &MediaContext,
    mut item: MediaItem,
    bytes: &[u8],
    url: &str,
    slug: &str,
) -> Outcome {
    let lib = &ctx.library;
    let kind = item.kind;
    let ext = output_extension(kind, url, bytes, &item.params);
    let rel = match lib.save_output(bytes, slug, item.index, &ext, item.created_at) {
        Ok(rel) => rel,
        Err(e) => {
            return failed(
                lib,
                item,
                MediaStatus::Failed,
                format!("could not save the result: {e}"),
            )
        }
    };
    item.bytes = bytes.len() as u64;
    item.path = Some(rel.clone());
    let abs = lib.abs(&rel);
    match kind {
        MediaKind::Image => {
            if let Ok((w, h)) = image::image_dimensions(&abs) {
                item.width = Some(w);
                item.height = Some(h);
            }
        }
        MediaKind::Video => {
            item.poster = extract_poster(&abs, &rel).await;
        }
    }
    let poster = item.poster.clone();
    settle(lib, &mut item, MediaStatus::Succeeded, None);
    Outcome {
        item,
        status: MediaStatus::Succeeded,
        rel: Some(rel),
        poster,
        error: None,
    }
}

/// Run one local job for a recorded row: the row turns `processing` once
/// the engine starts on it, and settles with the engine's record attached.
async fn wait_local(
    ctx: &MediaContext,
    provider: &dyn LocalMedia,
    mut item: MediaItem,
    job: LocalJob,
    slug: &str,
    warnings: Arc<std::sync::Mutex<Vec<String>>>,
) -> Outcome {
    let lib = ctx.library.clone();
    let snapshot = std::sync::Mutex::new(item.clone());
    let advice = warnings.clone();
    let progress: Arc<dyn Fn(LocalProgress) + Send + Sync> = {
        let lib = lib.clone();
        Arc::new(move |p: LocalProgress| {
            // A poisoned snapshot only means an earlier callback panicked;
            // the feed update is cosmetic, so skip it rather than guess.
            let Ok(mut row) = snapshot.lock() else { return };
            let previous = row.progress.clone();
            let note = previous.as_ref().and_then(|p| p.note.clone());
            let next = match p {
                LocalProgress::Queued => progress_at("queued", None, note),
                LocalProgress::Loading => progress_at("loading", None, note),
                LocalProgress::Step { step, total } => {
                    progress_at("sampling", Some((step, total)), note)
                }
                LocalProgress::Finishing => progress_at("finishing", None, note),
                LocalProgress::Warning(message) => {
                    if let Ok(mut all) = warnings.lock() {
                        if !all.contains(&message) {
                            all.push(message.clone());
                        }
                    }
                    MediaProgress {
                        note: Some(message),
                        ..previous
                            .clone()
                            .unwrap_or_else(|| progress_at("loading", None, None))
                    }
                }
            };
            if next.phase != "queued" {
                row.status = MediaStatus::Processing;
            }
            if Some(&next) != previous.as_ref() {
                row.progress = Some(next);
                lib.upsert(row.clone());
            }
        })
    };
    match provider.generate(job, progress).await {
        Ok(output) => {
            item.seed = output.seed.or(item.seed.take());
            item.provider = Some(output.record);
            save_output(ctx, item, &output.bytes, "", slug).await
        }
        Err(LocalMediaError::Cancelled) => {
            failed(&lib, item, MediaStatus::Cancelled, "cancelled".to_string())
        }
        // Retrying into the same squeeze fails the same way (or stops the
        // chat model instead): the user has to free memory first.
        Err(LocalMediaError::Memory { detail, .. }) => {
            // The row keeps the user-facing reason; the agent also hears
            // what to do about it.
            if let Ok(mut all) = advice.lock() {
                let advice = "Don't retry a memory stop right away: tell the user, who can \
                              close other apps or use a smaller (or hosted) model."
                    .to_string();
                if !all.contains(&advice) {
                    all.push(advice);
                }
            }
            failed(&lib, item, MediaStatus::Failed, detail)
        }
        Err(e) => failed(&lib, item, MediaStatus::Failed, e.to_string()),
    }
}

fn progress_at(phase: &str, steps: Option<(u32, u32)>, note: Option<String>) -> MediaProgress {
    MediaProgress {
        phase: phase.to_string(),
        step: steps.map(|(s, _)| s),
        total: steps.map(|(_, t)| t),
        note,
    }
}

/// The saved file's extension: sniffed from the bytes first, then the
/// URL, then the requested `output_format`, then the kind's default.
fn output_extension(kind: MediaKind, url: &str, bytes: &[u8], params: &Value) -> String {
    if bytes.starts_with(b"\x89PNG") {
        return "png".into();
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return "jpg".into();
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "webp".into();
    }
    if bytes.starts_with(b"GIF8") {
        return "gif".into();
    }
    // ISO base media: the major brand tells QuickTime from MP4, and players
    // (and the hub's reference upload) pick a decoder by the name.
    if bytes.len() > 12 && &bytes[4..8] == b"ftyp" {
        return if &bytes[8..12] == b"qt  " {
            "mov"
        } else {
            "mp4"
        }
        .into();
    }
    if bytes.starts_with(b"\x1A\x45\xDF\xA3") {
        return "webm".into();
    }
    let from_url = url
        .split('?')
        .next()
        .and_then(|p| p.rsplit('/').next())
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| {
            matches!(
                e.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "mp4" | "webm" | "mov"
            )
        });
    if let Some(e) = from_url {
        return e;
    }
    if let Some(fmt) = params.get("output_format").and_then(Value::as_str) {
        return fmt.to_ascii_lowercase();
    }
    match kind {
        MediaKind::Image => "png".into(),
        MediaKind::Video => "mp4".into(),
    }
}

/// Grab a poster frame with `ffmpeg` when it's installed; `None` otherwise
/// (a missing ffmpeg is the common case and not an error).
async fn extract_poster(abs: &std::path::Path, rel: &str) -> Option<String> {
    let poster_abs = abs.with_extension("jpg");
    let poster_rel = match rel.rsplit_once('.') {
        Some((stem, _)) => format!("{stem}.jpg"),
        None => format!("{rel}.jpg"),
    };
    let run = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(abs)
        .args(["-frames:v", "1", "-q:v", "3"])
        .arg(&poster_abs)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match tokio::time::timeout(Duration::from_secs(60), run).await {
        Ok(Ok(status)) if status.success() && poster_abs.exists() => Some(poster_rel),
        _ => None,
    }
}

/// Version a finished batch with Oxen when the user asked for it and the
/// project is an Oxen repo: add each saved file (and the manifest), commit.
/// Best-effort — a missing `oxen` binary or a non-repo is silently fine.
async fn commit_outputs(ctx: &MediaContext, outcomes: &[Outcome], slug: &str) {
    if !ctx.prefs.commit_with_oxen {
        return;
    }
    let mut paths: Vec<String> = outcomes.iter().filter_map(|o| o.rel.clone()).collect();
    paths.extend(outcomes.iter().filter_map(|o| o.poster.clone()));
    if paths.is_empty() {
        return;
    }
    paths.push(format!("{}/manifest.jsonl", ctx.library.dir_rel()));
    let root = ctx.root.clone();
    let model = outcomes
        .first()
        .map(|o| o.item.model.clone())
        .unwrap_or_default();
    let message = format!("generate: {slug} ({model})");
    let _ = tokio::task::spawn_blocking(move || {
        let oxen = harness_oxen::Oxen::new();
        if !oxen.is_available() || !oxen.is_repo(&root) {
            return;
        }
        for path in &paths {
            if let Err(e) = oxen.add(&root, path) {
                tracing::warn!("oxen add {path}: {e}");
            }
        }
        if let Err(e) = oxen.commit(&root, &message) {
            tracing::warn!("oxen commit: {e}");
        }
    })
    .await;
}

/// The model-visible report of a batch, with the images attached (a poster
/// frame for videos) so the model can look at what it made.
fn report(
    kind: MediaKind,
    model: &str,
    outcomes: &[Outcome],
    elapsed: Duration,
    notes: &[String],
    attach: bool,
    root: &std::path::Path,
) -> String {
    let done: Vec<&Outcome> = outcomes
        .iter()
        .filter(|o| o.status == MediaStatus::Succeeded)
        .collect();
    let total_cost: f64 = done.iter().filter_map(|o| o.item.cost_usd).sum();
    let cost = if done.iter().any(|o| o.item.cost_usd.is_some()) {
        format!(", est. {}", fmt_usd(total_cost))
    } else {
        String::new()
    };
    let mut out = if done.is_empty() {
        format!(
            "No {kind} was produced by {model} ({}).\n",
            fmt_elapsed(elapsed)
        )
    } else {
        format!(
            "Generated {} {kind}{} with {model} in {}{cost}:\n",
            done.len(),
            if done.len() == 1 { "" } else { "s" },
            fmt_elapsed(elapsed)
        )
    };
    let mut attachments = Vec::new();
    for o in outcomes {
        match (&o.rel, &o.error) {
            (Some(rel), _) => {
                let mut line = format!("- {rel}");
                match (o.item.width, o.item.height) {
                    (Some(w), Some(h)) => line.push_str(&format!(" ({w}×{h})")),
                    _ => line.push_str(&format!(" ({})", fmt_bytes(o.item.bytes))),
                }
                if let Some(d) = o.item.duration_secs {
                    line.push_str(&format!(", {d}s"));
                }
                if let Some(p) = &o.poster {
                    line.push_str(&format!(" — poster {p}"));
                }
                out.push_str(&line);
                out.push('\n');
                if attach {
                    match kind {
                        MediaKind::Image => attachments.push(rel.clone()),
                        MediaKind::Video => {
                            if let Some(p) = &o.poster {
                                attachments.push(p.clone());
                            }
                        }
                    }
                }
            }
            (None, Some(err)) => out.push_str(&format!(
                "- #{} {}: {err}\n",
                o.item.index,
                o.status.as_str()
            )),
            (None, None) => out.push_str(&format!("- #{} {}\n", o.item.index, o.status.as_str())),
        }
    }
    for n in notes {
        out.push_str(&format!("Note: {n}\n"));
    }
    if !done.is_empty() {
        match kind {
            MediaKind::Image => out.push_str(
                "The user sees them in the chat and their media feed. Refer to one by its \
                 path (as a `ref` or `parent`) to vary, edit, upscale, or animate it.",
            ),
            MediaKind::Video => out.push_str(
                "The user can play it in the chat and their media feed. You can't watch \
                 it; describe it from the prompt and the poster frame, and refer to it by \
                 path to extend or edit it.",
            ),
        }
        if attach && !attachments.is_empty() {
            out.push_str(if kind == MediaKind::Image {
                " The image(s) are attached below so you can look at them."
            } else {
                " The poster frame is attached below."
            });
        } else if !attach && kind == MediaKind::Image {
            out.push_str(" Call read_file on a path to look at it.");
        }
        out.push('\n');
    }
    for rel in attachments {
        // Absolute: the agent loop reads the file from wherever the host
        // process runs (the desktop's cwd is not the project).
        let abs = root.join(&rel);
        out.push_str(&harness_core::attach::image_marker(&abs.to_string_lossy()));
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m{:02}s", s / 60, s % 60)
    }
}

fn fmt_bytes(b: u64) -> String {
    if b >= 1024 * 1024 {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", b / 1024)
    }
}

// ---- media_status -----------------------------------------------------------

pub struct MediaStatusTool {
    pub(crate) ctx: Arc<MediaContext>,
}

/// Arguments to `media_status`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct MediaStatusArgs {
    /// A generation id to cancel (with `cancel: true`). Omit to list.
    #[serde(default)]
    pub id: Option<String>,
    /// Cancel the generation `id` while it is still queued or processing.
    #[serde(default)]
    pub cancel: Option<bool>,
}

#[async_trait]
impl TypedTool for MediaStatusTool {
    const NAME: &'static str = MEDIA_STATUS_TOOL;
    type Args = MediaStatusArgs;

    fn description(&self) -> &str {
        "List this chat's image/video generations — in flight and recent — with \
         their status, paths, and cost; or cancel one by id. Background results \
         are delivered to you automatically, so don't call this in a loop."
    }

    async fn run(&self, args: MediaStatusArgs, _call: &CallContext) -> Result<String, ToolError> {
        if args.cancel == Some(true) {
            let id = args
                .id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    ToolError::InvalidArguments("`cancel` needs the generation `id`".into())
                })?;
            if crate::local::is_local_job_id(id) {
                let running = match &self.ctx.local {
                    Some(local) => local.cancel(id).await,
                    None => false,
                };
                // Outputs of a batch wait their turn without reaching the
                // provider; marking the row cancelled below is what skips them.
                let queued = || {
                    self.ctx
                        .library
                        .in_flight(None)
                        .iter()
                        .any(|i| i.id == id && i.status == MediaStatus::Queued)
                };
                if !running && !queued() {
                    return Err(ToolError::InvalidArguments(format!(
                        "no local generation `{id}` is queued or running"
                    )));
                }
            } else {
                let Some(queue) = self.ctx.queue() else {
                    return Ok(no_key_result());
                };
                queue.cancel(id).await.map_err(queue_error)?;
            }
            if let Some(mut item) = self
                .ctx
                .library
                .in_flight(None)
                .into_iter()
                .find(|i| i.id == id)
            {
                item.status = MediaStatus::Cancelled;
                item.completed_at = Some(now_unix());
                // The row is what skips a queued local output, so a failed
                // manifest write still updates the live feed.
                if let Err(e) = self.ctx.library.record(item.clone()) {
                    tracing::warn!("could not record the cancel of {id}: {e}");
                    self.ctx.library.upsert(item);
                }
            }
            return Ok(format!("Cancelled generation {id}."));
        }
        let in_flight = self.ctx.library.in_flight(Some(&self.ctx.session));
        let recent = self.ctx.library.recent(&self.ctx.session, 8);
        if in_flight.is_empty() && recent.is_empty() {
            return Ok("No generations in this chat yet.".to_string());
        }
        let mut out = String::new();
        if !in_flight.is_empty() {
            out.push_str(&format!("{} in flight:\n", in_flight.len()));
            let now = now_unix();
            for i in &in_flight {
                out.push_str(&format!(
                    "- {} {} {} ({}s elapsed) — {}\n",
                    i.id,
                    i.status.as_str(),
                    i.kind,
                    now - i.created_at,
                    short(&i.prompt)
                ));
            }
        }
        let finished: Vec<&MediaItem> = recent.iter().filter(|i| i.status.is_terminal()).collect();
        if !finished.is_empty() {
            out.push_str("Recent:\n");
            for i in finished {
                out.push_str(&format!(
                    "- {} {} {}{}{} — {}\n",
                    i.status.as_str(),
                    i.kind,
                    i.path.as_deref().unwrap_or(&i.id),
                    i.cost_usd
                        .map(|c| format!(" ({})", fmt_usd(c)))
                        .unwrap_or_default(),
                    i.error
                        .as_deref()
                        .map(|e| format!(" [{e}]"))
                        .unwrap_or_default(),
                    short(&i.prompt)
                ));
            }
        }
        Ok(out.trim_end().to_string())
    }
}

fn short(text: &str) -> String {
    let t: String = text.chars().take(60).collect();
    if t.len() < text.len() {
        format!("{t}…")
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AskerSpendConfirm, NoConfirmSink};
    use harness_tools::{Question, QuestionAnswer, QuestionAsker};
    use serde_json::json;
    use std::sync::Mutex;

    fn catalog() -> Catalog {
        Catalog::from_json(&json!({"data": [
            {"id": "flux-mini", "endpoint": "/images/generate",
             "pricing": {"cost_per_image": 0.01},
             "request_schema": {"required": ["prompt"], "properties": {
                "prompt": {"type": "string", "description": "Use @Image1 for references."},
                "input_image": {"type": "array", "items": {"type": "string"}},
                "aspect_ratio": {"type": "string", "enum": ["1:1", "16:9"], "default": "16:9"},
                "seed": {"type": "integer"},
                "num_inference_steps": {"type": "integer", "minimum": 1, "maximum": 50, "default": 28}}}},
            {"id": "pricey", "endpoint": "/images/generate",
             "pricing": {"cost_per_image": 2.0},
             "request_schema": {"properties": {"prompt": {"type": "string"}}}},
            {"id": "veo", "endpoint": "/videos/generate",
             "pricing": {"cost_per_second": 0.1},
             "request_schema": {"properties": {
                "prompt": {"type": "string"},
                "input_image": {"type": "string"},
                "tail_image_url": {"type": "string"},
                "duration": {"type": "integer", "minimum": 4, "maximum": 8, "default": 8},
                "resolution": {"type": "string", "enum": ["720p", "1080p"], "default": "720p"}}}},
            // Seedance 2.5 reference-to-video's shape: face media in their
            // own fields, numbered before the rest of their kind, and a
            // duration that is seconds or "auto".
            {"id": "seedance-ref", "endpoint": "/videos/generate",
             "pricing": {"cost_per_second": 0.1},
             "request_schema": {"properties": {
                "prompt": {"type": "string", "x-order": 0,
                    "description": "Use @Image1, @Video1 to reference inputs.",
                    "x-media-references": {"groups": [
                        {"field": "input_face_images", "pattern": "[Image {n}]"},
                        {"field": "input_images", "pattern": "[Image {n}]"},
                        {"field": "input_face_videos", "pattern": "[Video {n}]"},
                        {"field": "input_videos", "pattern": "[Video {n}]"}]}},
                "input_face_images": {"type": "array", "x-order": 1,
                    "items": {"type": "string", "format": "uri"}},
                "input_images": {"type": "array", "x-order": 2,
                    "items": {"type": "string", "format": "uri"}},
                "input_face_videos": {"type": "array", "x-order": 3,
                    "items": {"type": "string", "format": "uri"}},
                "input_videos": {"type": "array", "x-order": 4,
                    "items": {"type": "string", "format": "uri"}},
                "duration": {"type": ["integer", "string"], "minimum": 4, "maximum": 30,
                    "default": "auto", "x-order": 9}}}}
        ]}))
    }

    fn video_request(model: &str, prompt: &str) -> GenerateRequest {
        GenerateRequest {
            kind: MediaKind::Video,
            prompt: prompt.into(),
            model: Some(model.into()),
            refs: vec![],
            typed: vec![],
            count: 1,
            name: None,
            parent: None,
            extra: Map::new(),
            wait: false,
        }
    }

    #[tokio::test]
    async fn a_union_typed_duration_is_sent_as_an_integer() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(
            dir.path(),
            "http://127.0.0.1:9/api/ai",
            Arc::new(NoConfirmSink),
            MediaPrefs::default(),
        );
        let cat = catalog();
        // The tool's duration is a float; seedance rejects `5.0`.
        let req = GenerateRequest {
            typed: vec![("duration", json!(5.0))],
            ..video_request("seedance-ref", "x")
        };
        let p = prepare(&ctx, &cat, &req).unwrap();
        assert!(p.body["duration"].is_i64(), "{}", p.body["duration"]);
        assert_eq!(p.body["duration"], json!(5));

        // The string half of the union passes through, and the range still
        // holds for numbers.
        let mut extra = Map::new();
        extra.insert("duration".into(), json!("auto"));
        let p = prepare(
            &ctx,
            &cat,
            &GenerateRequest {
                extra,
                ..video_request("seedance-ref", "x")
            },
        )
        .unwrap();
        assert_eq!(p.body["duration"], json!("auto"));
        let req = GenerateRequest {
            typed: vec![("duration", json!(45.0))],
            ..video_request("seedance-ref", "x")
        };
        let err = prepare(&ctx, &cat, &req).unwrap_err();
        assert!(err.to_string().contains("≤ 30"), "{err}");
    }

    #[tokio::test]
    async fn extra_media_fields_take_project_files_and_number_faces_first() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(
            dir.path(),
            "http://127.0.0.1:9/api/ai",
            Arc::new(NoConfirmSink),
            MediaPrefs::default(),
        );
        std::fs::create_dir_all(dir.path().join("keys")).unwrap();
        for name in ["keys/k0.png", "keys/k1.png", "bg.png"] {
            std::fs::write(dir.path().join(name), png()).unwrap();
        }
        std::fs::write(dir.path().join("motion.mp4"), b"mp4").unwrap();
        assert_eq!(
            ctx.refs.stage(dir.path().join("bg.png")).as_deref(),
            Some("[Image #1]")
        );
        let cat = catalog();

        // The session that failed: keyframes and the motion clip as project
        // paths in the face fields, the background as an ordinary ref.
        let mut extra = Map::new();
        extra.insert(
            "input_face_images".into(),
            json!(["keys/k0.png", "https://cdn.test/hosted.png", "keys/k1.png"]),
        );
        extra.insert("input_face_videos".into(), json!(["motion.mp4"]));
        let req = GenerateRequest {
            extra,
            ..video_request(
                "seedance-ref",
                "the room from [Image #1], moving like @Video1",
            )
        };
        let p = prepare(&ctx, &cat, &req).unwrap();
        assert_eq!(p.resolved.len(), 4, "every local file is uploaded");
        assert!(
            p.body.get("input_face_images").is_none()
                && p.params.get("input_face_images").is_none(),
            "media fields land after upload and never reach the manifest params"
        );

        let urls: std::collections::HashMap<PathBuf, String> = p
            .resolved
            .iter()
            .map(|r| (r.path.clone(), format!("https://hub.test/{}", r.filename)))
            .collect();
        let mut body = p.body.clone();
        let prompt = place_refs(
            &p.model,
            &p.resolved,
            &p.fields,
            &urls,
            &req.prompt,
            &mut body,
        )
        .unwrap();
        assert_eq!(
            body["input_face_images"],
            json!([
                "https://hub.test/k0.png",
                "https://cdn.test/hosted.png",
                "https://hub.test/k1.png"
            ])
        );
        assert_eq!(
            body["input_face_videos"],
            json!(["https://hub.test/motion.mp4"])
        );
        assert_eq!(body["input_images"], json!(["https://hub.test/bg.png"]));
        // Face images are numbered first, so the background is the fourth.
        assert_eq!(prompt, "the room from @Image4, moving like @Video1");

        // A label named in a face field goes there and nowhere else, even
        // when the prompt mentions it too.
        let mut extra = Map::new();
        extra.insert("input_face_images".into(), json!(["[Image #1]"]));
        let req = GenerateRequest {
            extra,
            ..video_request("seedance-ref", "the boy in [Image #1]")
        };
        let p = prepare(&ctx, &cat, &req).unwrap();
        let urls = p
            .resolved
            .iter()
            .map(|r| (r.path.clone(), "https://hub.test/bg.png".to_string()))
            .collect();
        let mut body = p.body.clone();
        let prompt = place_refs(
            &p.model,
            &p.resolved,
            &p.fields,
            &urls,
            &req.prompt,
            &mut body,
        )
        .unwrap();
        assert_eq!(
            body["input_face_images"],
            json!(["https://hub.test/bg.png"])
        );
        assert!(body.get("input_images").is_none(), "{body:?}");
        assert_eq!(prompt, "the boy in @Image1");

        // A clip in an image field is refused before anything is uploaded.
        let mut extra = Map::new();
        extra.insert("input_face_images".into(), json!(["motion.mp4"]));
        let err = prepare(
            &ctx,
            &cat,
            &GenerateRequest {
                extra,
                ..video_request("seedance-ref", "x")
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("takes image references"), "{err}");
    }

    struct RecordingAsker(Mutex<Vec<String>>, bool);

    #[async_trait]
    impl QuestionAsker for RecordingAsker {
        async fn ask(
            &self,
            questions: &[Question],
        ) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
            self.0.lock().unwrap().push(questions[0].question.clone());
            Ok(Some(vec![QuestionAnswer {
                header: "Spend".into(),
                question: questions[0].question.clone(),
                selected: vec![if self.1 {
                    "Generate".into()
                } else {
                    "Skip".into()
                }],
            }]))
        }
    }

    fn ctx_for(
        root: &std::path::Path,
        base_url: &str,
        sink: Arc<dyn MediaSink>,
        prefs: MediaPrefs,
    ) -> Arc<MediaContext> {
        let refs = Arc::new(MediaRefs::new());
        let library = Arc::new(MediaLibrary::new(root, "generations"));
        Arc::new(
            MediaContext::new(
                "s1",
                root,
                prefs,
                Some(MediaApi {
                    base_url: base_url.to_string(),
                    api_key: "k".into(),
                }),
                refs,
                library,
                sink,
            )
            .with_catalog(catalog()),
        )
    }

    fn png() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(4, 2, image::Rgb([200, 30, 30]))
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    async fn hub_ok(server: &mut mockito::ServerGuard, id: &str) -> Vec<mockito::Mock> {
        let result_url = format!("{}/result/{id}.png", server.url());
        vec![
            server
                .mock("POST", "/api/ai/queue")
                .with_body(
                    json!({"generations": [{"generation_id": id, "status": "queued"}]}).to_string(),
                )
                .create_async()
                .await,
            server
                .mock("GET", format!("/api/ai/queue/{id}").as_str())
                .with_body(
                    json!({"generation_id": id, "status": "succeeded", "media_type": "image",
                           "result_url": result_url, "seed": 7})
                    .to_string(),
                )
                .create_async()
                .await,
            server
                .mock("GET", format!("/result/{id}.png").as_str())
                .with_body(png())
                .create_async()
                .await,
        ]
    }

    #[tokio::test]
    async fn generates_an_image_saves_it_and_attaches_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new_async().await;
        let base = format!("{}/api/ai", server.url());
        let ctx = ctx_for(
            dir.path(),
            &base,
            Arc::new(NoConfirmSink),
            MediaPrefs::default(),
        );
        // A reference the user attached, mentioned only in the prompt: it is
        // uploaded to the hub's playground and its presigned URL goes into
        // the request.
        let reference = dir.path().join("ref.png");
        std::fs::write(&reference, png()).unwrap();
        assert_eq!(ctx.refs.stage(&reference).as_deref(), Some("[Image #1]"));
        let upload_mocks = vec![
            server
                .mock("GET", "/api/users/me")
                .with_body(json!({"user": {"username": "greg"}}).to_string())
                .create_async()
                .await,
            server
                .mock("GET", "/api/repos/greg/playground")
                .with_body(json!({"repository": {"name": "playground"}}).to_string())
                .create_async()
                .await,
            server
                .mock("POST", "/api/repos/greg/playground/workbench/context")
                .with_body(json!({"status": "success", "asset_path": "oxen/context/ref.png"}).to_string())
                .expect(1)
                .create_async()
                .await,
            server
                .mock("GET", "/api/repos/greg/playground/file/presigned_url/main/oxen/context/ref.png")
                .with_body(json!({"url": "https://hub.test/api/repos/greg/playground/file/main/oxen/context/ref.png?oxen_signature=s"}).to_string())
                .create_async()
                .await,
            server
                .mock("POST", "/api/ai/queue")
                .match_body(mockito::Matcher::PartialJson(json!({
                    "input_image": ["https://hub.test/api/repos/greg/playground/file/main/oxen/context/ref.png?oxen_signature=s"],
                    "prompt": "an ox like @Image1"
                })))
                .with_body(json!({"generations": [{"generation_id": "g1", "status": "queued"}]}).to_string())
                .expect(1)
                .create_async()
                .await,
        ];
        // Registered after the specific enqueue matcher above, so that one
        // wins (mockito matches in registration order).
        let mocks = hub_ok(&mut server, "g1").await;

        let tool = GenerateImageTool { ctx: ctx.clone() };
        // A lane shares its parent's tools: the call names the chat, not the
        // context the tools were built for.
        let call = CallContext::new("lane-7", Some(12), "call_42");
        let out = tool
            .invoke_from(
                json!({"prompt": "an ox like [Image #1]", "model": "flux-mini", "aspect_ratio": "1:1", "name": "Big Ox"}),
                &call,
            )
            .await
            .unwrap();
        assert!(out.starts_with("Generated 1 image with flux-mini"), "{out}");
        assert!(
            out.contains("generations/") && out.contains("-big-ox-1.png (4×2)"),
            "{out}"
        );
        let marker_path = out
            .split("<<attach-image:")
            .nth(1)
            .and_then(|m| m.split(':').nth(1))
            .map(|p| p.trim_end_matches(">>").to_string())
            .expect("marker");
        assert!(
            std::path::Path::new(&marker_path).is_absolute(),
            "{marker_path}"
        );
        assert!(std::path::Path::new(&marker_path).exists(), "{marker_path}");
        for m in upload_mocks {
            m.assert_async().await;
        }
        drop(mocks);
        assert!(
            ctx.library.uploads().is_empty(),
            "upload rows clear once the batch is recorded"
        );
        let items = ctx.library.items();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.status, MediaStatus::Succeeded);
        assert_eq!(item.session, "lane-7");
        assert_eq!(item.turn_seq, Some(12));
        assert_eq!(item.call_id.as_deref(), Some("call_42"));
        assert_eq!(item.cost_usd, Some(0.01));
        assert_eq!(item.params["aspect_ratio"], "1:1");
        assert_eq!(item.seed, Some(json!(7)));
        assert_eq!(item.refs.len(), 1);
        assert!(item.refs[0].starts_with("generations/refs/"));
        assert!(dir.path().join(item.path.as_ref().unwrap()).exists());
        assert!(dir.path().join("generations/manifest.jsonl").exists());

        // Provenance: the reference traces to the chip and the file the
        // user attached, the prompt the agent wrote survives the rewrite,
        // and the hub's record rides along verbatim.
        assert_eq!(item.sources.len(), 1);
        let source = &item.sources[0];
        assert_eq!(source.path, item.refs[0]);
        assert_eq!(source.origin, SourceOrigin::Attachment);
        assert_eq!(source.label.as_deref(), Some("[Image #1]"));
        assert_eq!(source.source, "ref.png", "inside the project → relative");
        assert_eq!(source.generation, None);
        assert_eq!(source.kind, "image");
        assert_eq!(source.sha256.len(), 64);
        assert_eq!(item.agent_prompt.as_deref(), Some("an ox like [Image #1]"));
        assert_eq!(item.prompt, "an ox like @Image1");
        let provider = item.provider.as_ref().expect("hub record kept");
        assert_eq!(provider["status"], "succeeded");
        assert_eq!(provider["seed"], 7);

        // A generation used as a reference traces back to its own item,
        // even when it reached the request through a chip.
        let out_path = dir.path().join(item.path.as_ref().unwrap());
        assert_eq!(ctx.refs.stage(&out_path).as_deref(), Some("[Image #2]"));
        let r = refs::resolve(&["[Image #2]".into()], &ctx.refs, dir.path()).unwrap();
        let stored = ctx.library.store_ref(&r[0].path).unwrap();
        let traced = source_of(&ctx, &r[0], stored);
        assert_eq!(traced.origin, SourceOrigin::Generation);
        assert_eq!(traced.generation.as_deref(), Some("g1"));
        assert_eq!(traced.source, item.path.clone().unwrap());
        assert_eq!(traced.label.as_deref(), Some("[Image #2]"));
        let plain = dir.path().join("plain.png");
        std::fs::write(&plain, png()).unwrap();
        let r = refs::resolve(&["plain.png".into()], &ctx.refs, dir.path()).unwrap();
        let stored = ctx.library.store_ref(&r[0].path).unwrap();
        assert_eq!(source_of(&ctx, &r[0], stored).origin, SourceOrigin::File);
    }

    #[tokio::test]
    async fn hub_repo_becomes_the_queue_target() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new_async().await;
        let result_url = format!("{}/result/g7.png", server.url());
        let target = server
            .mock("POST", "/api/ai/queue")
            .match_body(mockito::Matcher::PartialJson(json!({
                "target_namespace": "ox", "target_repo": "art", "target_directory": "generations"
            })))
            .with_body(
                json!({"generations": [{"generation_id": "g7", "status": "queued"}]}).to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        server
            .mock("GET", "/api/ai/queue/g7")
            .with_body(
                json!({"generation_id": "g7", "status": "succeeded", "media_type": "image",
                       "result_url": result_url})
                .to_string(),
            )
            .create_async()
            .await;
        server
            .mock("GET", "/result/g7.png")
            .with_body(png())
            .create_async()
            .await;
        let base = format!("{}/api/ai", server.url());
        let prefs = MediaPrefs {
            hub_repo: Some("ox/art".into()),
            ..MediaPrefs::default()
        };
        let ctx = ctx_for(dir.path(), &base, Arc::new(NoConfirmSink), prefs);
        let out = GenerateImageTool { ctx }
            .invoke(json!({"prompt": "an ox", "model": "flux-mini"}))
            .await
            .unwrap();
        assert!(out.starts_with("Generated 1 image"), "{out}");
        target.assert_async().await;
    }

    #[tokio::test]
    async fn a_switched_destination_applies_to_the_next_generation() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new_async().await;
        let mut enqueues = Vec::new();
        for (id, ns, repo) in [("g1", "ox", "art"), ("g2", "studio", "shots")] {
            let result_url = format!("{}/result/{id}.png", server.url());
            enqueues.push(
                server
                    .mock("POST", "/api/ai/queue")
                    .match_body(mockito::Matcher::PartialJson(
                        json!({"target_namespace": ns, "target_repo": repo}),
                    ))
                    .with_body(
                        json!({"generations": [{"generation_id": id, "status": "queued"}]})
                            .to_string(),
                    )
                    .expect(1)
                    .create_async()
                    .await,
            );
            server
                .mock("GET", format!("/api/ai/queue/{id}").as_str())
                .with_body(
                    json!({"generation_id": id, "status": "succeeded", "media_type": "image",
                           "result_url": result_url})
                    .to_string(),
                )
                .create_async()
                .await;
            server
                .mock("GET", format!("/result/{id}.png").as_str())
                .with_body(png())
                .create_async()
                .await;
        }
        let picked = Arc::new(Mutex::new(Some(("ox".to_string(), "art".to_string()))));
        let resolve = picked.clone();
        let ctx = Arc::new(
            MediaContext::new(
                "s1",
                dir.path(),
                // The chat started with a different repo; the resolver wins.
                MediaPrefs {
                    hub_repo: Some("old/place".into()),
                    ..MediaPrefs::default()
                },
                Some(MediaApi {
                    base_url: format!("{}/api/ai", server.url()),
                    api_key: "k".into(),
                }),
                Arc::new(MediaRefs::new()),
                Arc::new(MediaLibrary::new(dir.path(), "generations")),
                Arc::new(NoConfirmSink),
            )
            .with_catalog(catalog())
            .with_destination(Arc::new(move || resolve.lock().unwrap().clone())),
        );
        let tool = GenerateImageTool { ctx };
        tool.invoke(json!({"prompt": "an ox", "model": "flux-mini"}))
            .await
            .unwrap();
        *picked.lock().unwrap() = Some(("studio".into(), "shots".into()));
        tool.invoke(json!({"prompt": "an ox", "model": "flux-mini"}))
            .await
            .unwrap();
        for enqueue in enqueues {
            enqueue.assert_async().await;
        }
    }

    #[tokio::test]
    async fn over_budget_asks_and_a_decline_is_reported_not_retried() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new_async().await;
        let enqueue = server
            .mock("POST", "/api/ai/queue")
            .expect(0)
            .create_async()
            .await;
        let base = format!("{}/api/ai", server.url());
        let asker = Arc::new(RecordingAsker(Mutex::new(vec![]), false));
        let sink = Arc::new(AskerSpendConfirm(asker.clone()));
        let ctx = ctx_for(dir.path(), &base, sink, MediaPrefs::default());
        let tool = GenerateImageTool { ctx };
        let out = tool
            .invoke(json!({"prompt": "a mural", "model": "pricey"}))
            .await
            .unwrap();
        assert!(out.contains("declined"), "{out}");
        let asked = asker.0.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert!(
            asked[0].contains("about $2.00") && asked[0].contains("per-generation"),
            "{}",
            asked[0]
        );
        enqueue.assert_async().await;
    }

    #[tokio::test]
    async fn no_interactive_user_refuses_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(
            dir.path(),
            "http://127.0.0.1:9/api/ai",
            Arc::new(NoConfirmSink),
            MediaPrefs::default(),
        );
        let tool = GenerateImageTool { ctx };
        let err = tool
            .invoke(json!({"prompt": "a mural", "model": "pricey"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no user is available"), "{err}");
    }

    #[tokio::test]
    async fn validates_params_against_the_schema() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(
            dir.path(),
            "http://127.0.0.1:9/api/ai",
            Arc::new(NoConfirmSink),
            MediaPrefs::default(),
        );
        let tool = GenerateImageTool { ctx: ctx.clone() };
        let err = tool
            .invoke(json!({"prompt": "x", "model": "flux-mini", "aspect_ratio": "4:3"}))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("must be one of 1:1, 16:9"),
            "{err}"
        );
        let err = tool
            .invoke(
                json!({"prompt": "x", "model": "flux-mini", "extra": {"num_inference_steps": 99}}),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("≤ 50"), "{err}");
        let err = tool
            .invoke(json!({"prompt": "x", "model": "flux-mini", "extra": {"bogus": 1}}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no `bogus` parameter"), "{err}");
        let err = tool
            .invoke(json!({"prompt": "x", "model": "flux-mini", "extra": {"input_image": ["[Image #1]"]}}))
            .await
            .unwrap_err();
        // A media field in `extra` resolves its labels like `refs` does.
        assert!(
            err.to_string()
                .contains("isn't an attachment the user has provided"),
            "{err}"
        );
        let err = tool
            .invoke(json!({"prompt": "x", "model": "veo"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("use generate_video"), "{err}");
        let err = tool
            .invoke(json!({"prompt": "x", "model": "nope-9000"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no model `nope-9000`"), "{err}");
    }

    #[tokio::test]
    async fn references_land_in_first_and_tail_frame_slots() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(
            dir.path(),
            "http://127.0.0.1:9/api/ai",
            Arc::new(NoConfirmSink),
            MediaPrefs::default(),
        );
        for name in ["a.png", "b.png", "c.png"] {
            std::fs::write(dir.path().join(name), png()).unwrap();
            ctx.refs.stage(dir.path().join(name));
        }
        let cat = catalog();
        let req = GenerateRequest {
            kind: MediaKind::Video,
            prompt: "start on [Image #1], end on [Image #2]".into(),
            model: Some("veo".into()),
            refs: vec![],
            typed: vec![("duration", json!(6.4))],
            count: 1,
            name: None,
            parent: None,
            extra: Map::new(),
            wait: true,
        };
        let p = prepare(&ctx, &cat, &req).unwrap();
        assert_eq!(p.resolved.len(), 2, "labels in the prompt are picked up");
        assert_eq!(p.body["duration"], json!(6));
        assert!((p.per_output.unwrap() - 0.6).abs() < 1e-9);
        assert!(
            p.body.get("input_image").is_none(),
            "refs land after upload"
        );

        // Uploaded URLs go to the first-frame and tail-frame fields, and the
        // prompt's labels become positional mentions.
        let urls: std::collections::HashMap<PathBuf, String> = p
            .resolved
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    r.path.clone(),
                    format!("https://hub.test/f/{i}.png?oxen_signature=s"),
                )
            })
            .collect();
        let mut body = p.body.clone();
        let prompt = place_refs(
            &p.model,
            &p.resolved,
            &p.fields,
            &urls,
            &req.prompt,
            &mut body,
        )
        .unwrap();
        assert_eq!(
            body["input_image"],
            json!("https://hub.test/f/0.png?oxen_signature=s")
        );
        assert_eq!(
            body["tail_image_url"],
            json!("https://hub.test/f/1.png?oxen_signature=s")
        );
        assert_eq!(
            prompt,
            "start on reference image 1, end on reference image 2"
        );
        assert!(
            p.params.get("input_image").is_none(),
            "refs never reach the manifest params"
        );

        // Three images is one too many for a first/last-frame model.
        let req = GenerateRequest {
            refs: vec![
                "[Image #1]".into(),
                "[Image #2]".into(),
                "[Image #3]".into(),
            ],
            prompt: "x".into(),
            ..req
        };
        let err = prepare(&ctx, &cat, &req).unwrap_err();
        assert!(err.to_string().contains("at most two"), "{err}");

        // An array slot takes every image; @-style models get @Image1.
        let req = GenerateRequest {
            kind: MediaKind::Image,
            model: Some("flux-mini".into()),
            refs: vec![],
            prompt: "like [Image #1] with @Image1 style".into(),
            typed: vec![],
            count: 1,
            name: None,
            parent: None,
            extra: Map::new(),
            wait: true,
        };
        let p = prepare(&ctx, &cat, &req).unwrap();
        let urls: std::collections::HashMap<PathBuf, String> = p
            .resolved
            .iter()
            .map(|r| (r.path.clone(), "https://hub.test/a.png".to_string()))
            .collect();
        let mut body = p.body.clone();
        let prompt = place_refs(
            &p.model,
            &p.resolved,
            &p.fields,
            &urls,
            &req.prompt,
            &mut body,
        )
        .unwrap();
        assert_eq!(prompt, "like @Image1 with @Image1 style");
        assert_eq!(body["input_image"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn background_video_reports_through_an_aside() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new_async().await;
        let result_url = format!("{}/result/v1.mp4", server.url());
        server
            .mock("POST", "/api/ai/queue")
            .with_body(
                json!({"generations": [{"generation_id": "v1", "status": "queued"}]}).to_string(),
            )
            .create_async()
            .await;
        server
            .mock("GET", "/api/ai/queue/v1")
            .with_body(json!({"generation_id": "v1", "status": "succeeded", "media_type": "video", "result_url": result_url}).to_string())
            .create_async()
            .await;
        server
            .mock("GET", "/result/v1.mp4")
            .with_body(b"\x00\x00\x00\x18ftypisom-fake-video")
            .create_async()
            .await;
        let base = format!("{}/api/ai", server.url());
        let asides = Asides::default();
        let refs = Arc::new(MediaRefs::new());
        let library = Arc::new(MediaLibrary::new(dir.path(), "generations"));
        let prefs = MediaPrefs {
            per_generation_usd: Some(5.0),
            per_run_usd: Some(5.0),
            ..MediaPrefs::default()
        };
        let ctx = Arc::new(
            MediaContext::new(
                "s1",
                dir.path(),
                prefs,
                Some(MediaApi {
                    base_url: base,
                    api_key: "k".into(),
                }),
                refs,
                library,
                Arc::new(NoConfirmSink),
            )
            .with_catalog(catalog())
            .with_asides(asides.clone()),
        );
        let tool = GenerateVideoTool { ctx: ctx.clone() };
        let out = tool
            .invoke(json!({"prompt": "a balloon rises", "model": "veo", "duration": 4, "resolution": "720p"}))
            .await
            .unwrap();
        assert!(
            out.starts_with("Queued 1 video generation(s) with veo"),
            "{out}"
        );
        assert!(out.contains("do not poll"), "{out}");
        // The queued row is visible right away.
        assert_eq!(ctx.library.in_flight(Some("s1")).len(), 1);

        let deadline = Instant::now() + Duration::from_secs(10);
        while asides.is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let delivered = asides.take_all();
        assert_eq!(delivered.len(), 1, "aside not delivered");
        assert_eq!(delivered[0].kind, "media");
        assert!(
            delivered[0].title.contains("1/1 ready"),
            "{}",
            delivered[0].title
        );
        assert!(
            delivered[0].body.contains("-a-balloon-rises-1.mp4"),
            "{}",
            delivered[0].body
        );
        let item = &ctx.library.items()[0];
        assert_eq!(item.status, MediaStatus::Succeeded);
        assert_eq!(item.duration_secs, Some(4.0));
        assert!((item.cost_usd.unwrap() - 0.4).abs() < 1e-9);
    }

    /// A local provider with one installed model (which takes references)
    /// and one that isn't installed; records every job it runs.
    #[derive(Default)]
    struct FakeLocal {
        jobs: Mutex<Vec<LocalJob>>,
        cancelled: Mutex<Vec<String>>,
        /// When set, `generate` reports a memory warning and records what
        /// the library row showed mid-render.
        watch: Mutex<Option<Arc<MediaLibrary>>>,
        mid_render: Mutex<Vec<Option<MediaProgress>>>,
    }

    fn local_model(id: &str, installed: bool) -> MediaModel {
        MediaModel {
            id: id.into(),
            kind: MediaKind::Image,
            display_name: None,
            developer: None,
            summary: None,
            description: None,
            inputs: vec!["text".into(), "image".into()],
            pricing: Value::Null,
            request_schema: json!({"required": ["prompt"], "properties": {
                "prompt": {"type": "string"},
                "aspect_ratio": {"type": "string", "enum": ["1:1", "16:9"]},
                "seed": {"type": "integer"},
                "steps": {"type": "integer", "minimum": 1, "maximum": 8},
                "input_images": {"type": "array", "items": {"type": "string"}}}}),
            local: Some(crate::LocalInfo {
                installed,
                engine: "fake".into(),
                license: "Apache-2.0".into(),
                download_bytes: if installed { 0 } else { 5_000_000_000 },
                typical_secs: Some(20.0),
            }),
        }
    }

    #[async_trait]
    impl LocalMedia for FakeLocal {
        fn models(&self) -> Vec<MediaModel> {
            vec![
                local_model("local/fake", true),
                local_model("local/missing", false),
            ]
        }
        async fn generate(
            &self,
            job: LocalJob,
            progress: Arc<dyn Fn(LocalProgress) + Send + Sync>,
        ) -> Result<crate::LocalOutput, LocalMediaError> {
            let watched = self.watch.lock().unwrap().clone();
            if let Some(lib) = watched {
                progress(LocalProgress::Loading);
                progress(LocalProgress::Warning("memory is tight".into()));
                progress(LocalProgress::Step { step: 1, total: 4 });
                let row = lib.items().into_iter().find(|i| i.id == job.id);
                self.mid_render
                    .lock()
                    .unwrap()
                    .push(row.and_then(|r| r.progress));
            } else {
                progress(LocalProgress::Step { step: 1, total: 4 });
            }
            let seed = job.params.get("seed").cloned();
            self.jobs.lock().unwrap().push(job);
            Ok(crate::LocalOutput {
                bytes: png(),
                seed,
                record: json!({"engine": "fake", "elapsed_ms": 5}),
            })
        }
        /// Knows only ids it is "running" (any without `queued` in them).
        async fn cancel(&self, id: &str) -> bool {
            self.cancelled.lock().unwrap().push(id.to_string());
            !id.contains("queued")
        }
    }

    fn local_ctx(
        root: &std::path::Path,
        local: Arc<FakeLocal>,
        sink: Arc<dyn MediaSink>,
    ) -> Arc<MediaContext> {
        Arc::new(
            MediaContext::new(
                "s1",
                root,
                MediaPrefs::default(),
                None,
                Arc::new(MediaRefs::new()),
                Arc::new(MediaLibrary::new(root, "generations")),
                sink,
            )
            .with_catalog(catalog())
            .with_local(Some(local)),
        )
    }

    #[tokio::test]
    async fn local_models_run_without_a_key_upload_or_spend_prompt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sketch.png"), png()).unwrap();
        let local = Arc::new(FakeLocal::default());
        let asker = Arc::new(RecordingAsker(Mutex::new(vec![]), false));
        let ctx = local_ctx(
            dir.path(),
            local.clone(),
            Arc::new(AskerSpendConfirm(asker.clone())),
        );
        ctx.refs
            .stage_as("[Image #1]", dir.path().join("sketch.png"));
        let out = GenerateImageTool { ctx: ctx.clone() }
            .invoke(json!({
                "prompt": "ink [Image #1] in watercolor",
                "model": "local/fake",
                "seed": 5,
                "count": 2,
                "extra": {"steps": 2}
            }))
            .await
            .unwrap();
        assert!(
            out.starts_with("Generated 2 images with local/fake"),
            "{out}"
        );
        assert!(
            asker.0.lock().unwrap().is_empty(),
            "local generation never asks to spend"
        );
        let jobs = local.jobs.lock().unwrap().clone();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].prompt, "ink reference image 1 in watercolor");
        assert_eq!(jobs[0].refs, vec![dir.path().join("sketch.png")]);
        assert_eq!(jobs[0].params.get("steps"), Some(&json!(2)));
        // Variations step a fixed seed.
        assert_eq!(jobs[0].params.get("seed"), Some(&json!(5)));
        assert_eq!(jobs[1].params.get("seed"), Some(&json!(6)));
        let items = ctx.library.items();
        assert_eq!(items.len(), 2);
        for item in &items {
            assert_eq!(item.status, MediaStatus::Succeeded);
            assert!(item.id.starts_with("local-"), "{}", item.id);
            assert_eq!(item.cost_usd, None);
            assert_eq!(
                item.provider,
                Some(json!({"engine": "fake", "elapsed_ms": 5}))
            );
            assert_eq!(
                item.sources.len(),
                1,
                "the reference is kept for provenance"
            );
            assert!(dir.path().join(item.path.as_ref().unwrap()).exists());
        }
    }

    #[tokio::test]
    async fn local_rows_show_progress_and_warnings_reach_the_report() {
        let dir = tempfile::tempdir().unwrap();
        let local = Arc::new(FakeLocal::default());
        let ctx = local_ctx(dir.path(), local.clone(), Arc::new(NoConfirmSink));
        *local.watch.lock().unwrap() = Some(ctx.library.clone());
        let out = GenerateImageTool { ctx: ctx.clone() }
            .invoke(json!({"prompt": "x", "model": "local/fake"}))
            .await
            .unwrap();
        assert!(out.contains("Note: memory is tight"), "{out}");
        let seen = local.mid_render.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![Some(MediaProgress {
                phase: "sampling".into(),
                step: Some(1),
                total: Some(4),
                note: Some("memory is tight".into()),
            })]
        );
        let done = &ctx.library.items()[0];
        assert_eq!(done.status, MediaStatus::Succeeded);
        assert_eq!(done.progress, None, "a settled row drops its progress");
    }

    #[tokio::test]
    async fn a_queued_local_output_cancels_from_its_row() {
        let dir = tempfile::tempdir().unwrap();
        let local = Arc::new(FakeLocal::default());
        let ctx = local_ctx(dir.path(), local.clone(), Arc::new(NoConfirmSink));
        // A batch's second output waits in the library, unknown to the provider.
        GenerateImageTool { ctx: ctx.clone() }
            .invoke(json!({"prompt": "x", "model": "local/fake"}))
            .await
            .unwrap();
        let mut queued = ctx.library.items()[0].clone();
        queued.id = "local-queued-2".into();
        queued.status = MediaStatus::Queued;
        ctx.library.record(queued).unwrap();
        let status = MediaStatusTool { ctx: ctx.clone() };
        status
            .invoke(json!({"id": "local-queued-2", "cancel": true}))
            .await
            .unwrap();
        let row = ctx
            .library
            .items()
            .into_iter()
            .find(|i| i.id == "local-queued-2");
        assert_eq!(row.map(|i| i.status), Some(MediaStatus::Cancelled));
        // An id neither running nor queued is an error, not a fake success.
        let err = status
            .invoke(json!({"id": "local-queued-9", "cancel": true}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("queued or running"), "{err}");
    }

    #[tokio::test]
    async fn a_seed_at_the_top_of_the_range_wraps_for_variations() {
        let dir = tempfile::tempdir().unwrap();
        let local = Arc::new(FakeLocal::default());
        let ctx = local_ctx(dir.path(), local.clone(), Arc::new(NoConfirmSink));
        GenerateImageTool { ctx }
            .invoke(json!({"prompt": "x", "model": "local/fake", "seed": u64::MAX, "count": 2}))
            .await
            .unwrap();
        let seeds: Vec<_> = local
            .jobs
            .lock()
            .unwrap()
            .iter()
            .map(|j| j.params.get("seed").cloned())
            .collect();
        // The schema check clamps to i64::MAX; the next variation wraps to 0.
        assert_eq!(seeds, vec![Some(json!(i64::MAX)), Some(json!(0))]);
    }

    #[tokio::test]
    async fn without_a_key_the_default_falls_back_to_an_installed_local_model() {
        let dir = tempfile::tempdir().unwrap();
        let local = Arc::new(FakeLocal::default());
        let ctx = local_ctx(dir.path(), local.clone(), Arc::new(NoConfirmSink));
        let out = GenerateImageTool { ctx }
            .invoke(json!({"prompt": "a fox"}))
            .await
            .unwrap();
        assert!(out.contains("with local/fake"), "{out}");
        assert!(out.contains("no Oxen API key is set"), "{out}");
        assert_eq!(local.jobs.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_uninstalled_local_model_says_how_to_install_it() {
        let dir = tempfile::tempdir().unwrap();
        let local = Arc::new(FakeLocal::default());
        let ctx = local_ctx(dir.path(), local.clone(), Arc::new(NoConfirmSink));
        let err = GenerateImageTool { ctx }
            .invoke(json!({"prompt": "a fox", "model": "local/missing"}))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("oxen-harness media pull local/missing"),
            "{err}"
        );
        assert!(local.jobs.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn local_generations_cancel_through_the_provider() {
        let dir = tempfile::tempdir().unwrap();
        let local = Arc::new(FakeLocal::default());
        let ctx = local_ctx(dir.path(), local.clone(), Arc::new(NoConfirmSink));
        let out = MediaStatusTool { ctx: ctx.clone() }
            .invoke(json!({"id": "local-abc-1", "cancel": true}))
            .await
            .unwrap();
        assert!(out.contains("Cancelled"), "{out}");
        assert_eq!(local.cancelled.lock().unwrap().clone(), vec!["local-abc-1"]);
        let listed = MediaModelsTool { ctx }
            .invoke(json!({"query": "local"}))
            .await
            .unwrap();
        assert!(
            listed.contains("local/fake — free, runs on this machine"),
            "{listed}"
        );
        assert!(listed.contains("not installed"), "{listed}");
    }

    #[tokio::test]
    async fn missing_key_answers_with_the_stable_sentence() {
        let dir = tempfile::tempdir().unwrap();
        let refs = Arc::new(MediaRefs::new());
        let library = Arc::new(MediaLibrary::new(dir.path(), "generations"));
        let ctx = Arc::new(
            MediaContext::new(
                "s1",
                dir.path(),
                MediaPrefs::default(),
                None,
                refs,
                library,
                Arc::new(NoConfirmSink),
            )
            .with_catalog(catalog()),
        );
        let out = GenerateImageTool { ctx }
            .invoke(json!({"prompt": "x"}))
            .await
            .unwrap();
        assert!(out.starts_with(MEDIA_NO_KEY), "{out}");
    }

    #[tokio::test]
    async fn models_tool_lists_and_describes() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(
            dir.path(),
            "http://127.0.0.1:9/api/ai",
            Arc::new(NoConfirmSink),
            MediaPrefs::default(),
        );
        let tool = MediaModelsTool { ctx };
        let out = tool.invoke(json!({"kind": "image"})).await.unwrap();
        assert!(out.starts_with("2 model(s) for image"), "{out}");
        assert!(out.contains("- flux-mini — $0.01/image"), "{out}");
        let out = tool.invoke(json!({"id": "veo"})).await.unwrap();
        assert!(out.contains("veo (video model, $0.10/s)"), "{out}");
        assert!(out.contains("image → input_image, tail_image_url"), "{out}");
        assert!(out.contains("duration: integer 4–8 (default 8)"), "{out}");
        let out = tool.invoke(json!({"id": "seedance-ref"})).await.unwrap();
        assert!(
            out.contains("Other media fields: input_face_images[], input_face_videos[]."),
            "{out}"
        );
        assert!(
            out.contains("duration: integer|string 4–30 (default auto)"),
            "{out}"
        );
        let err = tool.invoke(json!({"id": "flux"})).await.unwrap_err();
        assert!(err.to_string().contains("did you mean: flux-mini"), "{err}");
    }

    /// A real one-cent generation against hub.oxen.ai. Opt in with
    /// `OXEN_MEDIA_SMOKE_KEY=<key> cargo test -p harness-media -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn smoke_generates_a_real_image() {
        let Ok(key) = std::env::var("OXEN_MEDIA_SMOKE_KEY") else {
            eprintln!("OXEN_MEDIA_SMOKE_KEY not set; skipping");
            return;
        };
        // `OXEN_MEDIA_SMOKE_DIR` keeps the output in a real folder (to look
        // at it, or to seed a project for a UI check); otherwise a tempdir.
        // `OXEN_MEDIA_SMOKE_VIDEO=1` renders a short clip instead.
        let keep = std::env::var("OXEN_MEDIA_SMOKE_DIR")
            .ok()
            .map(PathBuf::from);
        let tmp = tempfile::tempdir().unwrap();
        let root = keep.unwrap_or_else(|| tmp.path().to_path_buf());
        std::fs::create_dir_all(&root).unwrap();
        let dir = root.as_path();
        let refs = Arc::new(MediaRefs::new());
        let library = Arc::new(MediaLibrary::new(dir, "generations"));
        let ctx = Arc::new(MediaContext::new(
            "smoke",
            dir,
            MediaPrefs {
                per_generation_usd: Some(2.0),
                per_run_usd: Some(4.0),
                hub_repo: std::env::var("OXEN_MEDIA_SMOKE_REPO").ok(),
                ..MediaPrefs::default()
            },
            Some(MediaApi {
                base_url: harness_core::DEFAULT_BASE_URL.to_string(),
                api_key: key,
            }),
            refs,
            library.clone(),
            Arc::new(NoConfirmSink),
        ));
        // `OXEN_MEDIA_SMOKE_REF=<image>` sends a reference through the hub
        // upload path; `OXEN_MEDIA_SMOKE_REPO=ns/repo` names the hub repo.
        let reference = std::env::var("OXEN_MEDIA_SMOKE_REF").ok();
        let mut refs_arg = Vec::new();
        if let Some(path) = &reference {
            let label = ctx.refs.stage(path).expect("a media file");
            refs_arg.push(label);
        }
        let video = std::env::var("OXEN_MEDIA_SMOKE_VIDEO").is_ok();
        let out = if video {
            GenerateVideoTool { ctx: ctx.clone() }
                .invoke(json!({
                    "prompt": "A weathered ox pulls a covered wagon across cracked salt flats at golden hour; slow tracking shot, dust trailing, long shadows.",
                    "model": "bytedance-seedance-2-0-fast-text-to-video",
                    "duration": 4,
                    "resolution": "480p",
                    "aspect_ratio": "16:9",
                    "name": "ox-wagon",
                    "wait": true
                }))
                .await
                .unwrap()
        } else {
            let prompt = if reference.is_some() {
                "The same ox and wagon as @Image1, now at night under a full moon, cool blue light, painterly realism"
            } else {
                "A weathered ox pulling a covered wagon across cracked salt flats at golden hour, low three-quarter view, 35mm, long shadows, warm ochre and dusty blue, painterly realism, fine dust in the air"
            };
            GenerateImageTool { ctx: ctx.clone() }
                .invoke(json!({
                    "prompt": prompt,
                    "model": "black-forest-labs-flux-2-klein-4b",
                    "count": if reference.is_some() { 1 } else { 2 },
                    "name": if reference.is_some() { "ox-wagon-night" } else { "ox-wagon" },
                    "aspect_ratio": "16:9",
                    "refs": refs_arg
                }))
                .await
                .unwrap()
        };
        eprintln!("{out}");
        assert!(out.starts_with("Generated "), "{out}");
        for u in ctx.library.uploads() {
            eprintln!(
                "upload {} {:?} {}/{}",
                u.filename, u.status, u.bytes_sent, u.bytes_total
            );
        }
        let item = &library.items()[0];
        assert_eq!(item.status, MediaStatus::Succeeded);
        let abs = dir.join(item.path.as_ref().unwrap());
        assert!(abs.exists());
        eprintln!(
            "saved {} ({} bytes, poster {:?})",
            abs.display(),
            item.bytes,
            item.poster
        );
    }

    #[test]
    fn output_extension_sniffs_bytes_before_trusting_the_url() {
        assert_eq!(
            output_extension(
                MediaKind::Image,
                "http://x/a.jpg?sig=1",
                &png(),
                &Value::Null
            ),
            "png"
        );
        assert_eq!(
            output_extension(MediaKind::Image, "http://x/a.webp?sig=1", b"", &Value::Null),
            "webp"
        );
        assert_eq!(
            output_extension(MediaKind::Video, "http://x/a?sig=1", b"", &Value::Null),
            "mp4"
        );
        // A video's container is read from its `ftyp` brand, whatever the URL says.
        let mp4 = b"\x00\x00\x00\x20ftypisom\x00\x00\x02\x00";
        let mov = b"\x00\x00\x00\x14ftypqt  \x00\x00\x02\x00";
        for url in ["http://x/a.mov?sig=1", "http://x/a.mp4", "http://x/a"] {
            assert_eq!(
                output_extension(MediaKind::Video, url, mp4, &Value::Null),
                "mp4"
            );
            assert_eq!(
                output_extension(MediaKind::Video, url, mov, &Value::Null),
                "mov"
            );
        }
        // Older QuickTime files open with a `moov`/`mdat` atom and no brand.
        assert_eq!(
            output_extension(
                MediaKind::Video,
                "http://x/a.mov?sig=1",
                b"\x00\x00\x00\x08wide\x00\x00\x00\x00mdat",
                &Value::Null
            ),
            "mov"
        );
        let webp = b"RIFF\x24\x00\x00\x00WEBPVP8 ";
        assert_eq!(
            output_extension(MediaKind::Image, "http://x/a.png", webp, &Value::Null),
            "webp"
        );
        assert_eq!(
            output_extension(
                MediaKind::Image,
                "http://x/a",
                b"",
                &json!({"output_format": "jpeg"})
            ),
            "jpeg"
        );
    }
}
