//! Durable, sequential workflow execution. Editing never executes a graph.

use crate::{
    documents::{DocumentSnapshot, Documents},
    workflow::{Workflow, WorkflowNode},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub graph_path: String,
    pub graph_revision: String,
    pub graph: Workflow,
    pub status: String,
    pub nodes: BTreeMap<String, NodeResult>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeResult {
    pub status: String,
    pub outputs: Vec<Output>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Output {
    pub kind: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
}

#[async_trait]
pub trait Executor: Send + Sync {
    /// Validate every model and parameter before the first paid request.
    async fn validate(&self, graph: &Workflow) -> Result<(), String>;
    async fn execute(
        &self,
        run: &str,
        node: &WorkflowNode,
        inputs: BTreeMap<String, Vec<Output>>,
    ) -> Result<Vec<Output>, String>;
}

pub fn run_path(id: &str) -> Result<String, String> {
    if id.is_empty()
        || id.len() > 100
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("invalid workflow run id".into());
    }
    Ok(format!(".oxen-harness/workflow-runs/{id}.json"))
}

pub async fn create(
    docs: &Documents,
    document: DocumentSnapshot,
    executor: &dyn Executor,
) -> Result<Run, String> {
    let graph = Workflow::parse(&document.content)?;
    let errors = graph.diagnostics(true);
    if !errors.is_empty() {
        return Err(errors
            .into_iter()
            .map(|e| e.message)
            .collect::<Vec<_>>()
            .join("; "));
    }
    executor.validate(&graph).await?;
    let id = format!(
        "{:x}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
        std::process::id()
    );
    let run = Run {
        id,
        graph_path: document.path,
        graph_revision: document.revision,
        graph,
        status: "queued".into(),
        nodes: BTreeMap::new(),
        error: None,
    };
    docs.save(
        &run_path(&run.id)?,
        &serde_json::to_string_pretty(&run).map_err(|e| e.to_string())?,
        None,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(run)
}

async fn persist(docs: &Documents, run: &Run) -> Result<(), String> {
    let path = run_path(&run.id)?;
    let before = docs.read(&path).map_err(|e| e.to_string())?;
    docs.save(
        &path,
        &serde_json::to_string_pretty(run).map_err(|e| e.to_string())?,
        Some(&before.revision),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Cancellation finishes the current paid operation, preserving its result,
/// then skips downstream nodes. Never retry a possibly billed request silently.
pub async fn execute(
    docs: Documents,
    mut run: Run,
    executor: Arc<dyn Executor>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
) -> Result<Run, String> {
    let order = run.graph.execution_order()?;
    run.status = "running".into();
    persist(&docs, &run).await?;
    for id in order {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            run.status = "cancelled".into();
            break;
        }
        let node = run
            .graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or("workflow node disappeared")?;
        let mut inputs: BTreeMap<String, Vec<Output>> = BTreeMap::new();
        for edge in run.graph.edges.iter().filter(|e| e.target == id) {
            let result = run
                .nodes
                .get(&edge.source)
                .ok_or("dependency has no result")?;
            inputs
                .entry(edge.target_port.clone())
                .or_default()
                .extend(result.outputs.clone());
        }
        run.nodes.insert(
            id.clone(),
            NodeResult {
                status: "running".into(),
                ..Default::default()
            },
        );
        persist(&docs, &run).await?;
        match executor.execute(&run.id, node, inputs).await {
            Ok(outputs) => {
                run.nodes.insert(
                    id,
                    NodeResult {
                        status: "succeeded".into(),
                        outputs,
                        error: None,
                    },
                );
            }
            Err(error) => {
                run.nodes.insert(
                    id,
                    NodeResult {
                        status: "failed".into(),
                        outputs: vec![],
                        error: Some(error.clone()),
                    },
                );
                run.error = Some(error);
                run.status = "failed".into();
                break;
            }
        }
        persist(&docs, &run).await?;
    }
    if run.status == "running" {
        run.status = if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            "cancelled"
        } else {
            "succeeded"
        }
        .into();
    }
    persist(&docs, &run).await?;
    Ok(run)
}

pub fn parameters(node: &WorkflowNode) -> Value {
    node.config
        .get("params")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake;
    #[async_trait]
    impl Executor for Fake {
        async fn validate(&self, _: &Workflow) -> Result<(), String> {
            Ok(())
        }
        async fn execute(
            &self,
            _: &str,
            node: &WorkflowNode,
            inputs: BTreeMap<String, Vec<Output>>,
        ) -> Result<Vec<Output>, String> {
            if node.kind == "prompt" {
                Ok(vec![Output {
                    kind: "text".into(),
                    value: node.text("text").into(),
                    job: None,
                }])
            } else {
                Ok(inputs.into_values().flatten().collect())
            }
        }
    }
    #[tokio::test]
    async fn results_use_an_immutable_graph_snapshot_and_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let docs = Documents::new(dir.path()).unwrap();
        let doc = docs.save("a.graph.json", r#"{"version":1,"title":"Test","nodes":[{"id":"p","kind":"prompt","position":{"x":0,"y":0},"config":{"text":"original"}},{"id":"o","kind":"output","position":{"x":1,"y":1}}],"edges":[{"id":"e","source":"p","target":"o","target_port":"input"}]}"#, None).await.unwrap();
        let run = create(&docs, doc.clone(), &Fake).await.unwrap();
        docs.save(
            &doc.path,
            &doc.content.replace("original", "changed"),
            Some(&doc.revision),
        )
        .await
        .unwrap();
        let done = execute(docs.clone(), run, Arc::new(Fake), Arc::new(false.into()))
            .await
            .unwrap();
        assert_eq!(done.status, "succeeded");
        assert_eq!(done.nodes["o"].outputs[0].value, "original");
        let saved: Run = serde_json::from_str(
            &Documents::new(dir.path())
                .unwrap()
                .read(&run_path(&done.id).unwrap())
                .unwrap()
                .content,
        )
        .unwrap();
        assert_eq!(saved.nodes["o"].outputs, done.nodes["o"].outputs);
    }
    #[tokio::test]
    async fn cancellation_does_not_start_a_node() {
        let dir = tempfile::tempdir().unwrap();
        let docs = Documents::new(dir.path()).unwrap();
        let doc = docs.save("a.graph.json", r#"{"version":1,"title":"Test","nodes":[{"id":"p","kind":"prompt","position":{"x":0,"y":0},"config":{"text":"hello"}}],"edges":[]}"#, None).await.unwrap();
        let run = create(&docs, doc, &Fake).await.unwrap();
        let done = execute(docs, run, Arc::new(Fake), Arc::new(true.into()))
            .await
            .unwrap();
        assert_eq!(done.status, "cancelled");
        assert!(done.nodes.is_empty());
    }
}
