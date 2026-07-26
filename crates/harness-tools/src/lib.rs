//! Built-in agent tools for oxen-harness.
//!
//! This crate defines the [`TypedTool`] trait every capability implements (see
//! its docs for the add-a-tool recipe, or "Adding a tool" in the repo README),
//! a [`ToolRegistry`] for dispatching model tool calls by name, and the
//! concrete tools the agent uses: file read/write/edit, glob file discovery,
//! and regex content search ([`fs`]), sandboxed shell execution ([`shell`]),
//! git operations ([`git`]), Brave-backed web search ([`web`]), the task
//! checklist ([`plan`]), asking the user structured multiple-choice questions
//! ([`ask`]), side-panel documents ([`canvas`]), and opening project files in
//! the user's viewer ([`viewer`]). All file access is confined to a
//! [`sandbox::Workspace`].
//!
//! The lower-level [`Tool`] trait (raw JSON in, string out) exists for tools
//! whose schema is only known at runtime — user-defined [`CustomToolSpec`]
//! tools are the one case. New built-in tools should implement [`TypedTool`].
//!
//! **Host-surface tools** — tools whose whole effect is showing something in
//! the host's UI ([`ask`], [`canvas`], [`viewer`]) — follow one pattern: a
//! plain data struct describing what to show, a `…Sink` trait each front end
//! implements, and a [`TypedTool`] that validates arguments and forwards to
//! the sink. They register per host (never in the default registry): a host
//! that lacks the surface either degrades inside its sink (the CLI writes
//! canvas docs to disk) or doesn't register the tool at all, so the model is
//! never promised a panel that can't appear. [`viewer`] is the documented
//! reference implementation.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod ask;
pub mod canvas;
pub mod fs;
pub mod gh;
pub mod git;
mod http_body;
pub mod plan;
pub mod trail;
pub mod process;
pub mod retrieve;
pub mod sandbox;
pub mod shell;
pub mod skill;
pub mod tasks;
pub mod viewer;
pub mod web;
pub mod web_fetch;

pub use ask::{AskUserTool, Choice, Question, QuestionAnswer, QuestionAsker, ASK_USER_TOOL};
pub use canvas::{CanvasDoc, CanvasSink, CanvasTool, CANVAS_FORMATS, CANVAS_TOOL};
pub use fs::{
    FileState, Freshness, PathRule, EDIT_FILE_TOOL, FIND_FILES_TOOL, READ_FILE_TOOL,
    SEARCH_FILES_TOOL, WRITE_FILE_TOOL,
};
pub use gh::{GhTool, GH_TOOL};
pub use git::GIT_TOOL;
pub use plan::{
    parse_plan_arguments, plan_is_open, plan_snapshot, PlanItem, PlanSnapshot, PlanStatus,
    PlanTool, PLAN_TOOL,
};
pub use retrieve::{RetrieveOriginalTool, RETRIEVE_ORIGINAL_TOOL};
pub use trail::{
    merge_trail, parse_trail_arguments, TrailSnapshot, TrailTool, Waypoint, WaypointStatus,
    TRAIL_TOOL,
};
pub use sandbox::Workspace;
pub use shell::RUN_SHELL_TOOL;
pub use skill::{Skill, SkillScope, SkillTool, SKILL_TOOL};
pub use viewer::{FileView, OpenFileTool, ViewerSink, OPEN_FILE_TOOL};
pub use web::WEB_SEARCH_TOOL;
pub use web_fetch::{WebFetchTool, WEB_FETCH_TOOL};

/// Errors a tool can return while running.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("execution failed: {0}")]
    Execution(String),
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A capability the agent can invoke during the loop — the raw, dyn-dispatched
/// form the registry stores.
///
/// The `parameters_schema` is a JSON Schema object describing `invoke`'s
/// arguments; it is sent to the model as part of the OpenAI-compatible tool
/// definition so the model knows how to call it.
///
/// Prefer implementing [`TypedTool`] instead: it derives the schema from a
/// typed args struct so the two can't drift. Implement `Tool` directly only
/// when the schema isn't known at compile time (e.g. user-defined tools).
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable identifier the model uses to call this tool.
    fn name(&self) -> &str;

    /// Human-readable description shown to the model.
    fn description(&self) -> &str;

    /// JSON Schema describing the arguments accepted by [`Tool::invoke`].
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with model-provided arguments, returning a string
    /// result that is appended to the transcript as a `tool` message.
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError>;
}

