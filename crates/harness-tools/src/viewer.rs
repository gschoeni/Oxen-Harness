//! `open_file` — put a project file on screen in the user's file viewer panel
//! (the desktop's Editor dock: code with syntax highlighting, images, video).
//!
//! This module is also the REFERENCE for the host-surface tool pattern — a
//! tool whose entire effect is showing something in the host's UI. The shape,
//! shared with `ask` (question picker) and `canvas` (document panel):
//!
//! 1. **A data struct** describing what to show ([`FileView`]) — plain data,
//!    no host types, so it can cross any host boundary (a Tauri event, a
//!    terminal render, a test capture).
//! 2. **A sink trait** the host implements ([`ViewerSink`]). It returns
//!    `Option<String>`: a host note appended to the model-visible result
//!    (e.g. "saved to …"), or `None` when the surface itself is the result.
//! 3. **A [`TypedTool`]** that validates the model's arguments — every path
//!    through the [`Workspace`] sandbox — and forwards clean data to the sink.
//!    The tool result text tells the model what the user can now see, so it
//!    doesn't re-paste the content into chat.
//! 4. **Per-host registration.** Hosts that have the surface register the tool
//!    with their sink (the desktop emits a session-tagged `agent://…` event);
//!    hosts that don't simply DON'T register it — the model is never told
//!    about a panel that doesn't exist. (The CLI has no file panel, so it
//!    skips `open_file`; contrast `canvas`, where the CLI degrades inside its
//!    sink by writing a file and opening the browser.) Inert `Null…` sinks
//!    exist only for settings registries that list definitions without running
//!    anything.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::{sandbox::Workspace, ToolError, TypedTool};

/// The tool name the model calls (and front ends special-case for rendering).
pub const OPEN_FILE_TOOL: &str = "open_file";

/// What the viewer should show: existing project files, workspace-relative.
///
/// `paths` currently holds exactly one entry per call; it is a list so the
/// wire format already matches viewers that can show a set (the desktop's
/// image gallery) without a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileView {
    /// The workspace root the paths are relative to (hosts that need an
    /// absolute path join against this).
    pub root: PathBuf,
    /// Workspace-relative paths, `/`-separated, verified to exist.
    pub paths: Vec<String>,
}

/// A front end that can bring project files up in its viewer.
///
/// Returns an optional host note appended to the model-visible result; `None`
/// when the opened panel is itself the visible outcome. Hosts WITHOUT a viewer
/// surface should not implement a degraded sink — they should simply not
/// register the tool.
#[async_trait]
pub trait ViewerSink: Send + Sync {
    async fn open(&self, view: &FileView) -> Result<Option<String>, ToolError>;
}

/// The model-facing tool that opens a project file in the user's viewer.
pub struct OpenFileTool {
    workspace: Workspace,
    sink: Arc<dyn ViewerSink>,
}

impl OpenFileTool {
    pub fn new(workspace: Workspace, sink: Arc<dyn ViewerSink>) -> Self {
        Self { workspace, sink }
    }
}

/// Arguments to `open_file`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct OpenFileArgs {
    /// Workspace-relative path of the file to show (e.g. "src/main.rs").
    /// The file must exist; write it first if you are creating it.
    pub path: String,
}

#[async_trait]
impl TypedTool for OpenFileTool {
    const NAME: &'static str = OPEN_FILE_TOOL;
    type Args = OpenFileArgs;

    fn description(&self) -> &str {
        "Open a project file in the user's file viewer panel beside the chat \
         (source code with syntax highlighting, or an image/video rendered \
         natively). Use it to put a specific file in front of the user: one \
         you just created or finished editing, or one you are walking them \
         through. This SHOWS the file to the user — it does not return its \
         content (use read_file to read a file yourself), and it is not for \
         standalone documents you are authoring for the chat (use canvas). \
         Don't open every file you touch; open the one or two that matter."
    }

    async fn run(&self, args: OpenFileArgs) -> Result<String, ToolError> {
        let path = args.path.trim();
        if path.is_empty() {
            return Err(ToolError::InvalidArguments(
                "missing non-empty `path`".into(),
            ));
        }
        // Confine to the workspace, then insist on an existing regular file —
        // the viewer shows real project files, it doesn't create them.
        let resolved = self.workspace.resolve(path)?;
        let meta = std::fs::metadata(&resolved).map_err(|_| {
            ToolError::InvalidArguments(format!(
                "{path} does not exist — write the file first, then open it"
            ))
        })?;
        if !meta.is_file() {
            return Err(ToolError::InvalidArguments(format!(
                "{path} is a directory, not a file"
            )));
        }
        // Normalize to the workspace-relative form the viewer keys on (strips
        // any leading "./" the model included).
        let rel = resolved
            .strip_prefix(self.workspace.root())
            .unwrap_or(&resolved)
            .to_string_lossy()
            .replace('\\', "/");

        let view = FileView {
            root: self.workspace.root().to_path_buf(),
            paths: vec![rel.clone()],
        };
        let note = self.sink.open(&view).await?;
        let mut msg = format!(
            "Opened {rel} in the user's file viewer. They are looking at it now — \
             no need to repeat its contents in chat."
        );
        if let Some(note) = note {
            msg.push(' ');
            msg.push_str(&note);
        }
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A sink that records the last view it was asked to open.
    struct CapturingSink(Mutex<Option<FileView>>);

    #[async_trait]
    impl ViewerSink for CapturingSink {
        async fn open(&self, view: &FileView) -> Result<Option<String>, ToolError> {
            *self.0.lock().unwrap() = Some(view.clone());
            Ok(None)
        }
    }

    fn workspace(name: &str) -> (Workspace, PathBuf) {
        let dir = std::env::temp_dir().join(format!("oxen-viewer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        (Workspace::new(&dir).unwrap(), dir)
    }

    #[tokio::test]
    async fn opens_an_existing_file_and_reports_it() {
        let (ws, dir) = workspace("open");
        let sink = Arc::new(CapturingSink(Mutex::new(None)));
        let tool = OpenFileTool::new(ws, sink.clone());
        let out = tool
            .invoke(serde_json::json!({ "path": "./src/main.rs" }))
            .await
            .unwrap();
        assert!(out.contains("Opened src/main.rs"), "out: {out}");
        let view = sink.0.lock().unwrap().clone().unwrap();
        assert_eq!(view.paths, vec!["src/main.rs"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn rejects_missing_files_directories_and_escapes() {
        let (ws, dir) = workspace("reject");
        let sink = Arc::new(CapturingSink(Mutex::new(None)));
        let tool = OpenFileTool::new(ws, sink.clone());
        for args in [
            serde_json::json!({ "path": "src/missing.rs" }),
            serde_json::json!({ "path": "src" }),
            serde_json::json!({ "path": "../outside.txt" }),
            serde_json::json!({ "path": "  " }),
        ] {
            assert!(tool.invoke(args.clone()).await.is_err(), "accepted {args}");
        }
        assert!(
            sink.0.lock().unwrap().is_none(),
            "sink ran for invalid args"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
