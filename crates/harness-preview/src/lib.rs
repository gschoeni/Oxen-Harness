//! Live preview of the app the agent is building.
//!
//! The agent vibe-codes a website, calls `start_dev_server`, and the host shows
//! the running app next to the chat (an embedded webview on desktop, a URL in
//! the CLI). This crate owns the process side of that story:
//!
//! - [`DevServer`] — one long-lived dev-server process: spawned in the
//!   session's workspace, port auto-assigned (exported as `PORT`) or sniffed
//!   from the server's own output, readiness-polled over TCP, logs retained in
//!   a bounded ring buffer, and the whole process *group* killed on stop so
//!   `npm run dev`'s node children don't outlive the shell.
//! - [`DevServerManager`] — at most one server per chat session, addressable
//!   by session id, shared by the tools and the host UI.
//! - [`StartDevServerTool`] / [`StopDevServerTool`] / [`DevServerLogsTool`] —
//!   the model-facing tools.
//! - [`PreviewSink`] — the host trait notified of lifecycle changes
//!   (starting/ready/error/stopped) so a UI can open, update, or close the
//!   preview panel.
//! - [`config`] — remembers what worked in `<workspace>/.oxen-harness/preview.json`
//!   so later sessions can start the same server without re-discovery.

pub mod config;
pub mod console;
mod manager;
mod server;
mod sniff;
mod tools;
mod watch;

pub use console::{ConsoleBridge, ConsoleLine};
pub use manager::DevServerManager;
pub use server::{
    DevServer, PreviewPhase, PreviewSink, PreviewStatus, ServerSpec, DEFAULT_READY_TIMEOUT,
};
pub use sniff::detect_local_url;
pub use tools::{
    session_tools, sight_tools, DevServerLogsTool, PreviewConsoleTool, PreviewLens,
    PreviewScreenshotTool, StartDevServerTool, StopDevServerTool, DEV_SERVER_LOGS_TOOL,
    PREVIEW_CONSOLE_TOOL, PREVIEW_SCREENSHOT_TOOL, START_DEV_SERVER_TOOL, STOP_DEV_SERVER_TOOL,
};
pub use watch::hmr_capable;

/// Errors from starting or supervising a dev server.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    /// The server failed to start or become reachable; the message includes a
    /// tail of its output so the caller (usually the model) can debug.
    #[error("{0}")]
    Server(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
