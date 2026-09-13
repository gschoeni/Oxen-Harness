//! The one staged-Ctrl-C policy, shared by every loop that reads input: the
//! live idle prompt, the mid-turn composer, and the classic readline REPL.
//!
//! Claude-Code-style staging: a first Ctrl-C clears whatever is being typed,
//! a second arms the exit and shows the two ways out — Ctrl-C again leaves,
//! `d` first labels the run as training data — and only that confirmed
//! press actually exits, never a surprise quit mid-thought. The wording of
//! the arm notice and the interrupted-turn block also lives here so the live
//! and classic surfaces can't drift apart.

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

/// The key that, pressed while the exit is armed, opens the training-data
/// picker instead of leaving straight away.
pub(crate) const LABEL_KEY: char = 'd';

/// Tracks whether the next Ctrl-C exits. Arm it via [`ExitGuard::on_ctrl_c`];
/// call [`ExitGuard::disarm`] on any other activity so the confirmation never
/// goes stale. While armed, [`ExitGuard::wants_label`] says whether a plain
/// keystroke is the `d` that asks to label the run first.
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

    pub(crate) fn armed(&self) -> bool {
        self.armed
    }

    /// Whether `key` (an unmodified character press) is the armed-state `d`
    /// that opens the training-data picker.
    pub(crate) fn wants_label(&self, key: char) -> bool {
        self.armed && key == LABEL_KEY
    }
}

/// The acknowledgement shown when a Ctrl-C arms the exit: the two ways out
/// (leave now, or label the run as training data first) and the way back.
/// Fitted to `cols` so the pinned row never wraps into the output above:
/// the "any other key" tail goes first, then the label hint's detail.
pub(crate) fn arm_notice(ui: &Ui, cols: usize) -> String {
    let key = LABEL_KEY.to_string();
    // (segments as (styled, plain-width)) — longest form first.
    let forms: [Vec<(String, &str)>; 3] = [
        vec![
            (ui.accent("ctrl-c"), "ctrl-c"),
            (ui.dim("again leaves the trail ·"), "again leaves the trail ·"),
            (ui.accent(&key), "d"),
            (
                ui.dim("labels this run as training data first ·"),
                "labels this run as training data first ·",
            ),
            (ui.dim("any other key keeps riding"), "any other key keeps riding"),
        ],
        vec![
            (ui.accent("ctrl-c"), "ctrl-c"),
            (ui.dim("again leaves ·"), "again leaves ·"),
            (ui.accent(&key), "d"),
            (
                ui.dim("labels this run as training data"),
                "labels this run as training data",
            ),
        ],
        vec![
            (ui.accent("ctrl-c"), "ctrl-c"),
            (ui.dim("exit ·"), "exit ·"),
            (ui.accent(&key), "d"),
            (ui.dim("label"), "label"),
        ],
    ];
    // "  ⚠ " lead (4 cells) + segments joined by single spaces.
    let fits = |segs: &Vec<(String, &str)>| {
        4 + segs.iter().map(|(_, w)| w.len()).sum::<usize>() + segs.len() - 1 <= cols
    };
    let segs = forms.iter().find(|f| fits(f)).unwrap_or(&forms[2]);
    let body: Vec<&str> = segs.iter().map(|(s, _)| s.as_str()).collect();
    format!("  {} {}", ui.red("⚠"), body.join(" "))
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

    #[test]
    fn arm_notice_fits_the_terminal_width() {
        let ui = Ui::with(false, std::sync::Arc::new(harness_theme::Theme::default()));
        for cols in [40usize, 60, 80, 100, 120] {
            let line = arm_notice(&ui, cols);
            assert!(
                crate::width::str_width(&line) <= cols,
                "{cols} cols: {line:?}"
            );
            assert!(line.contains("ctrl-c") && line.contains(" d "), "{line:?}");
        }
        assert!(arm_notice(&ui, 120).contains("any other key keeps riding"));
        assert!(!arm_notice(&ui, 80).contains("any other key"));
    }

    #[test]
    fn label_key_only_counts_while_armed() {
        let mut guard = ExitGuard::default();
        assert!(!guard.wants_label(LABEL_KEY));
        assert_eq!(guard.on_ctrl_c(false), CtrlC::Arm);
        assert!(guard.armed());
        assert!(guard.wants_label(LABEL_KEY));
        assert!(!guard.wants_label('x'));
        guard.disarm();
        assert!(!guard.wants_label(LABEL_KEY));
    }
}
