//! Shared ANSI control sequences.
//!
//! Synchronized output (DEC private mode 2026) asks the terminal to hold a
//! frame until the closing sequence arrives, so a multi-write repaint lands
//! atomically instead of flickering mid-update. Supported by every modern
//! emulator (kitty, iTerm2, WezTerm, Ghostty, Windows Terminal); unsupporting
//! terminals ignore the private mode, so bracketing is always safe.

/// Begin a synchronized-output frame (`DECSET 2026`).
pub(crate) const SYNC_BEGIN: &str = "\x1b[?2026h";

/// End a synchronized-output frame (`DECRST 2026`) — the terminal presents
/// everything written since [`SYNC_BEGIN`] at once.
pub(crate) const SYNC_END: &str = "\x1b[?2026l";
