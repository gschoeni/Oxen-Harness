//! A small streaming Markdown renderer for the REPL.
//!
//! Assistant text arrives token-by-token, so we buffer until a line is complete
//! and then render that line — headings, lists, blockquotes, rules, fenced code
//! blocks, and inline spans (bold, italic, `code`, links) — using the Oregon
//! Trail palette. Rendering at line granularity keeps output live while still
//! letting block- and inline-level Markdown render correctly.
//!
//! GFM tables are the exception to line-at-a-time rendering: their rows are
//! buffered (with one line of lookahead to spot the `|---|` delimiter) so we can
//! measure each column and draw an aligned, box-drawn grid once the table ends.
//!
//! The inline grammar is deliberately small and forgiving: `**bold**`,
//! `*italic*`, `` `code` ``, `[text](url)`, and `\`-escapes. Underscores are
//! left literal so identifiers like `my_var` are never mangled. Anything
//! unmatched falls back to plain text.

use std::io::Write;

use crate::theme::Ui;

/// Column alignment parsed from a GFM table's delimiter row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// A buffered GFM table awaiting enough rows to compute column widths.
struct Table {
    aligns: Vec<Align>,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// Incrementally renders streamed Markdown to a writer.
pub struct MarkdownStream<W: Write> {
    ui: Ui,
    out: W,
    line: String,
    in_code: bool,
    /// The active code block's syntax highlighter — `Some` only inside a fence
    /// whose language we recognize (and color is on); lines fall back to the
    /// theme's flat code color otherwise.
    hl: Option<crate::highlight::Highlighter>,
    /// A line containing `|` held back until we see whether the next line is a
    /// table delimiter row (e.g. `|---|---|`). GFM tables are only confirmed by
    /// that delimiter, so we need one line of lookahead.
    pending_row: Option<String>,
    /// The table currently being accumulated, flushed when it ends.
    table: Option<Table>,
}

impl<W: Write> MarkdownStream<W> {
    pub fn new(ui: Ui, out: W) -> Self {
        Self {
            ui,
            out,
            line: String::new(),
            in_code: false,
            hl: None,
            pending_row: None,
            table: None,
        }
    }

