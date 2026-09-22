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

use crate::sandbox::Workspace;
use crate::{CallContext, ToolError, TypedTool};

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
    /// The workspace-relative file this document mirrors, when the model
    /// showed a project file rather than sending content: hosts that can
    /// watch the file follow its later edits without another call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
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
    /// Where `path` arguments resolve: a project file shown in the canvas
    /// is read through the sandbox, so the model can't render a file from
    /// outside the workspace.
    workspace: Workspace,
}

/// A project file the model asked to show, read through the sandbox.
struct ProjectFile {
    /// Workspace-relative, as the model named it (normalized).
    path: String,
    content: String,
}

impl CanvasTool {
    pub fn new(sink: Arc<dyn CanvasSink>, workspace: Workspace) -> Self {
        Self { sink, workspace }
    }

    async fn read_project_file(&self, rel: &str) -> Result<ProjectFile, ToolError> {
        let abs = self.workspace.resolve(rel)?;
        let content = tokio::fs::read_to_string(&abs).await.map_err(|e| {
            ToolError::Execution(format!(
                "could not read {rel} to show it in the canvas: {e}"
            ))
        })?;
        Ok(ProjectFile {
            path: rel.trim_start_matches("./").to_string(),
            content,
        })
    }
}

/// The canvas format a project file renders as, from its extension, with
/// the language hint a `code` document wants.
fn format_for_path(path: &str) -> (CanvasFormat, Option<String>) {
    let ext = path
        .rsplit('.')
        .next()
        .filter(|e| !e.contains('/'))
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" | "mdx" => (CanvasFormat::Markdown, None),
        "html" | "htm" => (CanvasFormat::Html, None),
        "svg" => (CanvasFormat::Svg, None),
        "" => (CanvasFormat::Code, None),
        other => (CanvasFormat::Code, Some(language_for_extension(other))),
    }
}

/// The inverse of [`code_extension`] for the common cases; anything else
/// keeps the extension as its language hint.
fn language_for_extension(ext: &str) -> String {
    match ext {
        "py" => "python",
        "rs" => "rust",
        "ts" => "typescript",
        "js" => "javascript",
        "rb" => "ruby",
        "sh" => "shell",
        "yml" => "yaml",
        other => other,
    }
    .to_string()
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
    /// Required with `content`; inferred from the extension with `path`.
    pub format: Option<CanvasFormat>,
    /// For format=code, the source language (e.g. 'python', 'rust', 'typescript').
    pub language: Option<String>,
    /// The full document body. When updating, send the complete new content,
    /// not a diff. Omit it when showing a project file with `path`.
    pub content: Option<String>,
    /// A project file to show instead of `content` (workspace-relative, e.g.
    /// "site/index.html"). The canvas renders the file and follows your later
    /// edits to it, so show a page or document you wrote to disk this way
    /// rather than sending its contents again.
    pub path: Option<String>,
}

