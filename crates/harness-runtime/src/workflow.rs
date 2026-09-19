//! Portable Oxen workflows: the same graph is edited by a renderer or the agent.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const WORKFLOW_VERSION: u32 = 1;
pub const MAX_NODES: usize = 128;
pub const MAX_EDGES: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Workflow {
    pub version: u32,
    pub title: String,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowNode {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub title: String,
    pub position: Position,
    #[serde(default)]
    pub config: Map<String, Value>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl WorkflowNode {
    pub fn text(&self, key: &str) -> &str {
        self.config.get(key).and_then(Value::as_str).unwrap_or("")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(default = "output_port")]
    pub source_port: String,
    pub target_port: String,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

fn output_port() -> String {
    "output".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Diagnostic {
    pub node: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NodeDefinition {
    pub kind: String,
    pub title: String,
    pub description: String,
    pub inputs: BTreeMap<String, String>,
    pub output: String,
}

pub fn node_definitions() -> Vec<NodeDefinition> {
    [
        (
            "prompt",
            "Prompt",
            "Write the starting prompt.",
            vec![],
            "text",
        ),
        (
            "image_input",
            "Image input",
            "Use a project image as a reference.",
            vec![],
            "image",
        ),
        (
            "video_input",
            "Video input",
            "Use a project video as a reference.",
            vec![],
            "video",
        ),
        (
            "rewrite",
            "Rewrite prompt",
            "Refine a prompt with an Oxen language model.",
            vec![("prompt", "text")],
            "text",
        ),
        (
            "image",
            "Generate image",
            "Generate images with an Oxen image model.",
            vec![("prompt", "text"), ("image", "image")],
            "image",
        ),
        (
            "video",
            "Generate video",
            "Animate an image or generate a video with Oxen.",
            vec![("prompt", "text"), ("image", "image"), ("video", "video")],
            "video",
        ),
        (
            "upscale",
            "Upscale image",
            "Enhance an image with an Oxen upscaler.",
            vec![("image", "image")],
            "image",
        ),
        (
            "video_upscale",
            "Upscale video",
            "Enhance a video with an Oxen upscaler.",
            vec![("video", "video")],
            "video",
        ),
        (
            "output",
            "Output",
            "Collect finished text, images, and video.",
            vec![("input", "any")],
            "none",
        ),
    ]
    .into_iter()
    .map(
        |(kind, title, description, inputs, output)| NodeDefinition {
            kind: kind.into(),
            title: title.into(),
            description: description.into(),
            inputs: inputs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            output: output.into(),
        },
    )
    .collect()
}

impl Workflow {
    pub fn parse(content: &str) -> Result<Self, String> {
        let graph: Self =
            serde_json::from_str(content).map_err(|e| format!("invalid workflow JSON: {e}"))?;
        if graph.version != WORKFLOW_VERSION {
            return Err(format!(
                "unsupported workflow version {}; supported version is {WORKFLOW_VERSION}",
                graph.version
            ));
        }
        Ok(graph)
    }

    /// Structural errors are useful during editing; required inputs are only
    /// checked for a run, allowing users to save an unfinished graph.
    pub fn diagnostics(&self, for_run: bool) -> Vec<Diagnostic> {
        let mut errors = Vec::new();
        let mut error = |node: Option<&str>, message: String| {
            errors.push(Diagnostic {
                node: node.map(str::to_string),
                message,
            })
        };
        if self.version != WORKFLOW_VERSION {
            error(
                None,
                format!("unsupported workflow version {}", self.version),
            );
        }
        if self.nodes.len() > MAX_NODES || self.edges.len() > MAX_EDGES {
            error(
                None,
                format!("workflow limit: {MAX_NODES} nodes and {MAX_EDGES} connections"),
            );
            return errors;
        }
        let definitions = node_definitions();
        let mut nodes = BTreeMap::new();
        for node in &self.nodes {
            if !valid_id(&node.id) {
                error(
                    Some(&node.id),
                    "node id must use 1–80 letters, digits, dots, dashes or underscores".into(),
                );
            }
            if nodes.insert(node.id.as_str(), node).is_some() {
                error(Some(&node.id), "duplicate node id".into());
            }
            if !node.position.x.is_finite() || !node.position.y.is_finite() {
                error(Some(&node.id), "node position must be finite".into());
            }
            if !definitions.iter().any(|d| d.kind == node.kind) {
                error(
                    Some(&node.id),
                    format!("unsupported node type {}", node.kind),
                );
            }
            if let Some(params) = node.config.get("params") {
                if !params.is_object() {
                    error(
                        Some(&node.id),
                        "model parameters must be a JSON object".into(),
                    );
                }
            }
        }
        let mut ids = BTreeSet::new();
        let mut inputs = BTreeSet::new();
        for edge in &self.edges {
            if !valid_id(&edge.id) || !ids.insert(&edge.id) {
                error(
                    None,
                    format!("invalid or duplicate connection id {}", edge.id),
                );
            }
            let (Some(source), Some(target)) = (
                nodes.get(edge.source.as_str()),
                nodes.get(edge.target.as_str()),
            ) else {
                error(
                    None,
                    format!("connection {} references a missing node", edge.id),
                );
                continue;
            };
            let source_def = definitions.iter().find(|d| d.kind == source.kind);
            let target_def = definitions.iter().find(|d| d.kind == target.kind);
            let Some((source_def, target_def)) = source_def.zip(target_def) else {
                continue;
            };
            let input_type = target_def.inputs.get(&edge.target_port);
            if edge.source_port != "output" || source_def.output == "none" {
                error(
                    Some(&source.id),
                    format!("connection {} has an invalid output port", edge.id),
                );
            }
            match input_type {
                Some(input) if input == "any" || input == &source_def.output => {}
                Some(input) => error(
                    Some(&target.id),
                    format!(
                        "{} needs {input}, received {}",
                        edge.target_port, source_def.output
                    ),
                ),
                None => error(
                    Some(&target.id),
                    format!("unknown input port {}", edge.target_port),
                ),
            }
            let first = inputs.insert((edge.target.as_str(), edge.target_port.as_str()));
            if !first && edge.target_port != "input" {
                error(
                    Some(&target.id),
                    format!("{} already has a connection", edge.target_port),
                );
            }
        }
        if let Err(message) = self.execution_order() {
            error(None, message);
        }
        if for_run {
            if self.nodes.is_empty() {
                error(None, "add a node before running the workflow".into());
            }
            for node in &self.nodes {
                let connected = |port: &str| inputs.contains(&(node.id.as_str(), port));
                match node.kind.as_str() {
                    "prompt" if node.text("text").trim().is_empty() => {
                        error(Some(&node.id), "write a prompt".into())
                    }
                    "image_input" | "video_input" if node.text("path").trim().is_empty() => {
                        error(Some(&node.id), "choose a project media file".into())
                    }
                    "rewrite" | "image" | "video"
                        if !connected("prompt") && node.text("prompt").trim().is_empty() =>
                    {
                        error(Some(&node.id), "connect or write a prompt".into())
                    }
                    "upscale" if !connected("image") => {
                        error(Some(&node.id), "connect an image to upscale".into())
                    }
                    "video_upscale" if !connected("video") => {
                        error(Some(&node.id), "connect a video to upscale".into())
                    }
                    "output" if !connected("input") => {
                        error(Some(&node.id), "connect an output".into())
                    }
                    _ => {}
                }
            }
        }
        errors
    }

    pub fn execution_order(&self) -> Result<Vec<String>, String> {
        if self.nodes.len() > MAX_NODES || self.edges.len() > MAX_EDGES {
            return Err("workflow exceeds node/connection limit".into());
        }
        let mut indegree: BTreeMap<&str, usize> =
            self.nodes.iter().map(|n| (n.id.as_str(), 0)).collect();
        for edge in &self.edges {
            if !indegree.contains_key(edge.source.as_str()) {
                return Err(format!("missing node {}", edge.source));
            }
            let degree = indegree
                .get_mut(edge.target.as_str())
                .ok_or_else(|| format!("missing node {}", edge.target))?;
            *degree += 1;
        }
        let mut queue: VecDeque<&str> = self
            .nodes
            .iter()
            .filter(|n| indegree.get(n.id.as_str()) == Some(&0))
            .map(|n| n.id.as_str())
            .collect();
        let mut order = Vec::new();
        while let Some(id) = queue.pop_front() {
            order.push(id.to_string());
            for edge in self.edges.iter().filter(|edge| edge.source == id) {
                if let Some(degree) = indegree.get_mut(edge.target.as_str()) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        queue.push_back(&edge.target);
                    }
                }
            }
        }
        if order.len() != self.nodes.len() {
            return Err("workflow contains a cycle or duplicate node ids".into());
        }
        Ok(order)
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

/// Parameters contain scalar/model options only. Reference media is resolved
/// by the host from connected, workspace-scoped nodes, never arbitrary URLs.
pub fn validate_parameters(params: &Value, schema: &Value) -> Result<(), String> {
    let params = params
        .as_object()
        .ok_or("model parameters must be an object")?;
    let properties = schema.get("properties").and_then(Value::as_object);
    for (name, value) in params {
        let property = properties
            .and_then(|p| p.get(name))
            .ok_or_else(|| format!("unknown model parameter {name}"))?;
        if [
            "prompt",
            "model",
            "num_generations",
            "target_repo",
            "target_namespace",
            "target_directory",
        ]
        .contains(&name.as_str())
            || property.get("format").and_then(Value::as_str) == Some("uri")
            || property.pointer("/items/format").and_then(Value::as_str) == Some("uri")
        {
            return Err(format!(
                "{name} is supplied through a workflow input, not model parameters"
            ));
        }
        validate_value(name, value, property)?;
    }
    Ok(())
}

fn validate_value(name: &str, value: &Value, schema: &Value) -> Result<(), String> {
    if value.is_null() && schema.get("nullable").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let valid_type = match schema.get("type").and_then(Value::as_str) {
        Some("string") => value.is_string(),
        Some("number") => value.is_number(),
        Some("integer") => value.is_i64() || value.is_u64(),
        Some("boolean") => value.is_boolean(),
        Some("array") => value.is_array(),
        Some("object") => value.is_object(),
        _ => true,
    };
    if !valid_type {
        return Err(format!("{name} has the wrong parameter type"));
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
        if !choices.contains(value) {
            return Err(format!(
                "{name} must be one of the model's advertised choices"
            ));
        }
    }
    if let Some(number) = value.as_f64() {
        if schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|min| number < min)
            || schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|max| number > max)
        {
            return Err(format!("{name} is outside the model's allowed range"));
        }
    }
    if let Some(text) = value.as_str() {
        if schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|max| text.chars().count() as u64 > max)
        {
            return Err(format!("{name} exceeds the model's maximum length"));
        }
    }
    if let Some(items) = value.as_array() {
        if schema
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|max| items.len() as u64 > max)
        {
            return Err(format!("{name} has too many items"));
        }
        if let Some(item_schema) = schema.get("items") {
            for item in items {
                validate_value(name, item, item_schema)?;
            }
        }
    }
    Ok(())
}

/// The same schema advertised to agents and plugin authors.
pub fn schema() -> Value {
    serde_json::to_value(schemars::schema_for!(Workflow))
        .unwrap_or_else(|e| serde_json::json!({"error":e.to_string()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph() -> Workflow {
        serde_json::from_value(json!({
            "version": 1, "title": "Product film", "custom": {"kept": true},
            "nodes": [
                {"id":"prompt", "kind":"prompt", "position":{"x":0,"y":0}, "config":{"text":"A ceramic vase"}},
                {"id":"rewrite", "kind":"rewrite", "position":{"x":250,"y":0}, "config":{}},
                {"id":"image", "kind":"image", "position":{"x":500,"y":0}, "config":{}},
                {"id":"upscale", "kind":"upscale", "position":{"x":750,"y":0}, "config":{"model":"flux-image-upscaler"}},
                {"id":"video", "kind":"video", "position":{"x":1000,"y":0}, "config":{"prompt":"Slow orbit"}},
                {"id":"out", "kind":"output", "position":{"x":1250,"y":0}, "config":{}}
            ],
            "edges": [
                {"id":"a","source":"prompt","target":"rewrite","target_port":"prompt"},
                {"id":"b","source":"rewrite","target":"image","target_port":"prompt"},
                {"id":"c","source":"image","target":"upscale","target_port":"image"},
                {"id":"d","source":"upscale","target":"video","target_port":"image"},
                {"id":"e","source":"video","target":"out","target_port":"input"}
            ]
        })).unwrap()
    }

    #[test]
    fn compiles_a_typed_image_upscale_video_pipeline_and_preserves_extensions() {
        let g = graph();
        assert!(g.diagnostics(true).is_empty(), "{:?}", g.diagnostics(true));
        assert_eq!(
            g.execution_order().unwrap(),
            ["prompt", "rewrite", "image", "upscale", "video", "out"]
        );
        assert_eq!(serde_json::to_value(&g).unwrap()["custom"]["kept"], true);
    }

    #[test]
    fn refuses_cycles_type_mismatches_missing_nodes_and_duplicate_ids() {
        let mut g = graph();
        g.edges[0].source = "rewrite".into();
        assert!(g
            .diagnostics(true)
            .iter()
            .any(|d| d.message.contains("cycle")));
        g = graph();
        g.edges[1].source = "upscale".into();
        assert!(g
            .diagnostics(true)
            .iter()
            .any(|d| d.message.contains("text")));
        g = graph();
        g.edges[0].source = "gone".into();
        assert!(g
            .diagnostics(true)
            .iter()
            .any(|d| d.message.contains("missing")));
        g = graph();
        g.nodes[1].id = "prompt".into();
        assert!(g
            .diagnostics(true)
            .iter()
            .any(|d| d.message.contains("duplicate")));
    }

    #[test]
    fn unfinished_graphs_can_be_saved_but_cannot_run() {
        let mut g = graph();
        g.edges.clear();
        assert!(g.diagnostics(false).is_empty());
        assert!(!g.diagnostics(true).is_empty());
        g.nodes[2].kind = "future.custom_node".into();
        assert!(g
            .diagnostics(true)
            .iter()
            .any(|d| d.message.contains("unsupported")));
        assert_eq!(
            serde_json::to_value(g).unwrap()["nodes"][2]["kind"],
            "future.custom_node"
        );
    }

    #[test]
    fn validates_model_parameters_without_accepting_arbitrary_reference_urls() {
        let schema = json!({"type":"object", "required":["scale"], "properties":{
            "scale":{"type":"integer","minimum":1,"maximum":4},
            "quality":{"type":"string","enum":["low","high"]}
        }});
        assert!(validate_parameters(&json!({"scale":2,"quality":"high"}), &schema).is_ok());
        assert!(validate_parameters(&json!({"scale":7}), &schema).is_err());
        assert!(validate_parameters(&json!({"scale":2,"quality":"typo"}), &schema).is_err());
        assert!(validate_parameters(
            &json!({"scale":2,"input_image":"https://example.com"}),
            &schema
        )
        .is_err());
    }
}
