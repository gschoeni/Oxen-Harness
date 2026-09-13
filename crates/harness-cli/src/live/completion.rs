//! Slash-command and argument completion for the composer.
//!
//! Typing `/` offers the command list; `/model <partial>` and
//! `/compression <partial>` become a small vertical **picker** (matched on
//! both id and display name for models), where ↑/↓ move the highlight, Tab
//! menu-completes and cycles, and Enter folds the visible selection into the
//! submission (see [`Live::accept_completion_on_submit`]).
//!
//! [`Live::accept_completion_on_submit`]: Live#method.accept_completion_on_submit

use crate::repl::{parse_command, ArgCompleter, Command, SLASH_COMMANDS};

use super::layout::Focus;
use super::Live;

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

/// The composer's completion state — `Some` only while there is something to
/// suggest, so the correlated flags it bundles can never drift apart (they
/// used to be four separate fields on [`Live`], reset by hand in three
/// places).
pub(super) struct CompletionState {
    /// Candidate full-line replacements (never empty — empty means `None`).
    items: Vec<CompletionItem>,
    /// Whether the candidates form an argument *picker* (highlighted row, ↑/↓
    /// navigation, Enter runs the selection) rather than plain command-word
    /// completion.
    picker: bool,
    /// The highlighted row: pre-highlighted at 0 for pickers, otherwise set by
    /// the first Tab / ↑ / ↓.
    index: Option<usize>,
    /// Whether the composer text was set by the last Tab (menu-complete), so
    /// the next Tab advances the cycle. Cleared by edits and arrow selection.
    applied: bool,
    /// Whether the user explicitly walked the list (↑/↓/Tab) since the last
    /// edit. A navigated row is accepted by Enter unconditionally; an
    /// auto-highlighted one only under the rules in
    /// [`Live::accept_completion_on_submit`].
    navigated: bool,
}

impl CompletionState {
    fn new(items: Vec<CompletionItem>, picker: bool) -> Self {
        Self {
            items,
            picker,
            index: picker.then_some(0),
            applied: false,
            navigated: false,
        }
    }

    /// The rows currently visible: a centered window (shared with the card
    /// picker's math) capped at 8 picker / 4 command-word rows.
    fn window(&self) -> std::ops::Range<usize> {
        let total = self.items.len();
        let max = if self.picker { 8 } else { 4 };
        crate::picker::centered_window(self.highlight(), total, max)
    }

    /// The highlighted row (0 until navigation touches it), clamped in-range.
    fn highlight(&self) -> usize {
        self.index
            .unwrap_or(0)
            .min(self.items.len().saturating_sub(1))
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
    let text = crate::width::fit(s, max);
    let width = crate::width::str_width(&text);
    FittedText { text, width }
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
            && self.completion.is_some()
    }

