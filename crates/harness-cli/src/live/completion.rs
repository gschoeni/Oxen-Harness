//! Slash-command and argument completion for the composer.
//!
//! Typing `/` offers the command list; `/model <partial>` and
//! `/compression <partial>` become a small vertical **picker** (matched on
//! both id and display name for models), where ↑/↓ move the highlight, Tab
//! menu-completes and cycles, and Enter folds the visible selection into the
//! submission (see [`Live::accept_completion_on_submit`]).
//!
//! [`Live::accept_completion_on_submit`]: Live#method.accept_completion_on_submit

use crate::repl::{parse_command, Command};

use super::layout::Focus;
use super::{Live, SLASH_COMMANDS};

/// One slash-command or argument completion row. `replacement` is the whole line
/// inserted by Tab; `label` is the compact selectable text; `detail` explains why
/// a user might pick it (model display name, provider, current marker, etc.).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CompletionItem {
    pub(super) replacement: String,
    pub(super) label: String,
    pub(super) detail: String,
}

impl CompletionItem {
    fn new(
        replacement: impl Into<String>,
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            replacement: replacement.into(),
            label: label.into(),
            detail: detail.into(),
        }
    }
}

struct FittedText {
    text: String,
    width: usize,
}

impl FittedText {
    fn max_width(mut self, min: usize) -> Self {
        self.width = self.width.max(min);
        self
    }
}

fn fit_text(s: &str, max: usize) -> FittedText {
    let count = s.chars().count();
    if count <= max {
        FittedText {
            text: s.to_string(),
            width: count,
        }
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        FittedText {
            text: format!("{kept}…"),
            width: max,
        }
    }
}

impl Live {
    /// Whether the completion list is currently on screen: composing at idle
    /// (no spinner), composer focused, no inline edit, and candidates exist.
    /// The paint path and Enter's fold both key off this, so Enter can only
    /// ever accept a selection the user can actually see.
    pub(super) fn completion_showing(&self) -> bool {
        self.spinner.is_none()
            && matches!(self.focus, Focus::Composer)
            && self.edit.is_none()
            && !self.completion.is_empty()
    }

    /// Completion picker shown above the box. Model names are long and similar,
    /// so render a small vertical list with provider/details instead of squeezing
    /// unlabeled chips into one hard-to-scan line.
    pub(super) fn completion_hint(&self, box_w: usize) -> Vec<String> {
        let visible = self.visible_completion_rows();
        let total = self.completion.len();
        let current = self.comp_index.unwrap_or(0).min(total.saturating_sub(1));
        let mut lines = Vec::new();
        let (title, hint) = if self.comp_picker {
            let text = self.composer.text();
            let cmd = text
                .split_whitespace()
                .next()
                .unwrap_or("/")
                .trim_start_matches('/')
                .to_string();
            (
                format!("{cmd} picker {}/{}", current + 1, total),
                "tab/↑↓ choose · enter run",
            )
        } else {
            ("completions".to_string(), "tab cycles")
        };
        lines.push(format!(
            "  {} {}  {}",
            self.ui.brown("⇥"),
            self.ui.accent(&title),
            self.ui.dim(hint)
        ));
        if visible.start > 0 {
            lines.push(format!(
                "  {} {}",
                self.ui.brown("│"),
                self.ui.dim(&format!("… {} more above", visible.start))
            ));
        }
        for i in visible.start..visible.end {
            let item = &self.completion[i];
            let selected = i == current;
            let pointer = if selected {
                self.ui.accent("❯")
            } else {
                " ".into()
            };
            let label = fit_text(&item.label, box_w / 2).max_width(8);
            let detail_budget = box_w.saturating_sub(label.width + 8).max(8);
            let detail = fit_text(&item.detail, detail_budget);
            if selected {
                let plain = format!("❯ {} — {}", item.label, item.detail);
                let plain = fit_text(&plain, box_w.saturating_sub(2)).text;
                lines.push(format!("  \x1b[7m{plain:<width$}\x1b[0m", width = box_w));
            } else {
                lines.push(format!(
                    "  {} {} {}",
                    pointer,
                    self.ui.cream(&label.text),
                    self.ui.dim(&detail.text)
                ));
            }
        }
        if visible.end < total {
            lines.push(format!(
                "  {} {}",
                self.ui.brown("│"),
                self.ui
                    .dim(&format!("… {} more below", total - visible.end))
            ));
        }
        lines
    }

