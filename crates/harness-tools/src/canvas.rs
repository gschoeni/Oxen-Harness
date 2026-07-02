//! `canvas` — show a substantial, standalone document in a side panel next to
//! the chat (like Claude Artifacts / ChatGPT Canvas).
//!
//! The model calls this when it produces a deliverable the user will read,
//! iterate on, or keep — a report, a rendered web page, a vector graphic, a
//! sizeable code file — rather than burying it in the chat. The document is addressed by
//! a stable `id`: calling `canvas` again with the same `id` *updates* the open
//! document in place.
//!
//! Rendering is host-specific (a desktop side panel, a browser tab from the
//! CLI), so this module defines only the [`CanvasDoc`] data, the [`CanvasSink`]
//! trait a front end implements, and the [`CanvasTool`] that bridges a model
//! tool call to that sink.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{ToolError, TypedTool};

/// The tool name the model calls (and front ends special-case for rendering).
pub const CANVAS_TOOL: &str = "canvas";

/// The document formats a canvas can render.
pub const CANVAS_FORMATS: &[&str] = &["markdown", "html", "code", "svg"];

/// A canvas document format, as the model supplies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CanvasFormat {
    /// Rich text: a report, article, or document.
    Markdown,
    /// A rendered web page or interactive experience.
    Html,
    /// A source file (set `language`).
    Code,
    /// A vector image.
    Svg,
}

impl CanvasFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Code => "code",
            Self::Svg => "svg",
        }
    }
}

/// A document to display in the canvas. Addressed by [`CanvasDoc::id`] so a
/// later call with the same id updates the same panel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CanvasDoc {
    /// Stable identifier; reuse it to update an existing document.
    pub id: String,
    /// Short human title shown above the document.
    pub title: String,
    /// One of [`CANVAS_FORMATS`]: `markdown`, `html`, `code`, `svg`.
    pub format: String,
    /// For `format = "code"`, the language hint (e.g. `"python"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The document body.
    pub content: String,
}

impl CanvasDoc {
    /// The conventional file extension for this document's format/language —
    /// used by hosts that write the doc to disk.
    pub fn extension(&self) -> &str {
        match self.format.as_str() {
            "html" => "html",
            "svg" => "svg",
            "code" => code_extension(self.language.as_deref()),
            _ => "md",
        }
    }
}

/// A front end that can show (or update) a canvas document.
///
/// Returns an optional host note appended to the model-visible result — e.g. the
/// CLI reports the file it wrote ("saved to …"), while a GUI panel returns
/// `None`. A host without any canvas surface should degrade gracefully rather
/// than error.
#[async_trait]
pub trait CanvasSink: Send + Sync {
    async fn show(&self, doc: &CanvasDoc) -> Result<Option<String>, ToolError>;
}

/// The model-facing tool that opens/updates the canvas.
pub struct CanvasTool {
    sink: Arc<dyn CanvasSink>,
}

impl CanvasTool {
    pub fn new(sink: Arc<dyn CanvasSink>) -> Self {
        Self { sink }
    }
}

/// Lowercase, filesystem/anchor-safe slug of a title (fallback document id),
/// capped at 64 characters.
fn slug(title: &str) -> String {
    harness_core::text::slug(title, "document")
        .chars()
        .take(64)
        .collect()
}

/// Arguments to `canvas`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct CanvasArgs {
    /// Stable id for the document. Reuse the same id to UPDATE a document you
    /// previously showed; omit it for a new document (one is derived from the
    /// title).
    pub id: Option<String>,
    /// Short human title for the document.
    pub title: Option<String>,
    /// markdown = rich text/report; html = a rendered web page or interactive
    /// experience; code = a source file (set `language`); svg = a vector image.
    pub format: CanvasFormat,
    /// For format=code, the source language (e.g. 'python', 'rust', 'typescript').
    pub language: Option<String>,
    /// The full document body. When updating, send the complete new content,
    /// not a diff.
    pub content: String,
}

/// Validate + normalize the tool arguments into a [`CanvasDoc`].
fn build_doc(args: CanvasArgs) -> Result<CanvasDoc, String> {
    if args.content.trim().is_empty() {
        return Err("missing non-empty `content`".into());
    }
    let title = args
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Document")
        .to_string();
    let language = args
        .language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // A model-supplied id lets it target updates; otherwise derive one from the
    // title so "update the report" naturally re-targets the same document.
    let id = args
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(slug)
        .unwrap_or_else(|| slug(&title));

    Ok(CanvasDoc {
        id,
        title,
        format: args.format.as_str().to_string(),
        language,
        content: args.content,
    })
}

#[async_trait]
impl TypedTool for CanvasTool {
    const NAME: &'static str = CANVAS_TOOL;
    type Args = CanvasArgs;

