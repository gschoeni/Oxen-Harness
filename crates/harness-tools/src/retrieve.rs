//! Retrieval of compressed-away tool output (the "R" in compress-cache-
//! retrieve).
//!
//! When context compression is on, bulky tool results are shrunk before each
//! model call and the removed content is stashed in a
//! [`harness_compress::CcrStore`] behind an inline `<<ccr:HASH>>` marker. This
//! tool is the model's way back: given the hash, it returns the full original.
//!
//! It is also how anything *parked* is read: a tool result over the agent's
//! cap, a fleet lane's whole reply, a patch. The parked thing is a variable
//! the model never has to load whole — `lines` reads a slice, `grep` finds
//! the lines that matter, and `chunks` splits it into parked pieces an agent
//! each can take (the recursive-language-model move: the context stays out
//! of the context).

use std::sync::Arc;

use async_trait::async_trait;
use harness_compress::CcrStore;

use crate::{CallContext, ToolError, TypedTool};

pub const RETRIEVE_ORIGINAL_TOOL: &str = "retrieve_original";

/// What `retrieve_original` accepts.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RetrieveArgs {
    /// The hex hash from a `<<ccr:HASH ...>>` marker in earlier tool output.
    pub hash: String,
    /// Optional line range instead of the whole thing: "40-80", "120-", "-30".
    #[serde(default)]
    pub lines: Option<String>,
    /// Optional case-insensitive search; returns matching lines with context.
    #[serde(default)]
    pub grep: Option<String>,
    /// Optional: split into this many pieces (2-64), each parked under its own
    /// marker, and list them — for agents' `inputs`.
    #[serde(default)]
    pub chunks: Option<usize>,
}

/// Most characters a whole retrieval hands back; past this the model is
/// steered to a slice, a grep, or chunks.
pub const MAX_RETRIEVE_CHARS: usize = 60_000;

/// Most pieces `chunks` will cut.
pub const MAX_CHUNKS: usize = 64;

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
        "Read content parked behind a <<ccr:HASH>> marker — a tool result over the size cap, \
         an agent's full reply, a patch, or rows compression removed. `lines` reads a slice, \
         `grep` finds the lines that matter, `chunks` splits it into parked pieces for agents \
         (spawn_agents / ask_model `inputs`). Read only what you need."
    }

    async fn run(&self, args: RetrieveArgs, _call: &CallContext) -> Result<String, ToolError> {
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
        let Some(original) = self.store.get(&hash) else {
            return Ok(format!(
                "No stored content for hash {hash} — it may have been evicted or the hash is \
                 mistyped. Work with the compressed view, or ask the user to re-run the \
                 original tool call if the full output is essential."
            ));
        };
        if let Some(needle) = args
            .grep
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            let hits = harness_core::text::grep_lines(&original, needle, 2, 80);
            return Ok(if hits.is_empty() {
                format!("no line contains {needle:?}")
            } else {
                hits
            });
        }
        if let Some(spec) = args.lines.as_deref() {
            let (from, to) = harness_core::text::parse_line_range(spec).ok_or_else(|| {
                ToolError::InvalidArguments(format!(
                    "lines must look like \"40-80\", \"120-\" or \"-30\" (got {spec:?})"
                ))
            })?;
            return Ok(harness_core::text::slice_lines(&original, from, to));
        }
        if let Some(count) = args.chunks {
            return Ok(self.chunk(&original, count));
        }
        let total = original.lines().count();
        Ok(harness_core::text::truncate_with_marker(
            &original,
            MAX_RETRIEVE_CHARS,
            &format!(
                "\n… [cut at {MAX_RETRIEVE_CHARS} chars of {total} lines; use lines, grep, or \
                 chunks for the rest]"
            ),
        ))
    }
}