/// The preferred way to write a built-in tool: arguments are a typed struct,
/// and the JSON Schema the model sees is derived from that same struct — so the
/// advertised interface and what `run` actually parses can never drift.
///
/// Implementing a tool takes three pieces:
///
/// 1. An args struct deriving `Deserialize` + `schemars::JsonSchema`. Doc
///    comments on the struct's fields become the model-facing descriptions —
///    write them for the model, not for rustdoc.
/// 2. An `impl TypedTool` with a `NAME` constant, a description telling the
///    model *when* to reach for the tool, and the `run` body.
/// 3. A registration call: `registry.with_typed(MyTool::new(...))` (see
///    [`ToolRegistry::default_for_workspace_with_web_key`] for the built-in set).
///
/// ```
/// use harness_tools::{ToolError, ToolRegistry, TypedTool};
///
/// /// What `echo` accepts. Field doc comments are shown to the model.
/// #[derive(serde::Deserialize, schemars::JsonSchema)]
/// struct EchoArgs {
///     /// The text to echo back.
///     text: String,
///     /// Repeat the text this many times (default 1).
///     times: Option<usize>,
/// }
///
/// struct EchoTool;
///
/// #[async_trait::async_trait]
/// impl TypedTool for EchoTool {
///     const NAME: &'static str = "echo";
///     type Args = EchoArgs;
///
///     fn description(&self) -> &str {
///         "Echo the provided text back, optionally repeated."
///     }
///
///     async fn run(&self, args: EchoArgs) -> Result<String, ToolError> {
///         Ok(args.text.repeat(args.times.unwrap_or(1).max(1)))
///     }
/// }
///
/// let registry = ToolRegistry::new().with_typed(EchoTool);
/// assert!(registry.get("echo").is_some());
/// ```
///
/// Optional fields are `Option<T>` (or `#[serde(default)]`) and are omitted from
/// the schema's `required` list automatically; enums derive to JSON `enum`
/// values, so invalid choices are rejected before `run` is called.
#[async_trait]
pub trait TypedTool: Send + Sync {
    /// Stable identifier the model uses to call this tool.
    const NAME: &'static str;

    /// The tool's arguments. Deriving `Deserialize` + `JsonSchema` keeps the
    /// advertised schema and the parsed arguments in lockstep.
    type Args: serde::de::DeserializeOwned + schemars::JsonSchema + Send;

    /// Human-readable description shown to the model: what the tool does and
    /// when to use it.
    fn description(&self) -> &str;

    /// Execute the tool with already-validated, typed arguments.
    async fn run(&self, args: Self::Args) -> Result<String, ToolError>;

    /// Parse raw JSON arguments and run — exactly what the registry does when
    /// the model calls the tool. Provided; useful in tests and for hosts that
    /// hold a concrete tool.
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError>
    where
        Self: Sized,
    {
        let parsed: Self::Args =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        self.run(parsed).await
    }
}

/// Derive the model-facing JSON Schema for a typed argument struct: subschemas
/// are inlined (no `$ref` indirection, which some providers reject) and
/// rustdoc-oriented noise (`$schema`, `title`) is stripped to keep the
/// per-request token overhead down.
pub fn schema_for<T: schemars::JsonSchema>() -> serde_json::Value {
    let generator = schemars::generate::SchemaSettings::draft07()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let mut schema = generator.into_root_schema_for::<T>().to_value();
    strip_meta(&mut schema);
    // A struct-level doc comment would land here and just repeat the tool's
    // description; the model already gets that one level up.
    if let Some(map) = schema.as_object_mut() {
        map.remove("description");
    }
    schema
}

