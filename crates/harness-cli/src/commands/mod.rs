//! One module per user-facing command, for both of the CLI's surfaces: the
//! in-REPL `/slash` commands and the `oxen-harness <cmd>` subcommands.
//!
//! The pattern (copy [`loops`] — it has both surfaces):
//!
//! - `pub async fn handle_repl(rest, agent, ui, …)` — the `/command` body.
//!   Parse the raw remainder yourself (so multi-word arguments keep their
//!   spaces) and return `Ok(true)` only when the REPL should exit.
//! - `pub async fn handle_cli(action, ui)` (optional) — the top-level
//!   subcommand body, taking a `clap` action enum defined here.
//!
//! Adding a new `/command` touches three synchronized spots outside this
//! directory, and a test fails if they drift:
//!
//! 1. A `Command` variant + parse arm in [`crate::repl`].
//! 2. A dispatch arm in [`crate::repl_loop`] calling your `handle_repl`.
//! 3. A `SLASH_COMMANDS` entry in [`crate::live`] (Tab completion + hints).
//!
//! Rendering stays out of here where it's shared: streamed turns go through
//! [`crate::render::TurnRenderer`], fleet lanes through [`crate::fleet_ui`],
//! and themed output through [`crate::theme::Ui`] — a command module should
//! read as orchestration, not escape codes.

pub(crate) mod auth;
pub(crate) mod compression;
pub(crate) mod location;
pub(crate) mod loops;
pub(crate) mod model;
pub(crate) mod oxen;
pub(crate) mod queue;
pub(crate) mod review;
pub(crate) mod theme;
pub(crate) mod trace;
