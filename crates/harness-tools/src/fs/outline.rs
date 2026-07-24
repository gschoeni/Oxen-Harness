//! Structural reads: show a file's shape, elide the bodies.
//!
//! Reading a 2,000-line file to change one function costs 2,000 lines of
//! context — and keeps costing them, because the transcript is resent on every
//! later turn until compaction eats it. Yet what the model needs from most of
//! that file is its *shape*: the imports, the types, the signatures.
//!
//! So a whole-file read of a large, parseable source file returns the file with
//! function bodies collapsed:
//!
//! ```text
//!      1  use std::fmt;
//!      3  pub struct Config {
//!      4      pub name: String,
//!      5  }
//!      8  impl Config {
//!      9      pub fn load(path: &Path) -> Result<Self> {
//! … [22 lines elided — read_file offset=10 limit=22 to see them]
//!     32      }
//!     33  }
//! ```
//!
//! Only function-like bodies are hidden. Type declarations, constants, and
//! imports are the interface — they stay. The elided ranges are recorded as
//! *unseen* (see [`super::state`]), so an edit aimed inside one is refused
//! with the range to re-read rather than applied blind.

use std::path::Path;

use tree_sitter::{Language, Node, Parser};

/// Below this many lines a file is shown whole: the round-trip to re-read an
/// elided body costs more than the lines saved.
pub const MIN_OUTLINE_LINES: usize = 200;

/// A body shorter than this stays visible — collapsing four lines into a
/// one-line marker saves nothing and reads worse.
const MIN_BODY_LINES: usize = 6;

/// Files larger than this are never parsed for an outline.
pub const MAX_OUTLINE_BYTES: u64 = 2 * 1024 * 1024;

/// An outline must hide at least this fraction of the file to be worth the
/// re-read risk; otherwise the model gets the real thing.
const MIN_ELIDED_FRACTION: f64 = 0.25;

/// A file rendered with its function bodies collapsed.
pub struct Outline {
    /// Line-numbered text, in the same shape a normal read returns.
    pub text: String,
    /// 1-based inclusive line ranges the model was actually shown.
    pub seen: Vec<(usize, usize)>,
    /// 1-based inclusive line ranges hidden behind markers.
    pub elided: Vec<(usize, usize)>,
    /// Total lines in the file.
    pub total_lines: usize,
}

impl Outline {
    /// How many lines the outline hid.
    pub fn elided_lines(&self) -> usize {
        self.elided.iter().map(|(a, b)| b - a + 1).sum()
    }
}

/// The grammar for a path's extension, if we ship one.
fn language_for(path: &Path) -> Option<Language> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::LANGUAGE.into(),
        "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        _ => return None,
    })
}

/// Node kinds whose `body` field is an implementation detail rather than an
/// interface. Deliberately not a list of *declarations* to keep: hiding bodies
/// leaves types, constants, imports, and signatures in place without needing a
/// per-language table of everything worth showing.
fn is_function_like(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_definition"
            | "function_declaration"
            | "method_declaration"
            | "method_definition"
            | "arrow_function"
            | "function_expression"
            | "generator_function_declaration"
    )
}

/// Render `source` with its function bodies collapsed, or `None` when the
/// language isn't one we parse, the file won't parse, or the result wouldn't
/// hide enough to be worth a possible re-read.
pub fn summarize(path: &Path, source: &str) -> Option<Outline> {
    let language = language_for(path)?;
    let total_lines = source.lines().count();
    if total_lines < MIN_OUTLINE_LINES {
        return None;
    }

    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;

    let mut elided: Vec<(usize, usize)> = Vec::new();
    collect_bodies(tree.root_node(), &mut elided);
    if elided.is_empty() {
        return None;
    }

    // Bodies nest (a closure inside a function); keep only the outermost runs.
    elided.sort_by_key(|&(start, _)| start);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in elided {
        match merged.last_mut() {
            Some((_, prev_end)) if start <= *prev_end + 1 => *prev_end = (*prev_end).max(end),
            _ => merged.push((start, end)),
        }
    }

    let hidden: usize = merged.iter().map(|(a, b)| b - a + 1).sum();
    if (hidden as f64) < total_lines as f64 * MIN_ELIDED_FRACTION {
        return None;
    }

    let text = render(source, &merged, path);
    let seen = complement(&merged, total_lines);
    Some(Outline {
        text,
        seen,
        elided: merged,
        total_lines,
    })
}

/// Walk for function-like nodes, recording the interior of each body: the
/// opening and closing lines stay so the structure still reads.
fn collect_bodies(node: Node<'_>, out: &mut Vec<(usize, usize)>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_function_like(child.kind()) {
            if let Some(body) = child.child_by_field_name("body") {
                let first = body.start_position().row + 2; // 1-based, past `{`
                let last = body.end_position().row; // 1-based, before `}`
                if last >= first && last - first + 1 >= MIN_BODY_LINES {
                    out.push((first, last));
                    // Everything inside is already hidden; don't descend.
                    continue;
                }
            }
        }
        collect_bodies(child, out);
    }
}

