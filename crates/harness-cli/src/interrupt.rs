//! The one staged-Ctrl-C policy, shared by every loop that reads input: the
//! live idle prompt, the mid-turn composer, and the classic readline REPL.
//!
//! Claude-Code-style staging: a first Ctrl-C clears whatever is being typed,
//! a second warns that one more leaves, and only a confirmed third actually
//! exits — never a surprise quit mid-thought. The wording of the arm notice
//! and the interrupted-turn block also lives here so the live and classic
//! surfaces can't drift apart.

use crate::theme::Ui;

/// What one Ctrl-C should do, given whether a draft (or in-progress queue
/// edit) exists and whether a previous press already armed the exit.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CtrlC {
    /// There's a draft — wipe it, stay.
    ClearDraft,
    /// Nothing to clear and not yet armed — warn that another Ctrl-C exits.
    Arm,
    /// Already armed — leave the session.
    Exit,
}

/// Tracks whether the next Ctrl-C exits. Arm it via [`ExitGuard::on_ctrl_c`];
/// call [`ExitGuard::disarm`] on any other activity so the confirmation never
/// goes stale.
#[derive(Default)]
pub(crate) struct ExitGuard {
    armed: bool,
}

impl ExitGuard {
    pub(crate) fn on_ctrl_c(&mut self, has_draft: bool) -> CtrlC {
        if has_draft {
            CtrlC::ClearDraft
        } else if self.armed {
            CtrlC::Exit
        } else {
            self.armed = true;
            CtrlC::Arm
        }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

/// The warning shown when a Ctrl-C arms the exit.
pub(crate) fn arm_notice(ui: &Ui) -> String {
    format!(
        "  {} {}",
        ui.red("⚠"),
        ui.dim("press ctrl-c again to leave the trail — any other key keeps riding"),
    )
}

/// The block printed when a running turn is interrupted: what happened, and
/// how to pick the trail back up. `mid_turn` is whether the transcript now
/// ends mid-turn (so `/retry` can continue it).
pub(crate) fn interrupted_lines(ui: &Ui, mid_turn: bool) -> Vec<String> {
    vec![
        format!(
            "  {} {}",
            ui.red("⚠ interrupted"),
            ui.dim("— the oxen pull up short"),
        ),
        format!(
            "  {}",
            ui.dim(if mid_turn {
                "every step so far is saved · /retry continues this turn, or just give new directions"
            } else {
                "every step so far is saved · give new directions whenever you're ready"
            })
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_stages_clear_then_arm_then_exit() {
        let mut guard = ExitGuard::default();
        // Something typed: the first Ctrl-C only clears it (and doesn't arm).
        assert_eq!(guard.on_ctrl_c(true), CtrlC::ClearDraft);
        // Nothing typed: warn first, exit only on the confirmed second press.
        assert_eq!(guard.on_ctrl_c(false), CtrlC::Arm);
        assert_eq!(guard.on_ctrl_c(false), CtrlC::Exit);
        // A draft mid-confirmation still only clears; any activity disarms.
        let mut guard = ExitGuard::default();
        assert_eq!(guard.on_ctrl_c(false), CtrlC::Arm);
        assert_eq!(guard.on_ctrl_c(true), CtrlC::ClearDraft);
        guard.disarm();
        assert_eq!(guard.on_ctrl_c(false), CtrlC::Arm);
    }
}