    fn visible_completion_rows(&self) -> std::ops::Range<usize> {
        let total = self.completion.len();
        let max = (if self.comp_picker { 8 } else { 4 }).min(total);
        let selected = self.comp_index.unwrap_or(0).min(total.saturating_sub(1));
        let start = selected
            .saturating_sub(max / 2)
            .min(total.saturating_sub(max));
        start..start + max
    }

    /// Candidate full-line replacements for the current composer text, plus
    /// whether they form an argument **picker** (highlighted row, ↑/↓
    /// navigation, Enter runs the selection) as opposed to plain command-word
    /// completion. Empty when the text isn't a completable `/command`.
    fn compute_candidates(&mut self) -> (Vec<CompletionItem>, bool) {
        let text = self.composer.text();
        if !text.starts_with('/') {
            return (Vec::new(), false);
        }
        let mut parts = text.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        match parts.next() {
            // Still typing the command word — match against the command list.
            None => (
                SLASH_COMMANDS
                    .iter()
                    .filter(|(c, _)| c.starts_with(cmd))
                    .map(|(c, desc)| CompletionItem::new(*c, *c, *desc))
                    .collect(),
                false,
            ),
            // `/model <partial>` — complete model names (cloud + local). Match on
            // both the API id and the friendly display name so typing "sonnet" or
            // "qwen" narrows to what a human remembers.
            Some(arg) if cmd == "/model" => {
                let typed = arg.trim();
                // Model ids never contain whitespace: a multi-word argument (or
                // a multi-line draft) isn't completable — it submits as typed.
                if typed.contains(char::is_whitespace) {
                    return (Vec::new(), false);
                }
                let needle = typed.to_lowercase();
                let mut items: Vec<CompletionItem> = self
                    .model_candidates()
                    .into_iter()
                    .filter(|m| {
                        needle.is_empty()
                            || m.label.to_lowercase().contains(&needle)
                            || m.detail.to_lowercase().contains(&needle)
                    })
                    .map(|mut m| {
                        m.replacement = format!("/model {}", m.replacement);
                        m
                    })
                    .collect();
                if typed.is_empty() {
                    // Standing affordance at the bottom of the list: picking it
                    // clears the argument so a fresh id can be typed in.
                    items.push(CompletionItem::new(
                        "/model ",
                        "+ add a new model",
                        "type an id — it's switched to and saved to the catalog",
                    ));
                } else if let Some(exact) = items.iter().position(|m| m.label == typed) {
                    // An exactly-typed id is hoisted to the highlighted row, so
                    // Enter runs *it* — never a longer id it happens to be a
                    // substring of.
                    let row = items.remove(exact);
                    items.insert(0, row);
                } else {
                    // The typed id itself is always runnable — the endpoint may
                    // serve models we don't know about. An explicit row keeps it
                    // reachable (↑ once) even when fuzzy matches exist; picking
                    // it switches to the id and saves it to the catalog (see
                    // `commands::model::handle_repl`).
                    items.push(CompletionItem::new(
                        format!("/model {typed}"),
                        typed,
                        "new model id — switch to it and save to the catalog",
                    ));
                }
                (items, true)
            }
            // `/compression <partial>` — pick one of the three modes.
            Some(arg) if cmd == "/compression" || cmd == "/compress" => {
                let needle = arg.trim().to_lowercase();
                let items = [
                    ("off", "send every tool result untouched"),
                    ("audit", "measure savings, change nothing"),
                    ("on", "compress stale tool output"),
                ]
                .iter()
                .filter(|(m, _)| m.starts_with(&needle))
                .map(|(m, desc)| CompletionItem::new(format!("{cmd} {m}"), *m, *desc))
                .collect();
                (items, true)
            }
            Some(_) => (Vec::new(), false),
        }
    }

