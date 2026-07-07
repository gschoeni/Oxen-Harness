//! Slash-command and argument completion for the composer.
//!
//! Typing `/` offers the command list; `/model <partial>` becomes a small
//! vertical picker over the cloud catalog + installed local models (matched on
//! both id and display name), and `/compression <partial>` completes the three
//! modes. Tab menu-completes and cycles, ↑/↓ move the picker highlight, and
//! Enter folds the visible selection into the submission (see
//! [`Live::accept_completion_on_submit`]).

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
    /// Completion picker shown above the box. Model names are long and similar,
    /// so render a small vertical list with provider/details instead of squeezing
    /// unlabeled chips into one hard-to-scan line.
    pub(super) fn completion_hint(&self, box_w: usize) -> Vec<String> {
        let visible = self.visible_completion_rows();
        let total = self.completion.len();
        let current = self.comp_index.unwrap_or(0).min(total.saturating_sub(1));
        let mut lines = Vec::new();
        let title = if self.is_model_completion() {
            format!("model picker {}/{}", current + 1, total)
        } else {
            "completions".to_string()
        };
        let hint = if self.is_model_completion() {
            "tab/↑↓ choose · enter run"
        } else {
            "tab cycles"
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

    fn is_model_completion(&self) -> bool {
        self.composer.text().starts_with("/model")
    }

    fn visible_completion_rows(&self) -> std::ops::Range<usize> {
        let total = self.completion.len();
        let max = (if self.is_model_completion() { 8 } else { 4 }).min(total);
        let selected = self.comp_index.unwrap_or(0).min(total.saturating_sub(1));
        let start = selected
            .saturating_sub(max / 2)
            .min(total.saturating_sub(max));
        start..start + max
    }

    /// Candidate full-line replacements for the current composer text: slash
    /// commands while typing the command word, or model names after `/model `.
    /// Empty when the text isn't a completable `/command`.
    fn compute_candidates(&mut self) -> Vec<CompletionItem> {
        let text = self.composer.text();
        if !text.starts_with('/') {
            return Vec::new();
        }
        let mut parts = text.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        match parts.next() {
            // Still typing the command word — match against the command list.
            None => SLASH_COMMANDS
                .iter()
                .filter(|(c, _)| c.starts_with(cmd))
                .map(|(c, desc)| CompletionItem::new(*c, *c, *desc))
                .collect(),
            // `/model <partial>` — complete model names (cloud + local). Match on
            // both the API id and the friendly display name so typing "sonnet" or
            // "qwen" narrows to what a human remembers.
            Some(arg) if cmd == "/model" => {
                let typed = arg.trim();
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
                // A typed id that matches nothing is still runnable — the
                // endpoint may serve models we don't know about. Offer it as
                // the picker's only row so entering a brand-new model id is a
                // first-class path: Enter switches to it and saves it to the
                // catalog (see `Command::Model` in main.rs).
                if items.is_empty() && !typed.is_empty() && !typed.contains(char::is_whitespace) {
                    items.push(CompletionItem::new(
                        format!("/model {typed}"),
                        typed,
                        "new model id — switch to it and save to the catalog",
                    ));
                } else {
                    // Standing affordance at the bottom of the list: picking it
                    // clears the argument so a fresh id can be typed in.
                    items.push(CompletionItem::new(
                        "/model ",
                        "+ add a new model",
                        "type an id — it's switched to and saved to the catalog",
                    ));
                }
                items
            }
            // `/compression <partial>` — complete the three modes.
            Some(arg) if cmd == "/compression" || cmd == "/compress" => {
                let needle = arg.trim().to_lowercase();
                [
                    ("off", "send every tool result untouched"),
                    ("audit", "measure savings, change nothing"),
                    ("on", "compress stale tool output"),
                ]
                .iter()
                .filter(|(m, _)| m.starts_with(&needle))
                .map(|(m, desc)| CompletionItem::new(format!("{cmd} {m}"), *m, *desc))
                .collect()
            }
            Some(_) => Vec::new(),
        }
    }

    /// Model rows for `/model` completion: cloud catalog plus installed local
    /// models, loaded once and cached with display names and current markers.
    fn model_candidates(&mut self) -> Vec<CompletionItem> {
        if self.model_items.is_none() {
            let selected = harness_runtime::models::selected();
            let active_local = harness_runtime::models::active_local();
            let mut items: Vec<CompletionItem> = harness_runtime::models::catalog()
                .into_iter()
                .map(|m| {
                    let marker = if active_local.is_none() && m.id == selected {
                        " ← current"
                    } else {
                        ""
                    };
                    let source = if m.builtin {
                        "cloud built-in"
                    } else {
                        "cloud custom"
                    };
                    CompletionItem::new(
                        m.id.clone(),
                        m.id,
                        format!("{} · {source}{marker}", m.name),
                    )
                })
                .collect();
            if let Ok(store) = harness_local::ModelStore::open() {
                items.extend(store.installed().into_iter().map(|m| {
                    let marker = if active_local.as_deref() == Some(m.id.as_str()) {
                        " ← current"
                    } else {
                        ""
                    };
                    let meta = [m.params.as_str(), m.quant.as_str()]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    let meta = if meta.is_empty() {
                        "local".to_string()
                    } else {
                        format!("local · {meta}")
                    };
                    CompletionItem::new(
                        m.id.clone(),
                        m.id,
                        format!("{} · {meta}{marker}", m.display),
                    )
                }));
            }
            items.sort_by_key(|item| item.label.to_lowercase());
            items.dedup_by(|a, b| a.label == b.label);
            self.model_items = Some(items);
        }
        self.model_items.clone().unwrap_or_default()
    }

    /// On Enter, fold the visible completion into the submission so Enter both
    /// completes and runs in one stroke: the highlighted row of an argument
    /// picker (its hint says "enter run"), or the single remaining candidate of
    /// a command word (`/mo` ↵ runs `/model`). An ambiguous prefix (`/e` could
    /// be `/export` or `/exit`) is left as typed, and so is a bare `/model ` —
    /// an empty argument means "show me the choices", which the command itself
    /// answers with the interactive picker, not whichever row sorts first.
    pub(super) fn accept_completion_on_submit(&mut self) {
        let text = self.composer.text();
        let has_arg = text
            .split_once(char::is_whitespace)
            .is_some_and(|(_, arg)| !arg.trim().is_empty());
        let replacement = match self.comp_index {
            Some(i) if has_arg => self.completion.get(i),
            Some(_) => None,
            None if self.completion.len() == 1 => self.completion.first(),
            None => None,
        };
        if let Some(r) = replacement {
            let text = r.replacement.clone();
            self.composer.set_text(&text);
        }
    }

    /// Recompute the completion hint after a compose-buffer change, and drop any
    /// in-progress Tab cycle (the candidates may have changed).
    pub(super) fn refresh_completion(&mut self) {
        self.completion = self.compute_candidates();
        self.comp_index = (!self.completion.is_empty() && self.is_model_completion()).then_some(0);
        self.comp_applied = false;
    }

    /// Move the highlighted completion row without changing the typed prefix.
    /// Only active for argument pickers; a plain Up/Down with no visible picker
    /// keeps its normal history/queue behavior.
    pub(super) fn move_completion(&mut self, delta: isize) -> bool {
        if self.completion.is_empty() || !self.is_model_completion() {
            return false;
        }
        let len = self.completion.len() as isize;
        let current = self.comp_index.unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len) as usize;
        self.comp_index = Some(next);
        self.comp_applied = false;
        true
    }

    /// Handle Tab: menu-complete the composer to the highlighted matching
    /// candidate, cycling on repeated presses. Returns whether anything changed.
    pub(super) fn complete(&mut self) -> bool {
        if self.completion.is_empty() {
            self.completion = self.compute_candidates();
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
        true
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::super::keys::KeyAction;
    use super::super::test_support::{key, live};
    use super::super::Live;

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
        for ch in "/e".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
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
        l.handle_key(key(KeyCode::Char('/')), 0);
        l.handle_key(key(KeyCode::Char('h')), 0);
        l.handle_key(key(KeyCode::Tab), 0);
        assert_eq!(l.composer.text(), "/help");
        // A normal edit clears the in-progress cycle selection.
        l.handle_key(key(KeyCode::Char('x')), 0);
        assert_eq!(l.comp_index, None);
    }

    #[test]
    fn model_completion_filters_by_display_and_arrow_selects() {
        let mut l = live(80, 24);
        for ch in "/model sonnet".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        assert!(l.completion.iter().any(|c| c.label == "claude-sonnet-4-6"));
        assert_eq!(l.comp_index, Some(0));
        // Down walks the rows and wraps past the trailing "add" row back to
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
        for ch in "/model ".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
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
        for ch in "/model some-brand-new-model".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        // Nothing in the catalog matches, so the typed id itself is offered.
        assert_eq!(l.completion.len(), 1);
        assert_eq!(l.completion[0].label, "some-brand-new-model");
        assert!(l.completion[0].detail.contains("new model id"));
        l.handle_key(key(KeyCode::Tab), 0);
        assert_eq!(l.composer.text(), "/model some-brand-new-model");
    }

    #[test]
    fn model_completion_does_not_offer_a_new_row_while_matches_exist() {
        let mut l = live(80, 24);
        for ch in "/model sonnet".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        assert!(l
            .completion
            .iter()
            .all(|c| !c.detail.contains("new model id")));
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
        for ch in "/mo".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        // `/model` is the only match, so Enter completes and runs it.
        assert_eq!(submit(&mut l), "/model");
    }

    #[test]
    fn enter_leaves_an_ambiguous_command_word_as_typed() {
        let mut l = live(80, 24);
        for ch in "/e".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        // `/export` and `/exit` both match — don't guess.
        assert_eq!(submit(&mut l), "/e");
    }

    #[test]
    fn enter_runs_the_highlighted_model_row() {
        let mut l = live(80, 24);
        for ch in "/model sonnet".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        // The picker hint says "enter run": Enter submits the highlighted
        // row's full id, not the typed fragment.
        let text = submit(&mut l);
        assert!(text.starts_with("/model claude-sonnet"), "got `{text}`");
    }

    #[test]
    fn enter_on_a_bare_model_command_submits_as_typed_for_the_picker() {
        let mut l = live(80, 24);
        for ch in "/model ".chars() {
            l.handle_key(key(KeyCode::Char(ch)), 0);
        }
        // An empty argument isn't a choice — `/model` goes through unchanged
        // so the command opens the interactive picker.
        assert_eq!(submit(&mut l).trim(), "/model");
    }
}