    /// Feed a chunk of streamed text; complete lines render immediately.
    pub fn push(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.line);
                self.render_line(&line);
            } else {
                self.line.push(ch);
            }
        }
        let _ = self.out.flush();
    }

    /// Flush any buffered partial line and close an unterminated code block.
    pub fn finish(&mut self) {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.render_line(&line);
        }
        // A header candidate that never got its delimiter row is just text.
        if let Some(header) = self.pending_row.take() {
            let _ = writeln!(self.out, "{}", render_block_line(&self.ui, &header));
        }
        self.flush_table();
        if self.in_code {
            let _ = writeln!(self.out, "{}", code_close_rule(&self.ui));
            self.in_code = false;
            self.hl = None;
        }
        let _ = self.out.flush();
    }

    fn render_line(&mut self, line: &str) {
        let trimmed = line.trim_start();

        // Fenced code block boundaries (flush any open table first).
        if let Some(after_fence) = trimmed.strip_prefix("```") {
            self.flush_pending_text();
            self.flush_table();
            if self.in_code {
                let _ = writeln!(self.out, "{}", code_close_rule(&self.ui));
                self.in_code = false;
                self.hl = None;
            } else {
                let lang = after_fence.trim();
                let label = if lang.is_empty() { "code" } else { lang };
                let _ = writeln!(self.out, "{}", code_open_rule(&self.ui, label));
                self.in_code = true;
                self.hl = self
                    .ui
                    .colored()
                    .then(|| crate::highlight::Highlighter::for_lang(lang))
                    .flatten();
            }
            return;
        }

        if self.in_code {
            // Code is shown verbatim (no inline parsing) and flush-left, with no
            // gutter or indent, so a terminal selection pastes back cleanly.
            // Syntax-highlighted when the fence named a known language,
            // flat code color otherwise.
            let rendered = self
                .hl
                .as_mut()
                .and_then(|h| h.line(line))
                .unwrap_or_else(|| self.ui.code(line));
            let _ = writeln!(self.out, "{rendered}");
            return;
        }

        // --- Table state machine -------------------------------------------
        // Inside a table: keep collecting rows until a non-table line ends it.
        if self.table.is_some() {
            if is_table_row(trimmed) {
                let cells = parse_row(line);
                self.table.as_mut().unwrap().rows.push(cells);
                return;
            }
            self.flush_table();
            // Fall through to render the current (non-table) line normally.
        }

        // A header candidate is pending: a delimiter row confirms the table.
        if let Some(header) = self.pending_row.take() {
            if is_delimiter_row(trimmed) {
                self.table = Some(Table {
                    aligns: parse_delimiter(trimmed),
                    header: parse_row(&header),
                    rows: Vec::new(),
                });
                return;
            }
            // Not a table after all — render the held line, then continue.
            let _ = writeln!(self.out, "{}", render_block_line(&self.ui, &header));
        }

        // A fresh line containing `|` might start a table — hold it back.
        if is_table_row(trimmed) {
            self.pending_row = Some(line.to_string());
            return;
        }

        let _ = writeln!(self.out, "{}", render_block_line(&self.ui, line));
    }

    /// Render a pending header candidate as plain text (it wasn't a table).
    fn flush_pending_text(&mut self) {
        if let Some(header) = self.pending_row.take() {
            let _ = writeln!(self.out, "{}", render_block_line(&self.ui, &header));
        }
    }

    /// Render and clear the buffered table, if any.
    fn flush_table(&mut self) {
        if let Some(table) = self.table.take() {
            let _ = write!(self.out, "{}", render_table(&self.ui, &table));
        }
    }
}

/// The rule above a code block, carrying the language label: `── rust ───────`.
/// Code lines themselves render flush-left with no gutter (see `render_line`),
/// so the frame lives entirely above/below the code and a selection of the code
/// pastes back clean.
fn code_open_rule(ui: &Ui, label: &str) -> String {
    format!(
        "{} {} {}",
        ui.brown("──"),
        ui.dim(label),
        ui.brown("──────────")
    )
}

/// The rule closing a code block — same weight as the opening rule, no corners.
fn code_close_rule(ui: &Ui) -> String {
    ui.brown("──────────────")
}

/// Render a single, complete non-code line of Markdown.
fn render_block_line(ui: &Ui, line: &str) -> String {
    let indent: String = line.chars().take_while(|c| *c == ' ').collect();
    let trimmed = &line[indent.len()..];

    // Horizontal rule: a line of only -, *, or _ (3+).
    let is_rule = trimmed.len() >= 3
        && (trimmed.chars().all(|c| c == '-')
            || trimmed.chars().all(|c| c == '*')
            || trimmed.chars().all(|c| c == '_'));
    if is_rule {
        return ui.brown("────────────────────────────────────────");
    }

    // ATX headings (#..######).
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
        let text = trimmed[hashes + 1..].trim_start();
        return format!("{indent}{}", ui.title(&render_inline(ui, text)));
    }

    // Blockquote. A dim `>` rather than a box-drawing bar, so a copied quote is
    // still valid Markdown instead of picking up `│` characters.
    if let Some(rest) = trimmed
        .strip_prefix("> ")
        .or_else(|| trimmed.strip_prefix(">"))
    {
        return format!(
            "{indent}{} {}",
            ui.brown(">"),
            ui.dim(&render_inline(ui, rest))
        );
    }

    // Unordered list item.
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return format!("{indent}{} {}", ui.accent("•"), render_inline(ui, rest));
        }
    }

    // Ordered list item: digits then `. ` or `) `.
    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let after = &trimmed[digits..];
        if let Some(rest) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
        {
            let num = &trimmed[..digits];
            return format!(
                "{indent}{} {}",
                ui.accent(&format!("{num}.")),
                render_inline(ui, rest)
            );
        }
    }

    format!("{indent}{}", render_inline(ui, trimmed))
}