/// Compact the derived schema: drop keys that describe the schema rather than
/// the arguments (`$schema`, `title`, integer width `format`s), and fold
/// documented enums (`oneOf` of `const`s) into a plain `enum` list with the
/// variant docs merged into the field description. The tool block is resent on
/// every model call, so each key here is per-request overhead.
fn strip_meta(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("$schema");
            map.remove("title");
            // Integer width hints ("uint", "int32", …) and the matching
            // `minimum: 0` say nothing a model needs.
            if map
                .get("format")
                .and_then(|f| f.as_str())
                .is_some_and(|f| f.contains("int"))
            {
                map.remove("format");
                if map.get("minimum").and_then(serde_json::Value::as_u64) == Some(0) {
                    map.remove("minimum");
                }
            }
            // `Option<T>` derives `"type": ["T", "null"]`; optionality is
            // already conveyed by absence from `required`.
            if let Some(types) = map.get("type").and_then(|t| t.as_array()) {
                if types.len() == 2 && types.contains(&serde_json::Value::from("null")) {
                    let other = types.iter().find(|t| *t != "null").cloned();
                    if let Some(other) = other {
                        map.insert("type".into(), other);
                    }
                }
            }
            if let Some((values, docs)) = documented_enum(map.get("oneOf")) {
                map.remove("oneOf");
                map.insert("type".into(), "string".into());
                map.insert("enum".into(), values.into());
                if !docs.is_empty() {
                    let mut description = map
                        .get("description")
                        .and_then(|d| d.as_str())
                        .map(str::to_string)
                        .unwrap_or_default();
                    if !description.is_empty() {
                        description.push(' ');
                    }
                    description.push_str(&docs);
                    map.insert("description".into(), description.into());
                }
            }
            for v in map.values_mut() {
                strip_meta(v);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                strip_meta(v);
            }
        }
        _ => {}
    }
}

/// If `one_of` is the shape schemars derives for a doc-commented string enum
/// (each entry `{const, type?, description?}`), return the plain value list and
/// a "value = doc; …" summary of the variant docs.
fn documented_enum(one_of: Option<&serde_json::Value>) -> Option<(Vec<serde_json::Value>, String)> {
    let entries = one_of?.as_array()?;
    let mut values = Vec::with_capacity(entries.len());
    let mut docs = Vec::new();
    for entry in entries {
        let obj = entry.as_object()?;
        let value = obj.get("const")?;
        if obj
            .keys()
            .any(|k| !["const", "type", "description"].contains(&k.as_str()))
        {
            return None;
        }
        if let Some(desc) = obj.get("description").and_then(|d| d.as_str()) {
            docs.push(format!(
                "{} = {}",
                value.as_str()?,
                desc.trim_end_matches('.')
            ));
        }
        values.push(value.clone());
    }
    Some((values, docs.join("; ")))
}

/// Bridges a [`TypedTool`] to the dyn-dispatched [`Tool`] the registry stores.
/// (A blanket `impl Tool for T: TypedTool` would conflict with hand-written
/// `Tool` impls under coherence rules, so the registry wraps instead.)
struct TypedAdapter<T>(T);

#[async_trait]
impl<T: TypedTool> Tool for TypedAdapter<T> {
    fn name(&self) -> &str {
        T::NAME
    }
    fn description(&self) -> &str {
        self.0.description()
    }
    fn parameters_schema(&self) -> serde_json::Value {
        schema_for::<T::Args>()
    }
    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        self.0.invoke(args).await
    }
}

/// Build the OpenAI-compatible tool definition for a tool.
///
/// Shape: `{ "type": "function", "function": { name, description, parameters } }`.
pub fn tool_definition(tool: &dyn Tool) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name(),
            "description": tool.description(),
            "parameters": tool.parameters_schema(),
        }
    })
}

/// Where a no-code custom tool sends its JSON arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CustomToolAction {
    /// POST the model-provided JSON arguments to an HTTP endpoint. The response
    /// body is returned to the model as the tool result.
    HttpPost { url: String },
}

/// A user-defined tool backed by a simple external action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub action: CustomToolAction,
}

struct CustomTool {
    spec: CustomToolSpec,
    client: reqwest::Client,
}

