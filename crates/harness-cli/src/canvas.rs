//! CLI side of the `canvas` tool. A terminal can't host a live web view, so we
//! write the document to disk, open web documents (html/svg) in the browser, and
//! render a preview of text documents (markdown/code/mermaid) inline.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use harness_tools::{CanvasDoc, CanvasSink, ToolError};

use crate::markdown::MarkdownStream;
use crate::theme::Ui;

/// Most lines of a document we preview inline before pointing at the saved file.
const PREVIEW_CAP: usize = 40;

/// Where canvas documents are written (`~/.oxen-harness/canvas/`).
fn canvas_dir() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".oxen-harness").join("canvas");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// Open a path in the user's default browser/app (best-effort, non-blocking).
fn open_in_browser(path: &Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(path).spawn();
}

/// The host sink: persists the document and, for web formats, opens it.
pub struct CliCanvasSink;

#[async_trait]
impl CanvasSink for CliCanvasSink {
    async fn show(&self, doc: &CanvasDoc) -> Result<Option<String>, ToolError> {
        let Some(dir) = canvas_dir() else {
            return Ok(Some("(no canvas directory available)".to_string()));
        };
        let path = dir.join(format!("{}.{}", doc.id, doc.extension()));
        std::fs::write(&path, &doc.content).map_err(|e| ToolError::Execution(e.to_string()))?;

        // html/svg are best viewed rendered, so launch the browser.
        let opened = matches!(doc.format.as_str(), "html" | "svg");
        if opened {
            open_in_browser(&path);
        }
        Ok(Some(if opened {
            format!("saved to {} · opened in your browser", path.display())
        } else {
            format!("saved to {}", path.display())
        }))
    }
}

/// Render an inline preview of a `canvas` tool call from its raw JSON arguments,
/// or `None` if the arguments don't parse. Mirrors `diff::render_file_change` so
/// both the classic and live renderers can drop it in place of the generic line.
pub fn render_canvas_block(ui: &Ui, arguments: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("Document");
    let format = v.get("format").and_then(|x| x.as_str()).unwrap_or("markdown");
    let language = v.get("language").and_then(|x| x.as_str());
    let content = v.get("content").and_then(|x| x.as_str()).unwrap_or("");

    let badge = match language {
        Some(l) => format!("({format} · {l})"),
        None => format!("({format})"),
    };
    let mut out = vec![format!(
        "  {} {}  {}",
        ui.green("📄"),
        ui.accent(&format!("Canvas: {title}")),
        ui.dim(&badge),
    )];

    match format {
        // Render markdown through the same renderer the chat uses, into a buffer.
        "markdown" => {
            let mut buf: Vec<u8> = Vec::new();
            {
                let mut md = MarkdownStream::new(ui.clone(), &mut buf);
                md.push(content);
                md.finish();
            }
            let text = String::from_utf8_lossy(&buf).into_owned();
            push_capped(&mut out, text.lines().map(|l| format!("  {l}")));
        }
        // html/svg open in the browser; just note that inline.
        "html" | "svg" => {
            out.push(format!("  {}", ui.dim("→ opening in your browser…")));
        }
        // code / mermaid / anything else: show the source, dimmed.
        _ => {
            push_capped(&mut out, content.lines().map(|l| format!("  {}", ui.dim(l))));
        }
    }
    Some(out)
}

/// Append up to [`PREVIEW_CAP`] lines, then a note for any remainder.
fn push_capped(out: &mut Vec<String>, lines: impl Iterator<Item = String>) {
    let mut extra = 0usize;
    for (i, line) in lines.enumerate() {
        if i < PREVIEW_CAP {
            out.push(line);
        } else {
            extra += 1;
        }
    }
    if extra > 0 {
        out.push(format!(
            "  …(+{extra} more lines — full document saved to the canvas folder)"
        ));
    }
}