// ===========================================================================
// GFM tables
// ===========================================================================

/// A line that could be part of a table: contains an unescaped `|`.
fn is_table_row(trimmed: &str) -> bool {
    let mut prev = '\0';
    for c in trimmed.chars() {
        if c == '|' && prev != '\\' {
            return true;
        }
        prev = c;
    }
    false
}

/// A GFM delimiter row: every cell is `-`/`:` only (e.g. `|:--|--:|:-:|`).
fn is_delimiter_row(trimmed: &str) -> bool {
    let cells = parse_row(trimmed);
    if cells.is_empty() {
        return false;
    }
    cells.iter().all(|cell| {
        let c = cell.trim();
        !c.is_empty()
            && c.chars().all(|ch| ch == '-' || ch == ':')
            && c.contains('-')
            && c.matches(':').count() <= 2
    })
}

/// Split a table row into trimmed cells, honoring `\|` escapes and stripping
/// the optional leading/trailing pipes.
fn parse_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let mut inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    inner = inner.strip_suffix('|').unwrap_or(inner);

    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut prev = '\0';
    for c in inner.chars() {
        if c == '|' && prev != '\\' {
            cells.push(cur.trim().to_string());
            cur = String::new();
        } else if c == '|' && prev == '\\' {
            // Replace the escape with a literal pipe.
            cur.pop();
            cur.push('|');
        } else {
            cur.push(c);
        }
        prev = c;
    }
    cells.push(cur.trim().to_string());
    cells
}

/// Parse alignments from a delimiter row's cells.
fn parse_delimiter(trimmed: &str) -> Vec<Align> {
    parse_row(trimmed)
        .iter()
        .map(|cell| {
            let c = cell.trim();
            let left = c.starts_with(':');
            let right = c.ends_with(':');
            match (left, right) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            }
        })
        .collect()
}

/// A row's cell at column `c`, or `""` when the row is short.
fn cell(row: &[String], c: usize) -> &str {
    row.get(c).map(|s| s.as_str()).unwrap_or("")
}

/// Render a buffered table as an aligned, box-drawn grid (Oregon Trail palette).
fn render_table(ui: &Ui, table: &Table) -> String {
    let ncols = table
        .header
        .len()
        .max(table.rows.iter().map(|r| r.len()).max().unwrap_or(0))
        .max(table.aligns.len())
        .max(1);

    let align = |c: usize| table.aligns.get(c).copied().unwrap_or(Align::Left);

    // Pre-render every cell and measure its visible width.
    let header_rendered: Vec<(String, usize)> = (0..ncols)
        .map(|c| {
            let styled = ui.strong(&render_inline(ui, cell(&table.header, c)));
            let w = display_width(&styled);
            (styled, w)
        })
        .collect();
    let rows_rendered: Vec<Vec<(String, usize)>> = table
        .rows
        .iter()
        .map(|row| {
            (0..ncols)
                .map(|c| {
                    let styled = render_inline(ui, cell(row, c));
                    let w = display_width(&styled);
                    (styled, w)
                })
                .collect()
        })
        .collect();

    // Column widths: widest visible cell in each column (min 1).
    let mut widths = vec![1usize; ncols];
    for c in 0..ncols {
        widths[c] = widths[c].max(header_rendered[c].1);
        for row in &rows_rendered {
            widths[c] = widths[c].max(row[c].1);
        }
    }

    let border = |left: &str, mid: &str, right: &str| -> String {
        let mut s = String::from(left);
        for (c, w) in widths.iter().enumerate() {
            s.push_str(&"─".repeat(w + 2));
            s.push_str(if c + 1 == ncols { right } else { mid });
        }
        format!("  {}\n", ui.brown(&s))
    };

    let data_row = |cells: &[(String, usize)]| -> String {
        let mut s = format!("  {}", ui.brown("│"));
        for (c, (styled, vw)) in cells.iter().enumerate() {
            let pad = widths[c].saturating_sub(*vw);
            let body = match align(c) {
                Align::Left => format!("{styled}{}", " ".repeat(pad)),
                Align::Right => format!("{}{styled}", " ".repeat(pad)),
                Align::Center => {
                    let l = pad / 2;
                    format!("{}{styled}{}", " ".repeat(l), " ".repeat(pad - l))
                }
            };
            s.push_str(&format!(" {body} {}", ui.brown("│")));
        }
        s.push('\n');
        s
    };

    let mut out = String::new();
    out.push_str(&border("┌", "┬", "┐"));
    out.push_str(&data_row(&header_rendered));
    out.push_str(&border("├", "┼", "┤"));
    for row in &rows_rendered {
        out.push_str(&data_row(row));
    }
    out.push_str(&border("└", "┴", "┘"));
    out
}

