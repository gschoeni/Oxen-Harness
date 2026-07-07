//! Retrieval of compressed-away tool output (the "R" in compress-cache-
//! retrieve).
//!
//! When context compression is on, bulky tool results are shrunk before each
//! model call and the removed content is stashed in a
//! [`harness_compress::CcrStore`] behind an inline `<<ccr:HASH>>` marker. This
//! tool is the model's way back: given the hash, it returns the full original.
//!
//! Registered by the agent itself when compression mode is `on` — it is not
//! part of the default workspace tool set, since without compression there are
//! no markers to resolve.

use std::sync::Arc;

use async_trait::async_trait;
use harness_compress::CcrStore;

use crate::{ToolError, TypedTool};

pub const RETRIEVE_ORIGINAL_TOOL: &str = "retrieve_original";

/// What `retrieve_original` accepts.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RetrieveArgs {
    /// The hex hash from a `<<ccr:HASH ...>>` marker in earlier tool output.
    pub hash: String,
}

/// Serves originals back out of the compression store.
pub struct RetrieveOriginalTool {
    store: Arc<CcrStore>,
}

impl RetrieveOriginalTool {
    pub fn new(store: Arc<CcrStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl TypedTool for RetrieveOriginalTool {
    const NAME: &'static str = RETRIEVE_ORIGINAL_TOOL;
    type Args = RetrieveArgs;

    fn description(&self) -> &str {
        "Recover the full original content behind a <<ccr:HASH>> marker. Earlier tool output \
         in this conversation may have been compressed to save context: dropped rows or elided \
         lines are replaced by a marker like <<ccr:a1b2c3d4e5f6 42_rows_offloaded>>. Call this \
         with the marker's hash when the compressed view is missing something you need. Prefer \
         working with the compressed view when it already answers the question."
    }

    async fn run(&self, args: RetrieveArgs) -> Result<String, ToolError> {
        // Accept the bare hash or a pasted marker (`<<ccr:HASH note>>`).
        let hash = args
            .hash
            .trim()
            .trim_start_matches("<<ccr:")
            .trim_end_matches(">>")
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        if hash.is_empty() {
            return Err(ToolError::InvalidArguments(
                "provide the hex hash from a <<ccr:HASH>> marker".into(),
            ));
        }
        match self.store.get(&hash) {
            Some(original) => Ok(original),
            None => Ok(format!(
                "No stored content for hash {hash} — it may have been evicted or the hash is \
                 mistyped. Work with the compressed view, or ask the user to re-run the \
                 original tool call if the full output is essential."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn retrieves_stored_original_by_hash_or_pasted_marker() {
        let store = Arc::new(CcrStore::default());
        let hash = store.put("the full original output");
        let tool = RetrieveOriginalTool::new(store);

        let out = tool.run(RetrieveArgs { hash: hash.clone() }).await.unwrap();
        assert_eq!(out, "the full original output");

        // A pasted marker (with note) resolves too.
        let out = tool
            .run(RetrieveArgs {
                hash: format!("<<ccr:{hash} 42_rows_offloaded>>"),
            })
            .await
            .unwrap();
        assert_eq!(out, "the full original output");
    }

    #[tokio::test]
    async fn unknown_hash_returns_guidance_not_an_error() {
        let tool = RetrieveOriginalTool::new(Arc::new(CcrStore::default()));
        let out = tool
            .run(RetrieveArgs {
                hash: "ffffffffffff".into(),
            })
            .await
            .unwrap();
        assert!(out.contains("No stored content"));
    }

    #[tokio::test]
    async fn empty_hash_is_invalid_arguments() {
        let tool = RetrieveOriginalTool::new(Arc::new(CcrStore::default()));
        let err = tool.run(RetrieveArgs { hash: "  ".into() }).await;
        assert!(matches!(err, Err(ToolError::InvalidArguments(_))));
    }
}
