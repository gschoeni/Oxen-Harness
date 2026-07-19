//! A reusable, Claude-Code-style interactive picker.
//!
//! Renders a single question as a selectable list: arrow keys move, number
//! keys jump, `space` toggles in multi-select, `enter` confirms, and
//! `esc`/`Ctrl-C` cancels. **Typing starts your own answer**: any other
//! printable character drops into the final "✎" row and edits it inline
//! (backspace deletes, `enter` submits, `esc` clears the draft first) — so a
//! prompt that says "type a name below" just works, with no need to discover
//! the row first.
//!
//! Used both by the agent's `ask_user_question` tool ([`crate::ask`]) and by
//! interactive menus (`/model`, `/theme`, `/location`, …), so every
//! option-taking command behaves identically. Split by concern:
//!
//! - [`core`] — the pure question/state model, key reducer, and the list-math
//!   helpers (`wrap_step`, `centered_window`) shared with the composer's
//!   inline completion picker.
//! - [`card`] — the framed-card skin, raw-mode ownership, and the
//!   redraw-in-place block drawing.
//! - [`input`] — the card-framed free-text/masked input prompt (`/auth`, the
//!   Brave key).
//!
//! Selection is blocking, so callers in async contexts should run it on a
//! blocking thread.

mod card;
mod core;
mod input;

pub use self::core::Choice;
pub(crate) use self::core::{centered_window, wrap_step};
pub use card::select;
pub(crate) use input::{card_input, CardInput, CardInputSpec};
