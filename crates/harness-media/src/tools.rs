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
    now_unix, slugify, MediaItem, MediaLibrary, MediaSource, MediaStatus, SourceOrigin, StoredRef,
};
use crate::queue::{deadline_for, QueueClient, QueueError};
use crate::refs::{self, MediaRefs, RefKind, ResolvedRef};
use crate::{
    MediaApi, MediaKind, MediaPrefs, MediaSink, GENERATE_IMAGE_TOOL, GENERATE_VIDEO_TOOL,
    MEDIA_MODELS_TOOL, MEDIA_NO_KEY, MEDIA_STATUS_TOOL, USER_AGENT,
};

/// At most this many outputs per call (the hub's `num_generations` cap).
pub const MAX_COUNT: u32 = 4;

/// Everything the four tools share for one session.
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
    batch_prefix: Option<String>,
    catalog: tokio::sync::OnceCell<Arc<Catalog>>,
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
            batch_prefix: None,
            catalog: tokio::sync::OnceCell::new(),
            feed: std::sync::OnceLock::new(),
            uploader: std::sync::OnceLock::new(),
            // The hub reads a User-Agent containing "oxen" as the Oxen CLI
            // and answers 426 when its version isn't current — so this
            // client must not say "oxen" (a result download returned the
            // CLI-out-of-date error under the obvious name).
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .build()
                .unwrap_or_default(),
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

    /// Point the catalog at a fixed copy (tests, offline).
    pub fn with_catalog(self, catalog: Catalog) -> Self {
        let _ = self.catalog.set(Arc::new(catalog));
        self
    }

    fn api(&self) -> Option<&MediaApi> {
        self.api.as_ref().filter(|a| !a.api_key.trim().is_empty())
    }

    pub async fn catalog(&self) -> Result<Arc<Catalog>, ToolError> {
        self.catalog
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

    /// The hub uploader for reference media (the user's playground repo,
    /// or the `hub_repo` they configured).
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
                        self.prefs.hub_target(),
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
        "Look up image and video generation models on Oxen.ai. With `query`/`kind`, \
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
    out.push_str("Parameters (typed ones go in the tool's own fields; the rest in `extra`):\n");
    out.push_str(&model.schema_summary());
    out
}

// ---- generate_image / generate_video ---------------------------------------

/// Arguments to `generate_image`.
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
         Costs money per image (the tool checks the user's budget and asks them \
         when a call is over it). Use `refs` for reference/edit inputs the user \
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
    notes: Vec<String>,
    per_output: Option<f64>,
    slug: String,
}