impl CustomTool {
    fn new(spec: CustomToolSpec) -> Self {
        Self {
            spec,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for CustomTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.spec.parameters.clone()
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        match &self.spec.action {
            CustomToolAction::HttpPost { url } => {
                let res = self
                    .client
                    .post(url)
                    .json(&args)
                    .send()
                    .await
                    .map_err(|e| ToolError::Execution(format!("HTTP request failed: {e}")))?;
                let status = res.status();
                const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
                let body = crate::http_body::text(res, MAX_RESPONSE_BYTES)
                    .await
                    .map_err(ToolError::Execution)?;
                if !status.is_success() {
                    return Err(ToolError::Execution(format!(
                        "HTTP {status}: {}",
                        body.trim()
                    )));
                }
                Ok(body)
            }
        }
    }
}

/// A set of tools, addressable by name, that the agent loop dispatches into.
///
/// Alongside the tools themselves, the registry holds optional per-tool
/// description overrides. When present, an override replaces the tool's built-in
/// description in the definitions advertised to the model (dispatch is
/// unaffected — only the prose the model reads changes). The host layers these in
/// from the user's saved tool preferences.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
    description_overrides: BTreeMap<String, String>,
    /// The file state the fs tools share, kept so a host can reach it after
    /// the fact — path-scoped project conventions are discovered a layer up
    /// (in `harness-runtime`, which depends on this crate) and installed here.
    files: Option<Arc<fs::FileState>>,
    /// The workspace the default registry was built for, so callers that hold
    /// only the registry (a resumed agent rehydrating read state) can resolve
    /// the workspace-relative paths recorded in a transcript.
    workspace: Option<Workspace>,
    /// Where output dropped by a tool's size cap is kept for
    /// `retrieve_original`. The agent reuses it for compression, so both kinds
    /// of `<<ccr:HASH>>` marker resolve through one store.
    overflow: Option<Arc<harness_compress::CcrStore>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The shared fs session state, when this registry was built with one.
    pub fn files(&self) -> Option<&Arc<fs::FileState>> {
        self.files.as_ref()
    }

    /// The workspace this registry was built for, when built for one.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    /// The store holding output that tool size caps dropped, when this
    /// registry was built with one.
    pub fn overflow_store(&self) -> Option<&Arc<harness_compress::CcrStore>> {
        self.overflow.as_ref()
    }

    /// Register a tool, returning the registry for chaining.
    pub fn with(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.insert(tool.name().to_string(), tool);
        self
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Register a [`TypedTool`], returning the registry for chaining.
    pub fn with_typed<T: TypedTool + 'static>(self, tool: T) -> Self {
        self.with(Arc::new(TypedAdapter(tool)))
    }

