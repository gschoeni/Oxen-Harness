//! One module per feature area, each holding the `#[tauri::command]` functions
//! the webview invokes (plus any view structs only those commands return).
//! The mirror image of the CLI's `commands/` — both front ends navigate by
//! feature, not by one giant file.
//!
//! Adding a command touches three synchronized spots:
//!
//! 1. Write the `#[tauri::command]` fn in the submodule that owns its concern
//!    (or add a new submodule here if it's a genuinely new area). Return
//!    `Result<_, String>` so errors surface as rejected promises.
//! 2. List it in the `invoke_handler!` in the crate root's `run()` — Tauri
//!    only routes commands named there.
//! 3. Add the typed wrapper in `app/src/lib/ipc.ts` — components never call
//!    `invoke` directly, so the wrapper is the frontend's entire contract
//!    with your command.
//!
//! Shared machinery stays out of here: agent lifecycle in [`crate::state`],
//! webview payloads in [`crate::events`], host↔agent bridges in
//! [`crate::bridges`] — a command module should read as orchestration.

pub(crate) mod connection;
pub(crate) mod loops;
pub(crate) mod models;
pub(crate) mod project;
pub(crate) mod review;
pub(crate) mod session;
pub(crate) mod skills;
pub(crate) mod theme;
pub(crate) mod tools;
pub(crate) mod turn;