    /// Completion picker shown above the box. Model names are long and similar,
    /// so render a small vertical list with provider/details instead of squeezing
    /// unlabeled chips into one hard-to-scan line.
    pub(super) fn completion_hint(&self, box_w: usize) -> Vec<String> {
        let Some(st) = self.completion.as_ref() else {
            return Vec::new();
        };
        let visible = st.window();
        let total = st.items.len();
        let current = st.highlight();
        let mut lines = Vec::new();
        let (title, hint) = if st.picker {
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
            let item = &st.items[i];
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
                let fitted = fit_text(&plain, box_w.saturating_sub(2));
                // Pad by cells, not chars, so the reverse-video band spans the
                // same columns whatever glyphs the model name carries.
                let pad = box_w.saturating_sub(fitted.width);
                lines.push(format!(
                    "  \x1b[7m{}{}\x1b[0m",
                    fitted.text,
                    " ".repeat(pad)
                ));
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

    /// Candidate full-line replacements for the current composer text, plus
    /// whether they form an argument **picker** (highlighted row, ↑/↓
    /// navigation, Enter runs the selection) as opposed to plain command-word
    /// completion. Empty when the text isn't a completable `/command`.
    fn compute_candidates(&mut self) -> (Vec<CompletionItem>, bool) {
        let text = self.composer.text();
        if let Some((start, end, needle)) = at_token(self.composer.chars(), self.composer.cursor())
        {
            let paths = self.path_candidates();
            let items = rank_paths(&paths, &needle)
                .into_iter()
                .map(|path| {
                    let mut line: Vec<char> = self.composer.chars().to_vec();
                    let mut mention: Vec<char> = format!("@{path}").chars().collect();
                    if !path.ends_with('/') {
                        mention.push(' ');
                    }
                    line.splice(start..end, mention);
                    let detail = if path.ends_with('/') {
                        "directory"
                    } else {
                        "file"
                    };
                    CompletionItem::new(line.into_iter().collect::<String>(), path, detail)
                })
                .collect();
            return (items, true);
        }
        if !text.starts_with('/') {
            return (Vec::new(), false);
        }
        let mut parts = text.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        // The argument completer comes from the command registry, so a new
        // command's picker is one `ArgCompleter` entry there — not an arm here.
        let completer = SLASH_COMMANDS
            .iter()
            .find(|spec| spec.matches(cmd))
            .map(|spec| &spec.completer);
        match parts.next() {
            // Still typing the command word — match against the command list,
            // built-ins first, then the workspace's custom commands.
            None => (
                SLASH_COMMANDS
                    .iter()
                    .filter(|spec| spec.name.starts_with(cmd))
                    .map(|spec| CompletionItem::new(spec.name, spec.name, spec.description))
                    .chain(
                        crate::custom_commands::entries()
                            .into_iter()
                            .filter(|(name, _)| name.starts_with(cmd))
                            .map(|(name, description)| {
                                CompletionItem::new(&name, &name, &description)
                            }),
                    )
                    .collect(),
                false,
            ),
            // `/model <partial>` — complete model names (cloud + local). Match on
            // both the API id and the friendly display name so typing "sonnet" or
            // "qwen" narrows to what a human remembers.
            Some(arg) if matches!(completer, Some(ArgCompleter::Models)) => {
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
                        // Price transparency: tag cloud rows with their cached
                        // per-million rate (warmed at startup/turn boundaries;
                        // local rows carry a static "free" instead). Read per
                        // keystroke rather than baked into the cached rows, so
                        // a late-warming cache still shows up. Slots in before
                        // the "← current" marker when one is present.
                        if let Some(rate) = crate::pricing::session_rate(&m.label)
                            .and_then(|r| crate::pricing::format_rate(&r))
                        {
                            match m.detail.find(" ← current") {
                                Some(pos) => m.detail.insert_str(pos, &format!(" · {rate}")),
                                None => m.detail = format!("{} · {rate}", m.detail),
                            }
                        }
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
            // A fixed-choice argument (`/compression <partial>`, `/permissions
            // <partial>`) — pick one of the registry's declared modes. The
            // replacement keeps the alias as typed (`/compress audit`).
            Some(arg) => {
                let Some(ArgCompleter::Static(choices)) = completer else {
                    return (Vec::new(), false);
                };
                let needle = arg.trim().to_lowercase();
                let items = choices
                    .iter()
                    .filter(|(m, _)| m.starts_with(&needle))
                    .map(|(m, desc)| CompletionItem::new(format!("{cmd} {m}"), *m, *desc))
                    .collect();
                (items, true)
            }
        }
    }

    /// Model rows for `/model` completion: the shared catalog + installed-local
    /// rows (see [`crate::commands::model::model_rows`]), loaded once and cached, with
    /// the *persisted* selection marked — the composer's completion list isn't
    /// session-scoped, so it marks what the next launch would ride. That is
    /// always the selected cloud model: a local model only runs on an explicit
    /// `--local`, never from the remembered pick.
    fn model_candidates(&mut self) -> Vec<CompletionItem> {
        if self.model_items.is_none() {
            let selected = harness_runtime::models::selected();
            let items = crate::commands::model::model_rows()
                .into_iter()
                .map(|row| {
                    let current = !row.local && row.id == selected;
                    let marker = if current { " ← current" } else { "" };
                    // Local models run on your own hardware — free, and never
                    // in the endpoint catalog, so the tag is static.
                    let free = if row.local { " · free" } else { "" };
                    CompletionItem::new(
                        row.id.clone(),
                        row.id.clone(),
                        format!("{}{free}{marker}", row.describe()),
                    )
                })
                .collect();
            self.model_items = Some(items);
        }
        self.model_items.clone().unwrap_or_default()
    }

    /// The workspace's paths for `@` completion, rebuilt at most every couple
    /// of seconds so a file the agent just wrote shows up without a restart.
    fn path_candidates(&mut self) -> Vec<String> {
        const REFRESH: std::time::Duration = std::time::Duration::from_secs(2);
        let fresh = self
            .path_items
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < REFRESH);
        if !fresh {
            let root = crate::custom_commands::workspace_root();
            let paths = harness_tools::fs::workspace_paths(&root, MAX_PATH_CANDIDATES);
            self.path_items = Some((std::time::Instant::now(), paths));
        }
        self.path_items
            .as_ref()
            .map(|(_, p)| p.clone())
            .unwrap_or_default()
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
        let Some(st) = self.completion.as_ref() else {
            return;
        };
        let replacement = if st.navigated || (st.picker && has_arg) {
            st.index.and_then(|i| st.items.get(i))
        } else if !st.picker
            && st.items.len() == 1
            && matches!(parse_command(&text), Command::Prompt(_))
        {
            st.items.first()
        } else {
            None
        };
        if let Some(r) = replacement {
            let text = r.replacement.clone();
            self.composer.set_text(&text);
        }
    }

    /// Recompute the completion hint after a compose-buffer change, dropping
    /// any in-progress Tab cycle or ↑/↓ selection (the candidates may have
    /// changed) — a fresh [`CompletionState`], or `None` when there is nothing
    /// to suggest.
    pub(super) fn refresh_completion(&mut self) {
        let (items, picker) = self.compute_candidates();
        self.completion = (!items.is_empty()).then(|| CompletionState::new(items, picker));
    }

    /// Move the highlighted completion row without changing the typed prefix.
    /// Only active for argument pickers; a plain Up/Down with no visible picker
    /// keeps its normal history/queue behavior.
    pub(super) fn move_completion(&mut self, delta: isize) -> bool {
        let Some(st) = self.completion.as_mut() else {
            return false;
        };
        if !st.picker {
            return false;
        }
        st.index = Some(crate::picker::wrap_step(
            st.index.unwrap_or(0),
            delta,
            st.items.len(),
        ));
        st.applied = false;
        st.navigated = true;
        true
    }

    /// Handle Tab: menu-complete the composer to the highlighted matching
    /// candidate, cycling on repeated presses. Returns whether anything changed.
    pub(super) fn complete(&mut self) -> bool {
        if self.completion.is_none() {
            self.refresh_completion();
        }
        let current_text = self.composer.text();
        let Some(st) = self.completion.as_mut() else {
            return false;
        };
        let next = match st.index {
            // Still on the last Tab's pick (no edits or arrowing since) →
            // advance to cycle.
            Some(i) if st.applied && current_text == st.items[i].replacement => {
                (i + 1) % st.items.len()
            }
            _ => st.highlight(),
        };
        let replacement = st.items[next].replacement.clone();
        st.index = Some(next);
        st.applied = true;
        st.navigated = true;
        self.composer.set_text(&replacement);
        true
    }
}

/// State accessors for tests (production code goes through the methods above).
#[cfg(test)]
impl Live {
    pub(super) fn completion_items(&self) -> &[CompletionItem] {
        self.completion
            .as_ref()
            .map(|st| st.items.as_slice())
            .unwrap_or(&[])
    }

    pub(super) fn comp_index(&self) -> Option<usize> {
        self.completion.as_ref().and_then(|st| st.index)
    }

    pub(super) fn comp_picker(&self) -> bool {
        self.completion.as_ref().is_some_and(|st| st.picker)
    }

    pub(super) fn comp_navigated(&self) -> bool {
        self.completion.as_ref().is_some_and(|st| st.navigated)
    }
}

/// The most workspace paths scanned for `@` completion.
const MAX_PATH_CANDIDATES: usize = 20_000;
/// Rows an `@` completion offers.
const MAX_PATH_ROWS: usize = 8;

/// The `@token` the caret is in: `(start, end, needle)` as char indices into
/// `chars`, with the needle being the text after the `@`. `None` when the
/// caret isn't inside a word that starts with `@` (an email-like `a@b` does
/// not count — the `@` must open the word).
pub(super) fn at_token(chars: &[char], cursor: usize) -> Option<(usize, usize, String)> {
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map(|i| i + 1)
        .unwrap_or(0);
    let end = chars[cursor..]
        .iter()
        .position(|c| c.is_whitespace())
        .map(|i| cursor + i)
        .unwrap_or(chars.len());
    if chars.get(start) != Some(&'@') {
        return None;
    }
    let needle: String = chars[start + 1..cursor].iter().collect();
    Some((start, end, needle))
}

/// Rank workspace paths against what was typed after the `@`: a basename
/// prefix beats a path prefix beats a substring beats a subsequence, and
/// shorter paths win ties so `src/lib.rs` outranks `src/legacy/lib.rs`.
pub(super) fn rank_paths(paths: &[String], needle: &str) -> Vec<String> {
    let needle = needle.to_lowercase();
    let mut scored: Vec<(u8, usize, &String)> = paths
        .iter()
        .filter_map(|path| {
            let lower = path.to_lowercase();
            let base = lower.rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
            let score = if needle.is_empty() {
                4
            } else if base.starts_with(&needle) {
                0
            } else if lower.starts_with(&needle) {
                1
            } else if lower.contains(&needle) {
                2
            } else if is_subsequence(&needle, &lower) {
                3
            } else {
                return None;
            };
            Some((score, path.len(), path))
        })
        .collect();
    scored.sort();
    scored
        .into_iter()
        .take(MAX_PATH_ROWS)
        .map(|(_, _, p)| p.clone())
        .collect()
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|n| it.any(|h| h == n))
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::super::keys::KeyAction;
    use super::super::test_support::{key, live};
    use super::super::Live;
    use super::CompletionItem;

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
        assert!(l.completion_items().iter().any(|c| c.label == "/model"));
        assert!(l.completion_items().iter().any(|c| c.label == "/theme"));
        // Narrowing filters the list.
        l.handle_key(key(KeyCode::Char('m')), 0);
        assert_eq!(
            l.completion_items()
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
        assert_eq!(l.comp_index(), None);
        assert!(!l.comp_navigated());
    }

    #[test]
    fn model_completion_filters_by_display_and_arrow_selects() {
        let mut l = live(80, 24);
        // Seed the cached catalog rows — the catalog is user-curated (nothing
        // ships built in), so the test provides what the user would have added.
        l.model_items = Some(vec![
            CompletionItem::new(
                "claude-sonnet-4-6",
                "claude-sonnet-4-6",
                "Claude Sonnet 4.6 · cloud",
            ),
            CompletionItem::new(
                "claude-opus-4-8",
                "claude-opus-4-8",
                "Claude Opus 4.8 · cloud",
            ),
        ]);
        type_line(&mut l, "/model sonnet");
        assert!(l
            .completion_items()
            .iter()
            .any(|c| c.label == "claude-sonnet-4-6"));
        assert_eq!(l.comp_index(), Some(0));
        // Down walks the rows and wraps past the trailing typed-id row back to
        // the first match.
        for _ in 0..l.completion_items().len() {
            l.handle_key(key(KeyCode::Down), 0);
        }
        assert_eq!(l.comp_index(), Some(0));
        l.handle_key(key(KeyCode::Tab), 0);
        assert!(l.composer.text().starts_with("/model "));
        assert!(l.composer.text().contains("claude-sonnet"));
    }

    #[test]
    fn model_completion_tags_rows_with_cached_rates() {
        crate::pricing::seed_for_test(
            "priced-picker-model",
            Some(harness_local::source::ModelPricing {
                input_cost_per_token: 0.000_003,
                output_cost_per_token: 0.000_015,
            }),
        );
        let mut l = live(80, 24);
        l.model_items = Some(vec![
            CompletionItem::new(
                "priced-picker-model",
                "priced-picker-model",
                "Priced · cloud ← current",
            ),
            CompletionItem::new("unpriced-model", "unpriced-model", "Mystery · cloud"),
        ]);
        type_line(&mut l, "/model model");
        let priced = l
            .completion_items()
            .iter()
            .find(|c| c.label == "priced-picker-model")
            .expect("priced row listed");
        // The rate slots in before the current marker; unpriced rows are untouched.
        assert_eq!(
            priced.detail,
            "Priced · cloud · $3/M in · $15/M out ← current"
        );
        let unpriced = l
            .completion_items()
            .iter()
            .find(|c| c.label == "unpriced-model")
            .expect("unpriced row listed");
        assert_eq!(unpriced.detail, "Mystery · cloud");
    }

    #[test]
    fn model_completion_ends_with_an_add_row_that_clears_the_argument() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model ");
        let last = l.completion_items().last().unwrap();
        assert_eq!(last.label, "+ add a new model");
        assert_eq!(last.replacement, "/model ");
        // Arrow up from the top wraps to the add row; Tab picks it, leaving
        // `/model ` ready for a fresh id to be typed.
        l.handle_key(key(KeyCode::Up), 0);
        assert_eq!(l.comp_index(), Some(l.completion_items().len() - 1));
        l.handle_key(key(KeyCode::Tab), 0);
        assert_eq!(l.composer.text(), "/model ");
    }