    /// Model rows for `/model` completion: the shared catalog + installed-local
    /// rows (see [`crate::commands::model::model_rows`]), loaded once and cached, with
    /// the *persisted* selection marked — the composer's completion list isn't
    /// session-scoped, so it marks what the next launch would ride.
    fn model_candidates(&mut self) -> Vec<CompletionItem> {
        if self.model_items.is_none() {
            let selected = harness_runtime::models::selected();
            let active_local = harness_runtime::models::active_local();
            let items = crate::commands::model::model_rows()
                .into_iter()
                .map(|row| {
                    let current = if row.local {
                        active_local.as_deref() == Some(row.id.as_str())
                    } else {
                        active_local.is_none() && row.id == selected
                    };
                    let marker = if current { " ← current" } else { "" };
                    CompletionItem::new(
                        row.id.clone(),
                        row.id.clone(),
                        format!("{}{marker}", row.describe()),
                    )
                })
                .collect();
            self.model_items = Some(items);
        }
        self.model_items.clone().unwrap_or_default()
    }

    /// On Enter, fold the **visible** completion into the submission so Enter
    /// both completes and runs in one stroke:
    ///
    /// - a row the user navigated to (↑/↓/Tab) always runs;
    /// - an argument picker's auto-highlighted top row runs when an argument
    ///   was typed (`/model sonnet` ↵ runs the highlighted match — an exact id
    ///   is hoisted to that row first, so it can't lose to a longer match);
    /// - a lone command-word candidate completes (`/mo` ↵ runs `/model`) —
    ///   unless the line already parses as a complete command (`/q` quits; it
    ///   is not a prefix request for `/queue`).
    ///
    /// Everything else — an ambiguous prefix, a bare `/model ` (empty argument
    /// means "show me the choices", answered by the interactive picker), or a
    /// list that isn't on screen (mid-turn, queue focus) — submits as typed.
    pub(super) fn accept_completion_on_submit(&mut self) {
        if !self.completion_showing() {
            return;
        }
        let text = self.composer.text();
        let has_arg = text
            .split_once(char::is_whitespace)
            .is_some_and(|(_, arg)| !arg.trim().is_empty());
        let replacement = if self.comp_navigated {
            self.comp_index.and_then(|i| self.completion.get(i))
        } else if self.comp_picker && has_arg {
            self.comp_index.and_then(|i| self.completion.get(i))
        } else if !self.comp_picker
            && self.completion.len() == 1
            && matches!(parse_command(&text), Command::Prompt(_))
        {
            self.completion.first()
        } else {
            None
        };
        if let Some(r) = replacement {
            let text = r.replacement.clone();
            self.composer.set_text(&text);
        }
    }

    /// Recompute the completion hint after a compose-buffer change, and drop any
    /// in-progress Tab cycle or ↑/↓ selection (the candidates may have changed).
    pub(super) fn refresh_completion(&mut self) {
        let (items, picker) = self.compute_candidates();
        self.completion = items;
        self.comp_picker = picker && !self.completion.is_empty();
        self.comp_index = self.comp_picker.then_some(0);
        self.comp_applied = false;
        self.comp_navigated = false;
    }

