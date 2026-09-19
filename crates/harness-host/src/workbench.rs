//! Session-scoped work views and Oxen workflow execution, shared by transports.

use crate::{EventSink, SessionService};
use async_trait::async_trait;
use harness_runtime::{
    documents::{DocumentSnapshot, Documents},
    workflow::{Workflow, WorkflowNode},
    workflow_run::{self, Executor, Output, Run},
};
use harness_tools::{
    views::{OpenViewArgs, RunWorkflowArgs, ViewHost},
    ToolError,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub struct Workbench {
    pub docs: Documents,
    pub session: String,
    pub sink: Arc<dyn EventSink>,
    pub media: Arc<harness_media::MediaContext>,
    pub gate: Arc<harness_permissions::PermissionGate>,
    pub client: Result<harness_llm::OxenClient, String>,
    pub store: Arc<harness_store::HistoryStore>,
    pub display: Mutex<Value>,
    pub runs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub failures: Mutex<HashMap<String, String>>,
}

pub struct WorkbenchView(pub Arc<Workbench>);
impl std::ops::Deref for WorkbenchView {
    type Target = Arc<Workbench>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn error(e: impl std::fmt::Display) -> ToolError {
    ToolError::Execution(e.to_string())
}

impl Workbench {
    async fn allowed(&self) -> Result<(), String> {
        use harness_permissions::{GateOutcome, GateReview, ToolEffect};
        match self
            .gate
            .review("run_workflow", &json!({}), ToolEffect::Mutating)
        {
            GateReview::Allow => Ok(()),
            GateReview::Deny { message } => Err(message),
            GateReview::Ask(request) => match self.gate.resolve(*request).await.0 {
                GateOutcome::Allow => Ok(()),
                GateOutcome::Deny { message } => Err(message),
                GateOutcome::AllowRewritten { .. } => {
                    Err("workflow execution cannot be rewritten".into())
                }
            },
        }
    }

    pub async fn start(
        self: &Arc<Self>,
        path: &str,
        revision: Option<&str>,
    ) -> Result<Run, String> {
        self.allowed().await?;
        let document = self.docs.read(path).map_err(|e| e.to_string())?;
        if revision.is_some_and(|r| r != document.revision) {
            return Err("graph changed on disk; reload before running".into());
        }
        // One run per conversation prevents accidental double-click billing.
        if !self.runs.lock().map_err(|e| e.to_string())?.is_empty() {
            return Err("this conversation already has a running workflow".into());
        }
        let reservation = Arc::new(AtomicBool::new(false));
        {
            let mut runs = self.runs.lock().map_err(|e| e.to_string())?;
            if !runs.is_empty() {
                return Err("this conversation already has a running workflow".into());
            }
            runs.insert("validating".into(), reservation.clone());
        }
        let created = workflow_run::create(&self.docs, document, self.as_ref()).await;
        let run = {
            let mut runs = self.runs.lock().map_err(|e| e.to_string())?;
            runs.remove("validating");
            let run = created?;
            runs.insert(run.id.clone(), reservation.clone());
            run
        };
        let latest_path = latest_path(path);
        let previous = match self.docs.read(&latest_path) {
            Ok(doc) => Some(doc),
            Err(harness_runtime::documents::DocumentError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(e) => {
                self.runs.lock().map_err(|e| e.to_string())?.remove(&run.id);
                return Err(e.to_string());
            }
        };
        if let Err(e) = self
            .docs
            .save(
                &latest_path,
                &json!({"id":run.id}).to_string(),
                previous.as_ref().map(|d| d.revision.as_str()),
            )
            .await
        {
            self.runs.lock().map_err(|e| e.to_string())?.remove(&run.id);
            return Err(e.to_string());
        }
        let engine = self.clone();
        let pending = run.clone();
        tokio::spawn(async move {
            let id = pending.id.clone();
            let outcome =
                workflow_run::execute(engine.docs.clone(), pending, engine.clone(), reservation)
                    .await;
            if let Err(reason) = outcome {
                match engine.failures.lock() {
                    Ok(mut failures) => {
                        failures.insert(id.clone(), reason);
                    }
                    Err(e) => eprintln!("record workflow failure {id}: {e}"),
                }
            }
            match engine.runs.lock() {
                Ok(mut runs) => {
                    runs.remove(&id);
                }
                Err(e) => eprintln!("finish workflow {id}: {e}"),
            };
        });
        Ok(run)
    }

    pub fn status(&self, id: &str) -> Result<Run, String> {
        let document = self
            .docs
            .read(&workflow_run::run_path(id)?)
            .map_err(|e| e.to_string())?;
        let mut run: Run = serde_json::from_str(&document.content).map_err(|e| e.to_string())?;
        if let Some(reason) = self.failures.lock().map_err(|e| e.to_string())?.get(id) {
            run.status = "failed".into();
            run.error = Some(reason.clone());
        } else if matches!(run.status.as_str(), "queued" | "running")
            && !self
                .runs
                .lock()
                .map_err(|e| e.to_string())?
                .contains_key(id)
        {
            run.status = "interrupted".into();
            run.error = Some("Host restarted during this run. Check the media library for completed jobs before starting another run; jobs may have been billed.".into());
        }
        Ok(run)
    }

    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let runs = self.runs.lock().map_err(|e| e.to_string())?;
        let flag = runs.get(id).ok_or("workflow is not running")?;
        flag.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn model(&self, node: &WorkflowNode) -> String {
        match node.text("model") {
            "" => match node.kind.as_str() {
                "upscale" => "flux-image-upscaler".into(),
                "video_upscale" => "flux-video-upscaler".into(),
                "rewrite" => harness_core::DEFAULT_MODEL.into(),
                "video" => self.media.prefs.default_video_model.clone(),
                _ => self.media.prefs.default_image_model.clone(),
            },
            model => model.into(),
        }
    }
}

#[async_trait]
impl Executor for Workbench {
    async fn validate(&self, graph: &Workflow) -> Result<(), String> {
        for node in &graph.nodes {
            match node.kind.as_str() {
                "image" | "video" | "upscale" | "video_upscale" => {
                    let catalog = self.media.catalog().await.map_err(|e| e.to_string())?;
                    let model_id = self.model(node);
                    let model = catalog
                        .get(&model_id)
                        .ok_or_else(|| format!("{}: unknown Oxen model {model_id}", node.id))?;
                    let video = matches!(node.kind.as_str(), "video" | "video_upscale");
                    if (model.kind == harness_media::MediaKind::Video) != video {
                        return Err(format!(
                            "{}: model {model_id} has the wrong media type",
                            node.id
                        ));
                    }
                    harness_runtime::workflow::validate_parameters(
                        &workflow_run::parameters(node),
                        &model.request_schema,
                    )
                    .map_err(|e| format!("{}: {e}", node.id))?;
                    for edge in graph.edges.iter().filter(|e| e.target == node.id) {
                        let kind = match edge.target_port.as_str() {
                            "image" => Some(harness_media::RefKind::Image),
                            "video" => Some(harness_media::RefKind::Video),
                            _ => None,
                        };
                        if let Some(kind) = kind {
                            if !model.accepts(kind) {
                                return Err(format!(
                                    "{}: {model_id} does not accept {} references",
                                    node.id, edge.target_port
                                ));
                            }
                        }
                    }
                    if self
                        .media
                        .api
                        .as_ref()
                        .is_none_or(|api| api.api_key.is_empty())
                    {
                        return Err(harness_media::MEDIA_NO_KEY.into());
                    }
                }
                "image_input" | "video_input" => {
                    let path = self
                        .docs
                        .resolve(node.text("path"))
                        .map_err(|e| e.to_string())?;
                    if !path.is_file() {
                        return Err(format!(
                            "{}: reference {} is not a file",
                            node.id,
                            node.text("path")
                        ));
                    }
                }
                "rewrite" => {
                    self.client.as_ref().map_err(Clone::clone)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn execute(
        &self,
        run: &str,
        node: &WorkflowNode,
        inputs: BTreeMap<String, Vec<Output>>,
    ) -> Result<Vec<Output>, String> {
        self.allowed().await?;
        let output = |kind: &str, value: String| {
            vec![Output {
                kind: kind.into(),
                value,
                job: None,
            }]
        };
        let prompt = inputs
            .get("prompt")
            .and_then(|v| v.first())
            .map(|v| v.value.as_str())
            .unwrap_or(node.text("prompt"));
        match node.kind.as_str() {
            "prompt" => Ok(output("text", node.text("text").into())),
            "image_input" | "video_input" => {
                self.docs
                    .resolve(node.text("path"))
                    .map_err(|e| e.to_string())?;
                Ok(output(
                    if node.kind == "image_input" {
                        "image"
                    } else {
                        "video"
                    },
                    node.text("path").into(),
                ))
            }
            "output" => Ok(inputs.into_values().flatten().collect()),
            "rewrite" => {
                let instructions = match node.text("instructions") { "" => "Rewrite this as a clear, vivid generation prompt. Return only the rewritten prompt.", text => text };
                let model = self.model(node);
                let request = harness_llm::ChatRequest::new(
                    &model,
                    vec![
                        harness_llm::ChatMessage::system(instructions),
                        harness_llm::ChatMessage::user(prompt),
                    ],
                )
                .max_tokens(2048);
                let reply = self
                    .client
                    .as_ref()
                    .map_err(Clone::clone)?
                    .chat(&request)
                    .await
                    .map_err(|e| format!("rewrite {}: {e}", node.id))?;
                if let Some(usage) = &reply.usage {
                    self.store
                        .record_model_usage_detailed(
                            &model,
                            "workflow",
                            usage.prompt_tokens as usize,
                            usage.completion_tokens as usize,
                            &harness_store::UsageDetail {
                                session_id: &self.session,
                                kind: "workflow",
                                ..Default::default()
                            },
                        )
                        .map_err(|e| format!("record billed rewrite usage: {e}"))?;
                }
                let text = reply
                    .message()
                    .and_then(|m| m.content_text())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("rewrite returned no text")?;
                Ok(output("text", text))
            }
            "image" | "video" | "upscale" | "video_upscale" => {
                let kind = if matches!(node.kind.as_str(), "video" | "video_upscale") {
                    harness_media::MediaKind::Video
                } else {
                    harness_media::MediaKind::Image
                };
                let references: Vec<String> = inputs
                    .values()
                    .flatten()
                    .filter(|o| o.kind != "text")
                    .map(|o| o.value.clone())
                    .collect();
                for path in &references {
                    self.docs.resolve(path).map_err(|e| e.to_string())?;
                }
                let media = Arc::new(
                    harness_media::MediaContext::new(
                        &self.session,
                        self.docs.root(),
                        self.media.prefs.clone(),
                        self.media.api.clone(),
                        self.media.refs.clone(),
                        self.media.library.clone(),
                        self.media.sink.clone(),
                    )
                    .with_batch_prefix(format!("wf-{run}-{}", node.id)),
                );
                let params = workflow_run::parameters(node)
                    .as_object()
                    .cloned()
                    .ok_or("model parameters must be an object")?;
                let prompt = if prompt.is_empty() {
                    "Enhance the supplied media."
                } else {
                    prompt
                };
                let items = media
                    .generate_workflow(kind, prompt.into(), self.model(node), references, params)
                    .await
                    .map_err(|e| e.to_string())?;
                items
                    .into_iter()
                    .map(|item| {
                        let path = item.path.ok_or_else(|| {
                            item.error.unwrap_or_else(|| {
                                format!("generation {} ended without a downloaded output", item.id)
                            })
                        })?;
                        Ok(Output {
                            kind: kind.to_string(),
                            value: path,
                            job: Some(item.id),
                        })
                    })
                    .collect()
            }
            other => Err(format!("unsupported workflow node {other}")),
        }
    }
}

#[async_trait]
impl ViewHost for WorkbenchView {
    async fn list(&self) -> Result<Value, ToolError> {
        Ok(
            json!({"views":harness_runtime::views::available().map_err(error)?, "workflow_schema":schemars_schema(), "nodes":harness_runtime::workflow::node_definitions()}),
        )
    }
    async fn open(&self, args: OpenViewArgs) -> Result<Value, ToolError> {
        let inferred = match &args.path {
            Some(path) => harness_runtime::views::resolve_view(path).map_err(error)?,
            None => "gallery".into(),
        };
        let view = args.view.as_deref().unwrap_or(&inferred);
        let definition = harness_runtime::views::available()
            .map_err(error)?
            .into_iter()
            .find(|d| d.id == view)
            .ok_or_else(|| error(format!("unknown view {view}")))?;
        if definition.requires_file && args.path.is_none() {
            return Err(error(format!("{view} requires a file path")));
        }
        if let Some(path) = &args.path {
            let resolved = self.docs.resolve(path).map_err(error)?;
            if !resolved.is_file() {
                return Err(error(format!("view file does not exist: {path}")));
            }
        }
        *self.display.lock().map_err(error)? =
            json!({"view":view,"path":args.path,"status":"requested"});
        self.sink.emit(harness_protocol::ProtocolEvent::ViewOpen {
            session: self.session.clone(),
            view: view.into(),
            path: args.path,
        });
        Ok(json!({"status":"requested","view":view}))
    }
    async fn inspect(&self) -> Result<Value, ToolError> {
        let mut value = self.display.lock().map_err(error)?.clone();
        if let Some(path) = value.get("path").and_then(Value::as_str).map(str::to_owned) {
            let doc = self.docs.read(&path).map_err(error)?;
            value["revision"] = json!(doc.revision);
            if path.ends_with(".graph.json") {
                value["diagnostics"] = match Workflow::parse(&doc.content) {
                    Ok(graph) => json!(graph.diagnostics(true)),
                    Err(e) => json!([{ "message":e }]),
                };
            }
        }
        Ok(value)
    }
    async fn run_workflow(&self, args: RunWorkflowArgs) -> Result<Value, ToolError> {
        let run = self
            .start(&args.path, args.revision.as_deref())
            .await
            .map_err(error)?;
        Ok(json!({"run":run,"record_path":workflow_run::run_path(&run.id).map_err(error)?}))
    }
}

fn schemars_schema() -> Value {
    harness_runtime::workflow::schema()
}

fn latest_path(path: &str) -> String {
    format!(
        ".oxen-harness/workflow-runs/{}.latest.json",
        harness_runtime::documents::revision_of(path.as_bytes())
    )
}

impl SessionService {
    pub async fn workbench_request(
        &self,
        session: &str,
        action: &str,
        payload: Value,
    ) -> Result<Value, String> {
        let string = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("missing {key}"))
        };
        match action {
            "read" => serde_json::to_value(self.read_document(session, string("path")?)?)
                .map_err(|e| e.to_string()),
            "save" => serde_json::to_value(
                self.save_document(
                    session,
                    string("path")?,
                    string("content")?,
                    payload.get("revision").and_then(Value::as_str),
                )
                .await?,
            )
            .map_err(|e| e.to_string()),
            _ => {
                let engine = self.workbench(session).await?;
                match action {
                    "models" => Ok(json!(engine
                        .media
                        .catalog()
                        .await
                        .map_err(|e| e.to_string())?
                        .models())),
                    "list" => WorkbenchView(engine)
                        .list()
                        .await
                        .map_err(|e| e.to_string()),
                    "open" => WorkbenchView(engine)
                        .open(serde_json::from_value(payload).map_err(|e| e.to_string())?)
                        .await
                        .map_err(|e| e.to_string()),
                    "inspect" => WorkbenchView(engine)
                        .inspect()
                        .await
                        .map_err(|e| e.to_string()),
                    "add_context" => {
                        let text = string("text")?;
                        if text.len() > 32768 {
                            return Err("view context is too large".into());
                        }
                        self.sink
                            .emit(harness_protocol::ProtocolEvent::ViewContext {
                                session: session.into(),
                                path: payload
                                    .get("path")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                text: text.into(),
                            });
                        Ok(Value::Null)
                    }
                    "report" => {
                        if payload.to_string().len() > 32_768 || !payload.is_object() {
                            return Err("invalid view report".into());
                        }
                        *engine.display.lock().map_err(|e| e.to_string())? = payload;
                        Ok(json!(null))
                    }
                    "run" => Ok(json!(
                        engine
                            .start(
                                string("path")?,
                                payload.get("revision").and_then(Value::as_str)
                            )
                            .await?
                    )),
                    "status" => Ok(json!(engine.status(string("id")?)?)),
                    "latest" => {
                        match engine.docs.read(&latest_path(string("path")?)) {
                            Ok(document) => {
                                let value: Value = serde_json::from_str(&document.content)
                                    .map_err(|e| e.to_string())?;
                                Ok(json!(engine.status(
                                    value["id"].as_str().ok_or("invalid latest run record")?
                                )?))
                            }
                            Err(harness_runtime::documents::DocumentError::Io {
                                source, ..
                            }) if source.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
                            Err(e) => Err(e.to_string()),
                        }
                    }
                    "cancel" => {
                        engine.cancel(string("id")?)?;
                        Ok(json!(null))
                    }
                    _ => Err(format!("unknown workbench action {action}")),
                }
            }
        }
    }

    pub async fn workbench(&self, session: &str) -> Result<Arc<Workbench>, String> {
        self.store()?
            .session_meta(session)
            .map_err(|e| format!("unknown work context {session}: {e}"))?;
        if let Some(engine) = self
            .workbenches
            .lock()
            .map_err(|e| e.to_string())?
            .get(session)
            .cloned()
        {
            return Ok(engine);
        }
        self.agent_or_build(session).await?;
        self.workbenches
            .lock()
            .map_err(|e| e.to_string())?
            .get(session)
            .cloned()
            .ok_or_else(|| format!("work context unavailable: {session}"))
    }
    pub fn documents(&self, session: &str) -> Result<Documents, String> {
        let meta = self
            .store()?
            .session_meta(session)
            .map_err(|e| format!("unknown work context {session}: {e}"))?;
        Documents::new(meta.workspace).map_err(|e| e.to_string())
    }
    pub fn read_document(&self, session: &str, path: &str) -> Result<DocumentSnapshot, String> {
        self.documents(session)?
            .read(path)
            .map_err(|e| e.to_string())
    }
    pub async fn save_document(
        &self,
        session: &str,
        path: &str,
        content: &str,
        revision: Option<&str>,
    ) -> Result<DocumentSnapshot, String> {
        self.documents(session)?
            .save(path, content, revision)
            .await
            .map_err(|e| e.to_string())
    }
}