    /// Register a [`TypedTool`].
    pub fn register_typed<T: TypedTool + 'static>(&mut self, tool: T) {
        self.register(Arc::new(TypedAdapter(tool)));
    }

    /// Register a user-defined no-code tool backed by an external action.
    pub fn register_custom(&mut self, spec: CustomToolSpec) {
        self.register(Arc::new(CustomTool::new(spec)));
    }

    /// Unregister a tool by name (used to apply the user's disabled-tools
    /// preference). Returns the removed tool, if any.
    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn Tool>> {
        self.description_overrides.remove(name);
        self.tools.remove(name)
    }

    /// Replace the description advertised to the model for `name`. No-op for
    /// dispatch; only the model-facing definition changes.
    pub fn set_description_override(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
    ) {
        self.description_overrides
            .insert(name.into(), description.into());
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// The (name, default description, schema) of every registered tool, sorted
    /// by name — for a host enumerating tools in a settings UI. Reports each
    /// tool's *built-in* description, ignoring any override.
    pub fn specs(&self) -> Vec<(String, String, serde_json::Value)> {
        self.tools
            .values()
            .map(|t| {
                (
                    t.name().to_string(),
                    t.description().to_string(),
                    t.parameters_schema(),
                )
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// The OpenAI-compatible `tools` array to send with a chat request, with any
    /// per-tool description overrides layered in.
    pub fn definitions(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|t| match self.description_overrides.get(t.name()) {
                Some(desc) => serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": desc,
                        "parameters": t.parameters_schema(),
                    }
                }),
                None => tool_definition(t.as_ref()),
            })
            .collect()
    }

    /// Dispatch a model tool call to the matching tool.
    pub async fn invoke(&self, name: &str, args: serde_json::Value) -> Result<String, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?;
        tool.invoke(args).await
    }

    /// Construct the default tool set rooted at a workspace: fs read/write/edit,
    /// find (glob), search (grep), shell, git, and web search (which prompts for
    /// a Brave API key on first use if none is configured).
    pub fn default_for_workspace(workspace: Workspace) -> Self {
        Self::default_for_workspace_with_web_key(workspace, None)
    }

    /// Like [`Self::default_for_workspace`], but with an explicit Brave Search
    /// API key for web search (e.g. one configured in a UI). A `None`/blank key
    /// falls back to the `BRAVE_API_KEY` environment variable. Web search is
    /// *always* registered; if no key resolves, the call fails with the
    /// recognizable [`web::WEB_SEARCH_NO_KEY`] error the front ends turn into an
    /// inline "add your API key" prompt.
    pub fn default_for_workspace_with_web_key(
        workspace: Workspace,
        brave_key: Option<String>,
    ) -> Self {
        // One file state behind all three fs tools: reads record what the
        // model has seen, writes check it, and both take the same per-path
        // lock. Subagents share their parent's registry, so this is also what
        // keeps two fleet lanes off the same file at once.
        let files = fs::FileState::gated();
        let mut registry = Self::new()
            .with_typed(fs::ReadFileTool::with_state(
                workspace.clone(),
                files.clone(),
            ))
            .with_typed(fs::WriteFileTool::with_state(
                workspace.clone(),
                files.clone(),
            ))
            .with_typed(fs::EditFileTool::with_state(
                workspace.clone(),
                files.clone(),
            ))
            .with_typed(fs::FindFilesTool::new(workspace.clone()))
            .with_typed(fs::SearchTool::new(workspace.clone()))
            .with_typed(git::GitTool::new(workspace.clone()))
            // GitHub PR checks/creation — how the model verifies the trail's
            // shipping stages before marking them done.
            .with_typed(gh::GhTool::new(workspace.clone()))
            // Planning/checklist tool — always available so any host gets it.
            .with_typed(plan::PlanTool::new())
            // The session's macro journey (title + waypoints) for the Ledger.
            .with_typed(trail::TrailTool::new())
            // Fetch a web page into context (no key needed); pairs with the
            // web_search tool registered just below.
            .with_typed(web_fetch::WebFetchTool::new());

        // The shell + background-task trio share one registry, so the task
        // ids `run_shell` hands out resolve in `task_output`/`kill_task`.
        // The overflow store keeps what output caps drop, so a truncated build
        // log is one `retrieve_original` away instead of gone.
        let overflow = Arc::new(harness_compress::CcrStore::default());
        let tasks = tasks::BackgroundTasks::in_temp_with_overflow(Some(overflow.clone()));
        registry.register_typed(shell::ShellTool::with_tasks(workspace.clone(), tasks.clone()));
        registry.register_typed(tasks::TaskOutputTool::new(tasks.clone()));
        registry.register_typed(tasks::KillTaskTool::new(tasks));

        // Always register web search so the model can use it; when no Brave key
        // is configured the call fails with a recognizable error that the UIs
        // turn into an inline "add your API key" prompt.
        registry.register_typed(web::WebSearchTool::with_key(brave_key));
        // `retrieve_original` serves both truncation overflow (always) and
        // context compression (when it's on) out of the same store.
        registry.register_typed(retrieve::RetrieveOriginalTool::new(overflow.clone()));
        registry.files = Some(files);
        registry.workspace = Some(workspace);
        registry.overflow = Some(overflow);
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The minimal reference tool — mirrors the `TypedTool` doc example.
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct EchoArgs {
        /// The text to echo back.
        text: String,
    }

    struct EchoTool;

    #[async_trait]
    impl TypedTool for EchoTool {
        const NAME: &'static str = "echo";
        type Args = EchoArgs;

        fn description(&self) -> &str {
            "Echo the provided text back."
        }

        async fn run(&self, args: EchoArgs) -> Result<String, ToolError> {
            Ok(args.text)
        }
    }

    #[test]
    fn tool_definition_has_openai_function_shape() {
        let def = tool_definition(&TypedAdapter(EchoTool));
        assert_eq!(def["type"], "function");
        assert_eq!(def["function"]["name"], "echo");
        assert_eq!(def["function"]["parameters"]["type"], "object");
        // The derived schema carries the field's doc comment and requiredness.
        let params = &def["function"]["parameters"];
        assert_eq!(params["properties"]["text"]["type"], "string");
        assert_eq!(
            params["properties"]["text"]["description"],
            "The text to echo back."
        );
        assert_eq!(params["required"][0], "text");
    }

    #[tokio::test]
    async fn registry_dispatches_by_name() {
        let registry = ToolRegistry::new().with_typed(EchoTool);
        assert_eq!(registry.len(), 1);
        let out = registry
            .invoke("echo", serde_json::json!({"text": "moo"}))
            .await
            .unwrap();
        assert_eq!(out, "moo");
    }

    #[tokio::test]
    async fn typed_dispatch_rejects_bad_arguments() {
        let registry = ToolRegistry::new().with_typed(EchoTool);
        let err = registry
            .invoke("echo", serde_json::json!({"text": 42}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn registry_errors_on_unknown_tool() {
        let registry = ToolRegistry::new();
        let err = registry
            .invoke("nope", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::UnknownTool(_)));
    }

    #[test]
    fn default_registry_contains_every_shipped_tool() {
        // Writing a tool and forgetting to register it fails silently — the
        // tool just never exists. If you added a tool module, add its NAME
        // constant here AND register it in `default_for_workspace_with_web_key`
        // (or in the hosts, for UI-bridged tools like ask/canvas).
        let workspace = Workspace::new(".").unwrap();
        let registry = ToolRegistry::default_for_workspace(workspace);
        let mut names: Vec<String> = registry.specs().into_iter().map(|(n, ..)| n).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                fs::EDIT_FILE_TOOL,
                fs::FIND_FILES_TOOL,
                gh::GH_TOOL,
                git::GIT_TOOL,
                tasks::KILL_TASK_TOOL,
                fs::READ_FILE_TOOL,
                retrieve::RETRIEVE_ORIGINAL_TOOL,
                shell::RUN_SHELL_TOOL,
                fs::SEARCH_FILES_TOOL,
                tasks::TASK_OUTPUT_TOOL,
                plan::PLAN_TOOL,
                trail::TRAIL_TOOL,
                web_fetch::WEB_FETCH_TOOL,
                web::WEB_SEARCH_TOOL,
                fs::WRITE_FILE_TOOL,
            ],
            "default registry drifted from the shipped tool set"
        );
    }

    #[test]
    fn default_tool_definitions_stay_within_budget() {
        // The tool-schema block is fixed overhead resent on every model call, so
        // it directly shrinks the usable context window. Pin its size so a new
        // tool or a verbose schema can't silently balloon the prefix. Current
        // size is ~11.6K chars (~2.9K tokens): the background-task trio
        // (`is_background` + `task_output` + `kill_task`), `edit_file`'s batch
        // form (~900 chars, repaid the first time a rename lands six call
        // sites in one call), and `retrieve_original` (~400, now always
        // present because output caps drop content nothing else can recover).
        // Derived schemas document every field and enum variant — deliberate
        // spend; `schema_for` strips what carries no meaning. Two raises in one
        // feature is the limit: the next tool either replaces one, or argues
        // for its permanent prefix cost in the commit that adds it.
        //
        // `update_trail` (~1.7K, budget 12K → 13.5K): the Ledger home screen
        // renders every session's journey from the snapshot this tool
        // maintains — it is the one tool whose absence degrades a headline
        // surface for every thread, and its guidance (when to chart, the
        // standard route, how it differs from update_plan) is what keeps
        // models from spamming it or skipping it.
        //
        // `gh` (~0.9K, 13.5K → 14.5K): every code trail now ends in shipping
        // stages the MODEL must verify (pushed, pr-reviewed, merged) — this
        // is the tool that verifies them and opens the PR, so it earns its
        // permanent seat next to `git`.
        let workspace = Workspace::new(".").unwrap();
        let registry = ToolRegistry::default_for_workspace(workspace);
        let chars: usize = registry
            .definitions()
            .iter()
            .map(|d| d.to_string().len())
            .sum();
        assert!(
            chars < 14_500,
            "default tool definitions grew to {chars} chars (budget 14500)"
        );
    }
}