/// Validate + normalize the tool arguments into a [`CanvasDoc`]. `file` is
/// the project file `path` named, already read; with it, the format, title,
/// and id default from the file rather than from the arguments.
fn build_doc(args: CanvasArgs, file: Option<ProjectFile>) -> Result<CanvasDoc, String> {
    let sent = args
        .content
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let (content, path, inferred) = match (file, sent) {
        (Some(_), Some(_)) => {
            return Err("send either `content` or `path`, not both".into());
        }
        (Some(file), None) => {
            let inferred = format_for_path(&file.path);
            (file.content, Some(file.path), Some(inferred))
        }
        (None, Some(_)) => (args.content.unwrap_or_default(), None, None),
        (None, None) => return Err("missing non-empty `content` (or a `path` to show)".into()),
    };
    let format = match (args.format, &inferred) {
        (Some(format), _) => format,
        (None, Some((format, _))) => *format,
        (None, None) => return Err("missing `format` (markdown, html, code, or svg)".into()),
    };
    // The file name is the natural title and id of a mirrored file, so
    // showing the same file again updates the same panel.
    let file_name = path
        .as_deref()
        .and_then(|p| p.rsplit('/').next())
        .map(str::to_string);
    let title = args
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| file_name.clone())
        .unwrap_or_else(|| "Document".to_string());
    let language = args
        .language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| inferred.and_then(|(_, language)| language))
        .filter(|_| format == CanvasFormat::Code);
    // A model-supplied id lets it target updates; otherwise derive one from the
    // file or the title so "update the report" naturally re-targets the same
    // document.
    let id = args
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(slug)
        .unwrap_or_else(|| slug(path.as_deref().unwrap_or(&title)));

    Ok(CanvasDoc {
        id,
        title,
        format: format.as_str().to_string(),
        language,
        content,
        path,
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
         `canvas` again with the SAME `id` and the full updated content. To \
         show a file you wrote to the project (an HTML page, a report), pass \
         its `path` instead of `content`: the canvas renders the file and \
         follows your later edits to it, so you never resend it."
    }

    fn concurrency(&self) -> crate::Concurrency {
        crate::Concurrency::Exclusive
    }

    async fn run(&self, args: CanvasArgs, _call: &CallContext) -> Result<String, ToolError> {
        let file = match args
            .path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            Some(rel) => Some(self.read_project_file(rel).await?),
            None => None,
        };
        let doc = build_doc(args, file).map_err(ToolError::InvalidArguments)?;
        let note = self.sink.show(&doc).await?;
        let mut msg = match &doc.path {
            Some(path) => format!(
                "Showing {path} rendered ({}) in the canvas [id={}]. The user can see \
                 it, and the canvas follows the file: editing {path} updates it, so \
                 there is no need to call canvas again.",
                doc.format, doc.id
            ),
            None => format!(
                "Showed canvas \"{}\" ({}) [id={}]. The user can see it; revise it by \
                 calling canvas again with id=\"{}\".",
                doc.title, doc.format, doc.id, doc.id
            ),
        };
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
        build_doc(parsed, None)
    }

    fn tool_in(dir: &std::path::Path) -> (Arc<CapturingSink>, CanvasTool) {
        let sink = Arc::new(CapturingSink(Mutex::new(None)));
        let tool = CanvasTool::new(sink.clone(), Workspace::new(dir).unwrap());
        (sink, tool)
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
        // Content without a format has nothing to render as.
        let err = parse_doc(&serde_json::json!({ "content": "# hi" })).unwrap_err();
        assert!(err.contains("`format`"), "{err}");
    }

    #[tokio::test]
    async fn a_project_file_renders_by_extension_and_follows_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("site")).unwrap();
        std::fs::write(dir.path().join("site/index.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.path().join("notes.md"), "# notes").unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        let (sink, tool) = tool_in(dir.path());

        let out = tool
            .invoke(serde_json::json!({ "path": "./site/index.html" }))
            .await
            .unwrap();
        assert!(
            out.contains("Showing site/index.html rendered (html)"),
            "{out}"
        );
        assert!(out.contains("follows the file"), "{out}");
        let shown = sink.0.lock().unwrap().clone().unwrap();
        assert_eq!(shown.path.as_deref(), Some("site/index.html"));
        assert_eq!(shown.content, "<h1>hi</h1>");
        assert_eq!(shown.format, "html");
        assert_eq!(shown.title, "index.html");
        assert_eq!(shown.id, "site-index-html", "the file names the panel");
        assert_eq!(shown.language, None);

        tool.invoke(serde_json::json!({ "path": "notes.md", "title": "Notes" }))
            .await
            .unwrap();
        let shown = sink.0.lock().unwrap().clone().unwrap();
        assert_eq!(
            (shown.format.as_str(), shown.title.as_str()),
            ("markdown", "Notes")
        );

        tool.invoke(serde_json::json!({ "path": "main.rs" }))
            .await
            .unwrap();
        let shown = sink.0.lock().unwrap().clone().unwrap();
        assert_eq!(shown.format, "code");
        assert_eq!(shown.language.as_deref(), Some("rust"));
        assert_eq!(shown.extension(), "rs");
    }

    #[tokio::test]
    async fn a_path_stays_inside_the_workspace_and_excludes_content() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.html"), "<p>no</p>").unwrap();
        std::fs::write(dir.path().join("page.html"), "<p>yes</p>").unwrap();
        let (sink, tool) = tool_in(dir.path());

        let err = tool
            .invoke(serde_json::json!({
                "path": outside.path().join("secret.html").to_string_lossy()
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("escapes the workspace"), "{err}");
        let err = tool
            .invoke(serde_json::json!({ "path": "missing.html" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("missing.html"), "{err}");
        let err = tool
            .invoke(serde_json::json!({ "path": "page.html", "content": "<p>also</p>" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not both"), "{err}");
        assert!(sink.0.lock().unwrap().is_none(), "nothing was shown");
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
        let (sink, tool) = tool_in(tempfile::tempdir().unwrap().path());
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
        assert_eq!(shown.path, None);
        // A doc without a path serializes as before (older hosts read it).
        let json = serde_json::to_value(&shown).unwrap();
        assert!(json.get("path").is_none());
    }

    #[test]
    fn schema_advertises_format_enum() {
        assert_eq!(CanvasTool::NAME, CANVAS_TOOL);
        let schema = crate::schema_for::<CanvasArgs>();
        // Every format the hosts can render must be advertised in the enum.
        // `format` is optional (inferred from a `path`), so the enum sits in
        // the non-null branch of its `anyOf`.
        let format = &schema["properties"]["format"];
        let formats: Vec<&str> = format["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|branch| branch["enum"].as_array())
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(formats, CANVAS_FORMATS);
        // …and `content` is optional too: `path` is the other way in.
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        assert!(
            !required.contains(&"content") && !required.contains(&"format"),
            "{required:?}"
        );
        assert!(schema["properties"]["path"].is_object());
    }
}
