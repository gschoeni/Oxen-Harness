//! Built-in agent tools for oxen-harness.
//!
//! This crate defines the [`TypedTool`] trait every capability implements (see
//! its docs for the add-a-tool recipe, or "Adding a tool" in the repo README),
//! a [`ToolRegistry`] for dispatching model tool calls by name, and the
//! concrete tools the agent uses: file read/write/edit, glob file discovery,
//! and regex content search ([`fs`]), sandboxed shell execution ([`shell`]),
//! git operations ([`git`]), Brave-backed web search ([`web`]), the task
//! checklist ([`plan`]), asking the user structured multiple-choice questions
//! ([`ask`]), and side-panel documents ([`canvas`]). All file access is
//! confined to a [`sandbox::Workspace`].
//!
//! The lower-level [`Tool`] trait (raw JSON in, string out) exists for tools
//! whose schema is only known at runtime — user-defined [`CustomToolSpec`]
//! tools are the one case. New built-in tools should implement [`TypedTool`].

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod ask;
pub mod canvas;
pub mod fs;
pub mod git;
pub mod plan;
pub mod sandbox;
pub mod shell;
pub mod web;

pub use ask::{AskUserTool, Choice, Question, QuestionAnswer, QuestionAsker, ASK_USER_TOOL};
pub use canvas::{CanvasDoc, CanvasSink, CanvasTool, CANVAS_FORMATS, CANVAS_TOOL};
pub use fs::{EDIT_FILE_TOOL, FIND_FILES_TOOL, READ_FILE_TOOL, SEARCH_FILES_TOOL, WRITE_FILE_TOOL};
pub use git::GIT_TOOL;
pub use plan::{PlanItem, PlanStatus, PlanTool, PLAN_TOOL};
pub use sandbox::Workspace;
pub use shell::RUN_SHELL_TOOL;
pub use web::WEB_SEARCH_TOOL;

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
                let body = res
                    .text()
                    .await
                    .map_err(|e| ToolError::Execution(format!("could not read response: {e}")))?;
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
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
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
        let mut registry = Self::new()
            .with_typed(fs::ReadFileTool::new(workspace.clone()))
            .with_typed(fs::WriteFileTool::new(workspace.clone()))
            .with_typed(fs::EditFileTool::new(workspace.clone()))
            .with_typed(fs::FindFilesTool::new(workspace.clone()))
            .with_typed(fs::SearchTool::new(workspace.clone()))
            .with_typed(shell::ShellTool::new(workspace.clone()))
            .with_typed(git::GitTool::new(workspace))
            // Planning/checklist tool — always available so any host gets it.
            .with_typed(plan::PlanTool::new());

        // Always register web search so the model can use it; when no Brave key
        // is configured the call fails with a recognizable error that the UIs
        // turn into an inline "add your API key" prompt.
        registry.register_typed(web::WebSearchTool::with_key(brave_key));
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
                git::GIT_TOOL,
                fs::READ_FILE_TOOL,
                shell::RUN_SHELL_TOOL,
                fs::SEARCH_FILES_TOOL,
                plan::PLAN_TOOL,
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
        // size is ~8.2K chars (~2K tokens) — derived schemas document every
        // field and enum variant, which is deliberate spend; `schema_for`
        // strips what carries no meaning. The ceiling leaves headroom for a
        // tool or two without inviting unchecked growth.
        let workspace = Workspace::new(".").unwrap();
        let registry = ToolRegistry::default_for_workspace(workspace);
        let chars: usize = registry
            .definitions()
            .iter()
            .map(|d| d.to_string().len())
            .sum();
        assert!(
            chars < 9_500,
            "default tool definitions grew to {chars} chars (budget 9500)"
        );
    }
}