/// The line ranges *not* covered by `elided`.
fn complement(elided: &[(usize, usize)], total: usize) -> Vec<(usize, usize)> {
    let mut seen = Vec::new();
    let mut next = 1usize;
    for &(start, end) in elided {
        if start > next {
            seen.push((next, start - 1));
        }
        next = end + 1;
    }
    if next <= total {
        seen.push((next, total));
    }
    seen
}

/// Line-numbered text with each elided run replaced by a marker naming the
/// exact `read_file` arguments that bring it back.
fn render(source: &str, elided: &[(usize, usize)], path: &Path) -> String {
    let mut out = String::new();
    let mut hide = elided.iter().peekable();
    for (index, line) in source.lines().enumerate() {
        let number = index + 1;
        if let Some(&&(start, end)) = hide.peek() {
            if number == start {
                out.push_str(&format!(
                    "… [{} lines elided — read_file path=\"{}\" offset={start} limit={} to see them]\n",
                    end - start + 1,
                    path.display(),
                    end - start + 1,
                ));
            }
            if number >= start && number <= end {
                if number == end {
                    hide.next();
                }
                continue;
            }
        }
        out.push_str(&format!("{number:>6}\t{line}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file with a long function body and a type declaration, over the
    /// minimum outline size.
    fn rust_source() -> String {
        let mut src = String::from("use std::fmt;\n\npub struct Config {\n    pub name: String,\n}\n\nimpl Config {\n    pub fn load() -> Self {\n");
        for i in 0..250 {
            src.push_str(&format!("        let step_{i} = {i};\n"));
        }
        src.push_str("        Self { name: String::new() }\n    }\n}\n");
        src
    }

    #[test]
    fn a_long_function_body_is_elided_but_its_signature_stays() {
        let src = rust_source();
        let outline = summarize(Path::new("config.rs"), &src).expect("outline");

        assert!(outline.text.contains("pub struct Config"));
        assert!(outline.text.contains("pub name: String"));
        assert!(outline.text.contains("pub fn load() -> Self {"));
        assert!(!outline.text.contains("let step_100 = 100;"));
        assert!(outline.text.contains("lines elided"));
        // The marker says exactly how to get the body back.
        assert!(outline.text.contains("offset=9"), "{}", outline.text);
        assert!(outline.elided_lines() > 200);
    }

    #[test]
    fn seen_and_elided_ranges_cover_the_file_exactly() {
        let src = rust_source();
        let outline = summarize(Path::new("config.rs"), &src).unwrap();

        let mut covered: Vec<(usize, usize)> = outline
            .seen
            .iter()
            .chain(outline.elided.iter())
            .copied()
            .collect();
        covered.sort();
        let total: usize = covered.iter().map(|(a, b)| b - a + 1).sum();
        assert_eq!(total, outline.total_lines);
        // No overlaps, no gaps.
        for pair in covered.windows(2) {
            assert_eq!(pair[0].1 + 1, pair[1].0, "gap or overlap at {pair:?}");
        }
    }

    #[test]
    fn a_short_file_is_never_outlined() {
        let src = "fn main() {\n    println!(\"hi\");\n}\n";
        assert!(summarize(Path::new("main.rs"), src).is_none());
    }

    #[test]
    fn a_long_file_of_declarations_is_not_worth_outlining() {
        // All interface, no bodies: the model would gain nothing and might
        // have to re-read.
        let src = (0..300)
            .map(|i| format!("pub const VALUE_{i}: usize = {i};"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(summarize(Path::new("consts.rs"), &src).is_none());
    }

    #[test]
    fn an_unsupported_language_falls_through() {
        let src = (0..300)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(summarize(Path::new("notes.txt"), &src).is_none());
    }

    #[test]
    fn python_and_typescript_parse_too() {
        let mut py = String::from("import os\n\nclass Config:\n    def load(self):\n");
        for i in 0..250 {
            py.push_str(&format!("        step_{i} = {i}\n"));
        }
        let outline = summarize(Path::new("config.py"), &py).expect("python outline");
        assert!(outline.text.contains("class Config:"));
        assert!(outline.text.contains("def load(self):"));
        assert!(!outline.text.contains("step_100 = 100"));

        let mut ts = String::from("import { z } from \"zod\";\n\nexport function load(): void {\n");
        for i in 0..250 {
            ts.push_str(&format!("  const step{i} = {i};\n"));
        }
        ts.push_str("}\n");
        let outline = summarize(Path::new("config.ts"), &ts).expect("ts outline");
        assert!(outline.text.contains("export function load(): void {"));
        assert!(!outline.text.contains("const step100 = 100;"));
    }

    #[test]
    fn nested_functions_produce_one_elision_not_two() {
        let mut src = String::from("pub fn outer() {\n");
        for i in 0..120 {
            src.push_str(&format!("    let a_{i} = {i};\n"));
        }
        src.push_str("    let inner = || {\n");
        for i in 0..120 {
            src.push_str(&format!("        let b_{i} = {i};\n"));
        }
        src.push_str("    };\n}\n");

        let outline = summarize(Path::new("nested.rs"), &src).unwrap();

        assert_eq!(outline.elided.len(), 1, "{:?}", outline.elided);
        assert_eq!(outline.text.matches("lines elided").count(), 1);
    }
}