    #[test]
    fn model_completion_offers_an_unknown_id_as_a_new_model() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model some-brand-new-model");
        // Nothing in the catalog matches, so the typed id itself is offered.
        assert_eq!(l.completion_items().len(), 1);
        assert_eq!(l.completion_items()[0].label, "some-brand-new-model");
        assert!(l.completion_items()[0].detail.contains("new model id"));
        l.handle_key(key(KeyCode::Tab), 0);
        assert_eq!(l.composer.text(), "/model some-brand-new-model");
    }

    #[test]
    fn model_completion_keeps_the_typed_id_reachable_beside_matches() {
        let mut l = live(80, 24);
        // Seed the catalog: it's user-curated (nothing ships built in), so
        // without this the test would depend on the machine's real config.
        l.model_items = Some(vec![CompletionItem::new(
            "claude-sonnet-4-6",
            "claude-sonnet-4-6",
            "Claude Sonnet 4.6 · cloud",
        )]);
        type_line(&mut l, "/model sonnet");
        // Fuzzy matches exist, and the literally-typed id is still offered as
        // an explicit last row — so a new id that happens to be a substring of
        // a catalog entry can always be run as typed.
        let last = l.completion_items().last().unwrap();
        assert_eq!(last.label, "sonnet");
        assert!(last.detail.contains("new model id"));
        assert!(
            l.completion_items().len() > 1,
            "fuzzy matches should also be listed"
        );
    }

    #[test]
    fn an_exactly_typed_model_id_is_hoisted_to_the_highlighted_row() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model claude-sonnet-4-6");
        // The exact id holds the highlighted row even if longer ids also
        // contain it, so Enter runs precisely what was typed.
        assert_eq!(l.completion_items()[0].label, "claude-sonnet-4-6");
        assert_eq!(l.comp_index(), Some(0));
        assert_eq!(submit(&mut l), "/model claude-sonnet-4-6");
    }

    #[test]
    fn a_multi_word_model_argument_gets_no_completion() {
        let mut l = live(80, 24);
        type_line(&mut l, "/model llama 3 8b");
        // Ids never contain whitespace — nothing to complete, and Enter must
        // submit the text exactly as typed (never swallow it into a picker row).
        assert!(l.completion_items().is_empty());
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
        // Seed the catalog: it's user-curated (nothing ships built in), so
        // without this the test would depend on the machine's real config.
        l.model_items = Some(vec![CompletionItem::new(
            "claude-sonnet-4-6",
            "claude-sonnet-4-6",
            "Claude Sonnet 4.6 · cloud",
        )]);
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
        let expected = l.completion_items()[l.comp_index().unwrap()]
            .replacement
            .clone();
        assert_eq!(submit(&mut l), expected);
    }

    #[test]
    fn compression_argument_gets_the_same_picker_navigation() {
        let mut l = live(80, 24);
        type_line(&mut l, "/compression ");
        assert!(l.comp_picker(), "argument completion should be a picker");
        assert_eq!(l.comp_index(), Some(0));
        // ↑/↓ move the highlight exactly like the model picker.
        l.handle_key(key(KeyCode::Down), 0);
        assert_eq!(l.comp_index(), Some(1));
        let expected = l.completion_items()[1].replacement.clone();
        assert_eq!(submit(&mut l), expected);
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn the_at_token_under_the_caret_is_found_and_emails_are_not() {
        let c = chars("look at @src/ma please");
        assert_eq!(at_token(&c, 15), Some((8, 15, "src/ma".into())));
        assert_eq!(at_token(&c, 3), None);
        assert_eq!(at_token(&chars("mail a@b.c"), 8), None);
        assert_eq!(at_token(&chars("@"), 1), Some((0, 1, String::new())));
    }

    #[test]
    fn paths_rank_basename_prefix_first_then_shorter() {
        let paths: Vec<String> = [
            "src/legacy/lib.rs",
            "src/lib.rs",
            "docs/library.md",
            "Cargo.lock",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            rank_paths(&paths, "lib"),
            vec!["src/lib.rs", "docs/library.md", "src/legacy/lib.rs"]
        );
        assert_eq!(rank_paths(&paths, "clk"), vec!["Cargo.lock"]);
        assert!(rank_paths(&paths, "zzz").is_empty());
    }
}