async fn run_generation(
    ctx: &Arc<MediaContext>,
    req: GenerateRequest,
    call: &CallContext,
) -> Result<String, ToolError> {
    let Some(queue) = ctx.queue() else {
        return Ok(no_key_result());
    };
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
    let ref_paths: Vec<String> = sources.iter().map(|s| s.path.clone()).collect();
    let mut body = prepared.body.clone();
    let prompt = place_refs(
        &prepared.model,
        &prepared.resolved,
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
    if let Some((ns, repo)) = ctx.prefs.hub_target() {
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
    let created_at = now_unix();
    let batch = format!("{created_at:x}-{}", &ids[0][..ids[0].len().min(8)]);
    let batch = match &ctx.batch_prefix {
        Some(prefix) => format!("{prefix}:{batch}"),
        None => batch,
    };
    let duration_secs = if req.kind == MediaKind::Video {
        crate::catalog::duration_secs(&prepared.model, &prepared.params)
    } else {
        None
    };
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
            prompt: prompt.clone(),
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
        })
        .collect();
    for item in &items {
        if let Err(e) = ctx.library.record(item.clone()) {
            tracing::warn!("could not record generation {}: {e}", item.id);
            ctx.library.upsert(item.clone());
        }
    }
    ctx.library.clear_uploads(&ctx.session, false);

    let slug = prepared.slug.clone();
    let notes = prepared.notes.clone();
    let model_id = prepared.model.id.clone();
    if !req.wait {
        if let Some(asides) = ctx.asides.clone() {
            let ctx2 = ctx.clone();
            let queue2 = queue.clone();
            let items2 = items.clone();
            let kind = req.kind;
            let slug2 = slug.clone();
            let feed2 = feed.clone();
            tokio::spawn(async move {
                let outcomes = wait_all(&ctx2, &queue2, items2, &slug2, feed2).await;
                commit_outputs(&ctx2, &outcomes, &slug2).await;
                let body = report(
                    kind,
                    &model_id,
                    &outcomes,
                    started.elapsed(),
                    &[],
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
                "Queued {count} {} generation(s) with {} (ids {}{est}). The result will be \
                 delivered to you automatically when it finishes — keep working on other \
                 things and do not poll. media_status shows progress; the user sees the \
                 job in their media feed.",
                req.kind,
                prepared.model.id,
                ids.join(", ")
            );
            for n in &notes {
                msg.push_str(&format!("\nNote: {n}"));
            }
            return Ok(msg);
        }
    }

    let outcomes = wait_all(ctx, &queue, items, &slug, feed).await;
    commit_outputs(ctx, &outcomes, &slug).await;
    Ok(report(
        req.kind,
        &prepared.model.id,
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
    let default_model = match req.kind {
        MediaKind::Image => &ctx.prefs.default_image_model,
        MediaKind::Video => &ctx.prefs.default_video_model,
    };
    let model_id = req
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(default_model);
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
    let resolved = refs::resolve(&ref_names, &ctx.refs, &ctx.root)?;

    let mut body = Map::new();
    let mut params = Map::new();
    let mut notes = Vec::new();

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
    // Extra params: must exist in the schema; reference slots go via `refs`.
    let ref_slots: Vec<String> = [RefKind::Image, RefKind::Video, RefKind::Audio]
        .into_iter()
        .flat_map(|k| model.slots(k))
        .map(|s| s.name)
        .collect();
    for (name, value) in &req.extra {
        if name == "prompt" || name == "model" || name == "num_generations" {
            continue;
        }
        if ref_slots.contains(name) {
            return Err(ToolError::InvalidArguments(format!(
                "`extra.{name}` takes reference media — pass files through `refs` instead \
                 (labels like [Image #1] or project paths) and they land there"
            )));
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
    validate_slots(&model, &resolved, catalog)?;
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
        notes,
        per_output,
        slug,
    })
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
        match uploader.upload(&r.path, r.kind, on_progress).await {
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

/// Put uploaded references into the model's fields and rewrite `[Image #N]`
/// mentions to the model's own convention. Slots were validated in
/// [`validate_slots`]; a model that can't take a kind never reaches here.
fn place_refs(
    model: &MediaModel,
    resolved: &[ResolvedRef],
    urls: &std::collections::HashMap<PathBuf, String>,
    prompt: &str,
    body: &mut Map<String, Value>,
) -> Result<String, ToolError> {
    let mut prompt = prompt.to_string();
    let at_style = model.prompt_uses_at_refs();
    for kind in [RefKind::Image, RefKind::Video, RefKind::Audio] {
        let of_kind: Vec<&ResolvedRef> = resolved.iter().filter(|r| r.kind == kind).collect();
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
        let mut uris = Vec::new();
        for r in &of_kind {
            let url = urls
                .get(&r.path)
                .cloned()
                .ok_or_else(|| ToolError::Execution(format!("{} was not uploaded", r.filename)))?;
            uris.push(url);
        }
        if primary.array {
            body.insert(
                primary.name.clone(),
                Value::Array(uris.into_iter().map(Value::String).collect()),
            );
        } else {
            let mut it = uris.into_iter();
            if let Some(first) = it.next() {
                body.insert(primary.name.clone(), Value::String(first));
            }
            if let (Some(second), Some(tail)) = (it.next(), slots.get(1)) {
                body.insert(tail.name.clone(), Value::String(second));
            }
        }
        // "[Image #2]" in the prompt → "@Image1" (its position among the
        // images sent) or "reference image 1".
        for (i, r) in of_kind.iter().enumerate() {
            if let Some(label) = &r.label {
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
    let ty = prop.get("type").and_then(Value::as_str).unwrap_or("");
    let value = match (ty, &value) {
        ("integer", Value::Number(n)) => Value::from(n.as_f64().unwrap_or(0.0).round() as i64),
        ("integer", Value::String(s)) if s.trim().parse::<f64>().is_ok() => {
            Value::from(s.trim().parse::<f64>().unwrap_or(0.0).round() as i64)
        }
        ("number", Value::String(s)) if s.trim().parse::<f64>().is_ok() => {
            Value::from(s.trim().parse::<f64>().unwrap_or(0.0))
        }
        ("string", Value::Number(n)) => Value::String(n.to_string()),
        ("boolean", Value::String(s)) => Value::Bool(matches!(s.trim(), "true" | "yes" | "on")),
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
    let finish = |item: &mut MediaItem, status: MediaStatus, error: Option<String>| {
        item.status = status;
        item.error = error;
        item.completed_at = Some(now_unix());
        if let Err(e) = lib.record(item.clone()) {
            tracing::warn!("could not record generation {}: {e}", item.id);
            lib.upsert(item.clone());
        }
    };
    let record = match result {
        Ok(record) => record,
        Err(QueueError::Timeout {
            elapsed, status, ..
        }) => {
            let msg = format!("still {status} after {elapsed}s; cancelled");
            finish(&mut item, MediaStatus::TimedOut, Some(msg.clone()));
            return Outcome {
                item,
                status: MediaStatus::TimedOut,
                rel: None,
                poster: None,
                error: Some(msg),
            };
        }
        Err(e) => {
            let msg = e.to_string();
            finish(&mut item, MediaStatus::Failed, Some(msg.clone()));
            return Outcome {
                item,
                status: MediaStatus::Failed,
                rel: None,
                poster: None,
                error: Some(msg),
            };
        }
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
        finish(&mut item, status, Some(msg.clone()));
        return Outcome {
            item,
            status,
            rel: None,
            poster: None,
            error: Some(msg),
        };
    }
    let Some(url) = record.result_url.clone() else {
        let msg = "succeeded but the hub returned no result URL".to_string();
        finish(&mut item, MediaStatus::Failed, Some(msg.clone()));
        return Outcome {
            item,
            status: MediaStatus::Failed,
            rel: None,
            poster: None,
            error: Some(msg),
        };
    };
    let bytes = match queue.download(&url).await {
        Ok(b) => b,
        Err(e) => {
            let msg = format!("download failed: {e}");
            finish(&mut item, MediaStatus::Failed, Some(msg.clone()));
            return Outcome {
                item,
                status: MediaStatus::Failed,
                rel: None,
                poster: None,
                error: Some(msg),
            };
        }
    };
    let ext = output_extension(kind, &url, &bytes, &item.params);
    let rel = match lib.save_output(&bytes, slug, item.index, &ext, item.created_at) {
        Ok(rel) => rel,
        Err(e) => {
            let msg = format!("could not save the result: {e}");
            finish(&mut item, MediaStatus::Failed, Some(msg.clone()));
            return Outcome {
                item,
                status: MediaStatus::Failed,
                rel: None,
                poster: None,
                error: Some(msg),
            };
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
    finish(&mut item, MediaStatus::Succeeded, None);
    Outcome {
        item,
        status: MediaStatus::Succeeded,
        rel: Some(rel),
        poster,
        error: None,
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
            let Some(queue) = self.ctx.queue() else {
                return Ok(no_key_result());
            };
            queue.cancel(id).await.map_err(queue_error)?;
            if let Some(mut item) = self
                .ctx
                .library
                .in_flight(None)
                .into_iter()
                .find(|i| i.id == id)
            {
                item.status = MediaStatus::Cancelled;
                item.completed_at = Some(now_unix());
                let _ = self.ctx.library.record(item);
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
                "resolution": {"type": "string", "enum": ["720p", "1080p"], "default": "720p"}}}}
        ]}))
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
        assert!(err.to_string().contains("through `refs`"), "{err}");
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
        let prompt = place_refs(&p.model, &p.resolved, &urls, &req.prompt, &mut body).unwrap();
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
        let prompt = place_refs(&p.model, &p.resolved, &urls, &req.prompt, &mut body).unwrap();
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