impl RetrieveOriginalTool {
    /// Cut `text` into `count` line-balanced pieces, park each, and list them.
    fn chunk(&self, text: &str, count: usize) -> String {
        let count = count.clamp(2, MAX_CHUNKS);
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() {
            return "(nothing to split)".into();
        }
        let count = count.min(lines.len());
        let per = lines.len().div_ceil(count);
        let mut out = format!("{count} pieces of {} lines:\n", lines.len());
        for (index, piece) in lines.chunks(per).enumerate() {
            let first = index * per + 1;
            let last = first + piece.len() - 1;
            let content = piece.join("\n");
            let hash = self.store.put(&content);
            out.push_str(&format!(
                "{}. {} lines {first}-{last} ({} chars)\n",
                index + 1,
                harness_compress::ccr::marker(&hash, Some("chunk")),
                content.chars().count()
            ));
        }
        out.trim_end().to_string()
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

        let out = tool
            .run(
                RetrieveArgs {
                    hash: hash.clone(),
                    lines: None,
                    grep: None,
                    chunks: None,
                },
                &CallContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(out, "the full original output");

        // A pasted marker (with note) resolves too.
        let out = tool
            .run(
                RetrieveArgs {
                    hash: format!("<<ccr:{hash} 42_rows_offloaded>>"),
                    lines: None,
                    grep: None,
                    chunks: None,
                },
                &CallContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(out, "the full original output");
    }

    #[tokio::test]
    async fn slices_greps_and_chunks_read_a_parked_thing_without_loading_it() {
        let store = Arc::new(CcrStore::default());
        let text = (1..=10)
            .map(|n| format!("line {n}{}", if n == 7 { " needle" } else { "" }))
            .collect::<Vec<_>>()
            .join("\n");
        let hash = store.put(&text);
        let tool = RetrieveOriginalTool::new(store.clone());
        let ask = |lines: Option<&str>, grep: Option<&str>, chunks: Option<usize>| RetrieveArgs {
            hash: hash.clone(),
            lines: lines.map(str::to_owned),
            grep: grep.map(str::to_owned),
            chunks,
        };

        assert_eq!(
            tool.run(ask(Some("3-4"), None, None), &CallContext::default())
                .await
                .unwrap(),
            "3: line 3\n4: line 4"
        );
        assert_eq!(
            tool.run(ask(None, Some("NEEDLE"), None), &CallContext::default())
                .await
                .unwrap(),
            "5: line 5\n6: line 6\n7: line 7 needle\n8: line 8\n9: line 9"
        );
        assert!(tool
            .run(ask(Some("x"), None, None), &CallContext::default())
            .await
            .is_err());

        // Chunks come back parked, each readable on its own, none of the
        // content in the reply.
        let listing = tool
            .run(ask(None, None, Some(3)), &CallContext::default())
            .await
            .unwrap();
        assert!(listing.starts_with("3 pieces of 10 lines:"), "{listing}");
        assert!(!listing.contains("line 1\n"), "{listing}");
        let markers: Vec<&str> = listing
            .split("<<ccr:")
            .skip(1)
            .filter_map(|rest| rest.split_whitespace().next())
            .collect();
        assert_eq!(markers.len(), 3);
        assert_eq!(
            store.get(markers[0]).as_deref(),
            Some("line 1\nline 2\nline 3\nline 4")
        );
        assert!(listing.contains("lines 1-4"), "{listing}");
        assert!(listing.contains("lines 9-10"), "{listing}");
        assert_eq!(store.get(markers[2]).as_deref(), Some("line 9\nline 10"));
    }

    #[tokio::test]
    async fn unknown_hash_returns_guidance_not_an_error() {
        let tool = RetrieveOriginalTool::new(Arc::new(CcrStore::default()));
        let out = tool
            .run(
                RetrieveArgs {
                    hash: "ffffffffffff".into(),
                    lines: None,
                    grep: None,
                    chunks: None,
                },
                &CallContext::default(),
            )
            .await
            .unwrap();
        assert!(out.contains("No stored content"));
    }

    #[tokio::test]
    async fn empty_hash_is_invalid_arguments() {
        let tool = RetrieveOriginalTool::new(Arc::new(CcrStore::default()));
        let err = tool
            .run(
                RetrieveArgs {
                    hash: "  ".into(),
                    lines: None,
                    grep: None,
                    chunks: None,
                },
                &CallContext::default(),
            )
            .await;
        assert!(matches!(err, Err(ToolError::InvalidArguments(_))));
    }
}