    /// Move the highlighted completion row without changing the typed prefix.
    /// Only active for argument pickers; a plain Up/Down with no visible picker
    /// keeps its normal history/queue behavior.
    pub(super) fn move_completion(&mut self, delta: isize) -> bool {
        if !self.comp_picker {
            return false;
        }
        let len = self.completion.len() as isize;
        let current = self.comp_index.unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len) as usize;
        self.comp_index = Some(next);
        self.comp_applied = false;
        self.comp_navigated = true;
        true
    }

    /// Handle Tab: menu-complete the composer to the highlighted matching
    /// candidate, cycling on repeated presses. Returns whether anything changed.
    pub(super) fn complete(&mut self) -> bool {
        if self.completion.is_empty() {
            let (items, picker) = self.compute_candidates();
            self.completion = items;
            self.comp_picker = picker && !self.completion.is_empty();
        }
        if self.completion.is_empty() {
            return false;
        }
        let next = match self.comp_index {
            // Still on the last Tab's pick (no edits or arrowing since) →
            // advance to cycle.
            Some(i)
                if self.comp_applied && self.composer.text() == self.completion[i].replacement =>
            {
                (i + 1) % self.completion.len()
            }
            _ => self.comp_index.unwrap_or(0).min(self.completion.len() - 1),
        };
        self.composer.set_text(&self.completion[next].replacement);
        self.comp_index = Some(next);
        self.comp_applied = true;
        self.comp_navigated = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::super::keys::KeyAction;
    use super::super::test_support::{key, live};
    use super::super::Live;

    fn type_line(l: &mut Live, text: &str) {
        for ch in text.chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
    }

    #[test]
    fn typing_a_slash_offers_command_suggestions() {
        let mut l = live(80, 24);
        l.handle_key(key(KeyCode::Char('/')), 0);
        // Every slash command is suggested right after `/`.
        assert!(l.completion.iter().any(|c| c.label == "/model"));
        assert!(l.completion.iter().any(|c| c.label == "/theme"));
        // Narrowing filters the list.
        l.handle_key(key(KeyCode::Char('m')), 0);
        assert_eq!(
            l.completion
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            vec!["/model"]
        );
    }

    #[test]
    fn tab_completes_and_cycles_command_matches() {
        let mut l = live(80, 24);
        type_line(&mut l, "/e");
        // `/export` and `/exit` both match; Tab menu-completes, then cycles.
        l.handle_key(key(KeyCode::Tab), 0);
        let first = l.composer.text();
        l.handle_key(key(KeyCode::Tab), 0);
        let second = l.composer.text();
        assert_ne!(first, second);
        assert!(first.starts_with("/e") && second.starts_with("/e"));
    }

    #[test]
    fn editing_after_complete_drops_the_cycle() {
        let mut l = live(80, 24);
        type_line(&mut l, "/h");
        l.handle_key(key(KeyCode::Tab), 0);
        assert_eq!(l.composer.text(), "/help");
        // A normal edit clears the in-progress cycle selection.
        l.handle_key(key(KeyCode::Char('x')), 0);
        assert_eq!(l.comp_index, None);
        assert!(!l.comp_navigated);
    }

    #[test]
    fn model_completion_filters_by_display_and_arrow_selects() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model sonnet");
        assert!(l.completion.iter().any(|c| c.label == "claude-sonnet-4-6"));
        assert_eq!(l.comp_index, Some(0));
        // Down walks the rows and wraps past the trailing typed-id row back to
        // the first match.
        for _ in 0..l.completion.len() {
            l.handle_key(key(KeyCode::Down), 0);
        }
        assert_eq!(l.comp_index, Some(0));
        l.handle_key(key(KeyCode::Tab), 0);
        assert!(l.composer.text().starts_with("/model "));
        assert!(l.composer.text().contains("claude-sonnet"));
    }

    #[test]
    fn model_completion_ends_with_an_add_row_that_clears_the_argument() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model ");
        let last = l.completion.last().unwrap();
        assert_eq!(last.label, "+ add a new model");
        assert_eq!(last.replacement, "/model ");
        // Arrow up from the top wraps to the add row; Tab picks it, leaving
        // `/model ` ready for a fresh id to be typed.
        l.handle_key(key(KeyCode::Up), 0);
        assert_eq!(l.comp_index, Some(l.completion.len() - 1));
        l.handle_key(key(KeyCode::Tab), 0);
        assert_eq!(l.composer.text(), "/model ");
    }

    #[test]
    fn model_completion_offers_an_unknown_id_as_a_new_model() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model some-brand-new-model");
        // Nothing in the catalog matches, so the typed id itself is offered.
        assert_eq!(l.completion.len(), 1);
        assert_eq!(l.completion[0].label, "some-brand-new-model");
        assert!(l.completion[0].detail.contains("new model id"));
        l.handle_key(key(KeyCode::Tab), 0);
        assert_eq!(l.composer.text(), "/model some-brand-new-model");
    }

    #[test]
    fn model_completion_keeps_the_typed_id_reachable_beside_matches() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model sonnet");
        // Fuzzy matches exist, and the literally-typed id is still offered as
        // an explicit last row — so a new id that happens to be a substring of
        // a catalog entry can always be run as typed.
        let last = l.completion.last().unwrap();
        assert_eq!(last.label, "sonnet");
        assert!(last.detail.contains("new model id"));
        assert!(
            l.completion.len() > 1,
            "fuzzy matches should also be listed"
        );
    }

    #[test]
    fn an_exactly_typed_model_id_is_hoisted_to_the_highlighted_row() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model claude-sonnet-4-6");
        // The exact id holds the highlighted row even if longer ids also
        // contain it, so Enter runs precisely what was typed.
        assert_eq!(l.completion[0].label, "claude-sonnet-4-6");
        assert_eq!(l.comp_index, Some(0));
        assert_eq!(submit(&mut l), "/model claude-sonnet-4-6");
    }

    #[test]
    fn a_multi_word_model_argument_gets_no_completion() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model llama 3 8b");
        // Ids never contain whitespace — nothing to complete, and Enter must
        // submit the text exactly as typed (never swallow it into a picker row).
        assert!(l.completion.is_empty());
        assert_eq!(submit(&mut l), "/model llama 3 8b");
    }

    fn submit(l: &mut Live) -> String {
        match l.handle_key(key(KeyCode::Enter), 0) {
            KeyAction::Submit(text) => text,
            _ => panic!("Enter in the composer should submit"),
        }
    }

    #[test]
    fn enter_completes_an_unambiguous_command_word() {
        let mut l = live(80, 24);
        type_line(&mut l, "/mo");
        // `/model` is the only match, so Enter completes and runs it.
        assert_eq!(submit(&mut l), "/model");
    }

    #[test]
    fn enter_leaves_an_ambiguous_command_word_as_typed() {
        let mut l = live(80, 24);
        type_line(&mut l, "/e");
        // `/export` and `/exit` both match — don't guess.
        assert_eq!(submit(&mut l), "/e");
    }

    #[test]
    fn enter_never_rewrites_a_recognized_command_alias() {
        let mut l = live(80, 24);
        type_line(&mut l, "/q");
        // `/q` is the exit alias (parse_command → Exit); the fact that it is
        // also a unique prefix of `/queue` must not hijack it.
        assert_eq!(submit(&mut l), "/q");
    }

    #[test]
    fn enter_runs_the_highlighted_model_row() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model sonnet");
        // The picker hint says "enter run": Enter submits the highlighted
        // row's full id, not the typed fragment.
        let text = submit(&mut l);
        assert_ne!(text, "/model sonnet");
        assert!(
            text.starts_with("/model ") && text.contains("sonnet"),
            "got `{text}`"
        );
    }

    #[test]
    fn enter_on_a_bare_model_command_submits_as_typed_for_the_picker() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model ");
        // An empty argument isn't a choice — `/model` goes through unchanged
        // so the command opens the interactive picker.
        assert_eq!(submit(&mut l).trim(), "/model");
    }

    #[test]
    fn enter_runs_a_navigated_row_even_with_an_empty_argument() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model ");
        // The hint says "↑↓ choose · enter run" — walking to a row and hitting
        // Enter must run that row, not fall back to the interactive picker.
        l.handle_key(key(KeyCode::Down), 0);
        let expected = l.completion[l.comp_index.unwrap()].replacement.clone();
        assert_eq!(submit(&mut l), expected);
    }

    #[test]
    fn compression_argument_gets_the_same_picker_navigation() {
        let mut l = live(80, 24);
        type_line(&mut l, "/compression ");
        assert!(l.comp_picker, "argument completion should be a picker");
        assert_eq!(l.comp_index, Some(0));
        // ↑/↓ move the highlight exactly like the model picker.
        l.handle_key(key(KeyCode::Down), 0);
        assert_eq!(l.comp_index, Some(1));
        let expected = l.completion[1].replacement.clone();
        assert_eq!(submit(&mut l), expected);
    }
}