    fn description(&self) -> &str {
        "Display a standalone document in a side-panel canvas next to the chat, \
         or update one you already opened. Use this for substantial, \
         self-contained deliverables the user will read, iterate on, or keep — \
         a report or article (markdown), a rendered web page or interactive demo \
         (html), a sizeable code file (code), or a vector graphic (svg). Prefer \
         it over a long fenced block in chat for anything \
         roughly 15+ lines or that stands on its own. Do NOT use it for short \
         answers, quick snippets, or conversational replies — opening a panel for \
         those is disruptive. To revise a document you already showed, call \
         `canvas` again with the SAME `id` and the full updated content."
    }

    async fn run(&self, args: CanvasArgs) -> Result<String, ToolError> {
        let doc = build_doc(args).map_err(ToolError::InvalidArguments)?;
        let note = self.sink.show(&doc).await?;
        let mut msg = format!(
            "Showed canvas \"{}\" ({}) [id={}]. The user can see it; revise it by \
             calling canvas again with id=\"{}\".",
            doc.title, doc.format, doc.id, doc.id
        );
        if let Some(note) = note {
            msg.push(' ');
            msg.push_str(&note);
        }
        Ok(msg)
    }
}

/// A reasonable file extension for a code document given its language hint.
fn code_extension(language: Option<&str>) -> &str {
    match language.map(|l| l.to_ascii_lowercase()) {
        Some(l) => match l.as_str() {
            "python" | "py" => "py",
            "rust" | "rs" => "rs",
            "typescript" | "ts" => "ts",
            "javascript" | "js" => "js",
            "tsx" => "tsx",
            "jsx" => "jsx",
            "json" => "json",
            "toml" => "toml",
            "yaml" | "yml" => "yaml",
            "go" => "go",
            "c" => "c",
            "cpp" | "c++" => "cpp",
            "java" => "java",
            "ruby" | "rb" => "rb",
            "shell" | "bash" | "sh" => "sh",
            "sql" => "sql",
            "css" => "css",
            "html" => "html",
            _ => "txt",
        },
        None => "txt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Parse raw JSON the way dispatch does (serde), then normalize — the same
    /// path a model tool call takes.
    fn parse_doc(args: &serde_json::Value) -> Result<CanvasDoc, String> {
        let parsed: CanvasArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
        build_doc(parsed)
    }

    /// A sink that records the last document it was shown.
    struct CapturingSink(Mutex<Option<CanvasDoc>>);

    #[async_trait]
    impl CanvasSink for CapturingSink {
        async fn show(&self, doc: &CanvasDoc) -> Result<Option<String>, ToolError> {
            *self.0.lock().unwrap() = Some(doc.clone());
            Ok(None)
        }
    }

    #[test]
    fn derives_id_from_title_when_omitted() {
        let doc = parse_doc(&serde_json::json!({
            "title": "Q3 Launch Plan!",
            "format": "markdown",
            "content": "# Plan"
        }))
        .unwrap();
        assert_eq!(doc.id, "q3-launch-plan");
        assert_eq!(doc.extension(), "md");
    }

    #[test]
    fn rejects_unknown_format_and_empty_content() {
        assert!(parse_doc(&serde_json::json!({
            "format": "pdf", "content": "x"
        }))
        .is_err());
        assert!(parse_doc(&serde_json::json!({
            "format": "markdown", "content": "   "
        }))
        .is_err());
    }

    #[test]
    fn code_docs_use_language_extension() {
        let doc = parse_doc(&serde_json::json!({
            "title": "parser",
            "format": "code",
            "language": "rust",
            "content": "fn main() {}"
        }))
        .unwrap();
        assert_eq!(doc.extension(), "rs");
    }

    #[tokio::test]
    async fn invoke_shows_doc_and_reports_id() {
        let sink = Arc::new(CapturingSink(Mutex::new(None)));
        let tool = CanvasTool::new(sink.clone());
        let out = tool
            .invoke(serde_json::json!({
                "id": "report",
                "title": "Report",
                "format": "markdown",
                "content": "# Hello"
            }))
            .await
            .unwrap();
        assert!(out.contains("id=report"), "out: {out}");
        let shown = sink.0.lock().unwrap().clone().unwrap();
        assert_eq!(shown.id, "report");
        assert_eq!(shown.content, "# Hello");
    }

    #[test]
    fn schema_advertises_format_enum() {
        assert_eq!(CanvasTool::NAME, CANVAS_TOOL);
        let schema = crate::schema_for::<CanvasArgs>();
        // Every format the hosts can render must be advertised in the enum.
        let formats: Vec<&str> = schema["properties"]["format"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(formats, CANVAS_FORMATS);
    }
}