/// Count visible columns in a string, skipping ANSI CSI escape sequences so
/// styled cells pad to the correct width. Cell-based (CJK/emoji span two).
fn display_width(s: &str) -> usize {
    crate::width::display_width(s)
}

/// Render inline Markdown spans within a line.
fn render_inline(ui: &Ui, s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;

    while i < n {
        let c = chars[i];
        match c {
            // Backslash escape: emit the next char literally.
            '\\' if i + 1 < n => {
                out.push(chars[i + 1]);
                i += 2;
            }
            // Inline code: `code` (no nested formatting).
            '`' => {
                if let Some(close) = find(&chars, '`', i + 1) {
                    let span: String = chars[i + 1..close].iter().collect();
                    out.push_str(&ui.code(&span));
                    i = close + 1;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            // Bold (**) or italic (*).
            '*' => {
                if i + 1 < n && chars[i + 1] == '*' {
                    if let Some(close) = find_pair(&chars, '*', i + 2) {
                        let span: String = chars[i + 2..close].iter().collect();
                        out.push_str(&ui.strong(&render_inline(ui, &span)));
                        i = close + 2;
                        continue;
                    }
                    out.push(c);
                    i += 1;
                } else if let Some(close) = find(&chars, '*', i + 1) {
                    let span: String = chars[i + 1..close].iter().collect();
                    out.push_str(&ui.em(&span));
                    i = close + 1;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            // Link: [text](url).
            '[' => {
                if let Some(rendered) = parse_link(ui, &chars, i) {
                    out.push_str(&rendered.0);
                    i = rendered.1;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Find the next occurrence of `target` at or after `from`.
fn find(chars: &[char], target: char, from: usize) -> Option<usize> {
    (from..chars.len()).find(|&j| chars[j] == target)
}

/// Find the next `cc` pair (e.g. `**`) at or after `from`.
fn find_pair(chars: &[char], target: char, from: usize) -> Option<usize> {
    let mut j = from;
    while j + 1 < chars.len() {
        if chars[j] == target && chars[j + 1] == target {
            return Some(j);
        }
        j += 1;
    }
    None
}

/// Parse `[text](url)` starting at `start` (`chars[start] == '['`).
/// Returns the rendered link and the index just past the closing `)`.
fn parse_link(ui: &Ui, chars: &[char], start: usize) -> Option<(String, usize)> {
    let close_bracket = find(chars, ']', start + 1)?;
    if chars.get(close_bracket + 1) != Some(&'(') {
        return None;
    }
    let close_paren = find(chars, ')', close_bracket + 2)?;
    let text: String = chars[start + 1..close_bracket].iter().collect();
    let url: String = chars[close_bracket + 2..close_paren].iter().collect();
    let rendered = format!("{} {}", ui.link(&text), ui.dim(&format!("({url})")));
    Some((rendered, close_paren + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Ui;

    fn ui() -> Ui {
        Ui::plain()
    }

    fn render(input: &str) -> String {
        let mut buf: Vec<u8> = Vec::new();
        let mut md = MarkdownStream::new(ui(), &mut buf);
        md.push(input);
        md.finish();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn inline_styles_strip_markers_in_plain_mode() {
        assert_eq!(render_inline(&ui(), "a **bold** b"), "a bold b");
        assert_eq!(render_inline(&ui(), "a *italic* b"), "a italic b");
        assert_eq!(render_inline(&ui(), "use `code` here"), "use code here");
    }

    #[test]
    fn nested_bold_then_code() {
        // Bold around code keeps the code content.
        assert_eq!(render_inline(&ui(), "**`x`**"), "x");
    }

    #[test]
    fn link_renders_text_and_url() {
        assert_eq!(
            render_inline(&ui(), "see [Oxen](https://oxen.ai)"),
            "see Oxen (https://oxen.ai)"
        );
    }

    #[test]
    fn escape_keeps_literal_marker() {
        assert_eq!(render_inline(&ui(), r"a \* b"), "a * b");
    }

    #[test]
    fn underscores_are_left_literal() {
        assert_eq!(render_inline(&ui(), "call my_var_name"), "call my_var_name");
    }

    #[test]
    fn heading_strips_hashes() {
        assert_eq!(render_block_line(&ui(), "## Title here"), "Title here");
    }

    #[test]
    fn unordered_list_uses_bullet() {
        assert_eq!(render_block_line(&ui(), "- item"), "• item");
    }

    #[test]
    fn ordered_list_keeps_number() {
        assert_eq!(render_block_line(&ui(), "3. third"), "3. third");
    }

    #[test]
    fn blockquote_gets_copyable_marker() {
        // `>` (not a box-drawing bar) so a copied quote is still valid Markdown.
        assert_eq!(render_block_line(&ui(), "> quoted"), "> quoted");
    }

    #[test]
    fn horizontal_rule_renders_a_line() {
        let out = render_block_line(&ui(), "---");
        assert!(out.chars().all(|c| c == '─'));
    }

    #[test]
    fn fenced_code_block_is_flush_left_with_no_side_bars() {
        let out = render("```rust\nfn main() {}\n```\n");
        // The language label lives in the rule above the code.
        assert!(out.contains("rust"));
        // Code lines carry no gutter, bar, or indent — a terminal selection of
        // the block pastes back exactly as written (and is not inline-parsed).
        assert!(out.lines().any(|l| l == "fn main() {}"), "got:\n{out}");
        assert!(!out.contains('│'), "no side bars expected:\n{out}");
        assert!(!out.contains('┌') && !out.contains('└'));
    }

    #[test]
    fn fenced_code_is_syntax_highlighted_when_color_is_on() {
        let ui = Ui::with(true, std::sync::Arc::new(harness_theme::Theme::default()));
        let mut buf: Vec<u8> = Vec::new();
        let mut md = MarkdownStream::new(ui, &mut buf);
        md.push("```rust\nfn main() {}\n```\n");
        md.finish();
        let out = String::from_utf8(buf).unwrap();
        let code_line = out
            .lines()
            .find(|l| l.contains("fn"))
            .expect("code line rendered");
        // Token-level coloring: the line carries multiple truecolor spans, not
        // the single flat ui.code() wrap.
        assert!(
            code_line.matches("\x1b[38;2;").count() >= 2,
            "expected multiple token spans: {code_line:?}"
        );
        // The visible text survives untouched (flush-left, copy-clean).
        let stripped: String = {
            let mut s = String::new();
            let mut chars = code_line.chars();
            while let Some(c) = chars.next() {
                if c == '\x1b' {
                    for n in chars.by_ref() {
                        if n.is_ascii_alphabetic() {
                            break;
                        }
                    }
                } else {
                    s.push(c);
                }
            }
            s
        };
        assert_eq!(stripped, "fn main() {}");
    }

    #[test]
    fn unknown_fence_language_falls_back_to_flat_code_color() {
        let ui = Ui::with(true, std::sync::Arc::new(harness_theme::Theme::default()));
        let mut buf: Vec<u8> = Vec::new();
        let mut md = MarkdownStream::new(ui, &mut buf);
        md.push("```made-up-lang\nsome text here\n```\n");
        md.finish();
        let out = String::from_utf8(buf).unwrap();
        let code_line = out
            .lines()
            .find(|l| l.contains("some text"))
            .expect("code line rendered");
        // Exactly one span: the theme's flat code color.
        assert_eq!(code_line.matches("\x1b[38;2;").count(), 1);
    }

    #[test]
    fn partial_line_flushes_on_finish() {
        // No trailing newline — must still appear after finish().
        assert_eq!(render("final words"), "final words\n");
    }

    #[test]
    fn list_preserves_indentation() {
        assert_eq!(render_block_line(&ui(), "  - nested"), "  • nested");
    }

    #[test]
    fn delimiter_row_is_detected() {
        assert!(is_delimiter_row("|---|:--:|--:|"));
        assert!(is_delimiter_row("--- | ---"));
        assert!(!is_delimiter_row("| a | b |"));
        assert!(!is_delimiter_row("| --- | x |"));
    }

    #[test]
    fn parse_row_splits_and_trims_and_unescapes() {
        assert_eq!(parse_row("| a | b |"), vec!["a", "b"]);
        assert_eq!(parse_row("a|b|c"), vec!["a", "b", "c"]);
        assert_eq!(parse_row(r"| a \| b | c |"), vec!["a | b", "c"]);
    }

    #[test]
    fn display_width_ignores_ansi_escapes() {
        assert_eq!(display_width("hi"), 2);
        assert_eq!(display_width("\x1b[1mhi\x1b[0m"), 2);
        assert_eq!(display_width("\x1b[38;2;1;2;3mX\x1b[0m"), 1);
    }

    #[test]
    fn table_renders_an_aligned_box() {
        let out = render(
            "| Crate | Responsibility |\n|-------|----------------|\n| harness-core | Shared types |\n",
        );
        // Box-drawing frame is present.
        assert!(out.contains('┌') && out.contains('┐'));
        assert!(out.contains('├') && out.contains('┼') && out.contains('┤'));
        assert!(out.contains('└') && out.contains('┴') && out.contains('┘'));
        assert!(out.contains("Crate"));
        assert!(out.contains("harness-core"));
        // Raw Markdown pipes/dashes must not leak through.
        assert!(!out.contains("|---"));

        // Every rendered table line shares the same visible width (columns line up).
        let widths: Vec<usize> = out
            .lines()
            .filter(|l| l.contains('│') || l.contains('┼') || l.contains('┬') || l.contains('┴'))
            .map(display_width)
            .collect();
        assert!(widths.len() >= 5, "expected 5 table lines, got {widths:?}");
        assert!(
            widths.iter().all(|w| *w == widths[0]),
            "table lines misaligned: {widths:?}"
        );
    }

    #[test]
    fn table_right_alignment_hugs_the_border() {
        let out = render("| Name | Score |\n|:-----|------:|\n| ab | 7 |\n");
        // The right-aligned value sits flush against the closing border.
        assert!(
            out.lines().any(|l| l.contains("7 │")),
            "expected right-aligned 7, got:\n{out}"
        );
    }

    #[test]
    fn lone_pipe_line_is_not_treated_as_a_table() {
        // A pipe in prose, with no delimiter row, renders literally.
        assert_eq!(render("a | b\n"), "a | b\n");
    }
}
