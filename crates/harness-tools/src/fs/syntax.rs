//! Did that edit leave the file parseable?
//!
//! The most common way a patch goes wrong is structural: a replacement that
//! drops a closing brace, an `old_string` that straddled a block boundary, a
//! batch whose hunks were individually right and jointly wrong. Nothing
//! catches it until a build runs — which is minutes later, or never, because
//! the model moved on believing the edit succeeded.
//!
//! The grammars loaded for outline reads answer this for free: parse the
//! before and after, and if the edit *introduced* an error node, say so on the
//! tool result while the model is still holding the context to fix it.
//!
//! Deliberately a comparison, not an assertion. A file that was already broken
//! (mid-refactor, a generated stub, a template with placeholders) must not be
//! blamed on the edit that touched it — that would train the model to
//! second-guess correct work.

use std::path::Path;

use tree_sitter::{Node, Parser};

/// Where an edit broke the syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxBreak {
    /// 1-based line of the first error node.
    pub line: usize,
    /// The offending line, trimmed, for the message.
    pub text: String,
}

/// The first syntax error in `source`, if the language is one we parse.
/// `None` also means "not parseable by us", which is why callers compare a
/// before and an after rather than trusting a single result.
fn first_error(path: &Path, source: &str) -> Option<SyntaxBreak> {
    let language = super::outline::language_for(path)?;
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    if !tree.root_node().has_error() {
        return None;
    }
    let node = find_error(tree.root_node())?;
    let line = node.start_position().row + 1;
    Some(SyntaxBreak {
        line,
        text: source
            .lines()
            .nth(line - 1)
            .unwrap_or_default()
            .trim()
            .chars()
            .take(120)
            .collect(),
    })
}

/// The first `ERROR`/missing node in document order.
fn find_error<'t>(node: Node<'t>) -> Option<Node<'t>> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    if !node.has_error() {
        return None;
    }
    // Collected before recursing: the cursor borrows the node it walks, so it
    // cannot outlive this frame while the returned node must.
    let children: Vec<Node<'t>> = {
        let mut cursor = node.walk();
        node.children(&mut cursor).collect()
    };
    children.into_iter().find_map(find_error)
}

/// A warning to append to a tool result when an edit introduced a syntax
/// error that wasn't there before. `before` of `None` means the file is new.
pub fn regression_note(path: &Path, before: Option<&str>, after: &str) -> Option<String> {
    // Files too large to outline are too large to re-parse twice per edit.
    if after.len() as u64 > super::outline::MAX_OUTLINE_BYTES {
        return None;
    }
    let broke = first_error(path, after)?;
    // Already broken before the edit: not this edit's doing, and saying so
    // would be noise at best and misleading at worst.
    if before.is_some_and(|text| first_error(path, text).is_some()) {
        return None;
    }
    Some(format!(
        "warning: this left a syntax error at line {} (`{}`) — the file no longer parses. \
         Read that area and fix it before moving on.",
        broke.line, broke.text
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_that_breaks_the_syntax_is_reported_with_its_line() {
        let before = "fn main() {\n    println!(\"hi\");\n}\n";
        let after = "fn main() {\n    println!(\"hi\");\n";

        let note = regression_note(Path::new("main.rs"), Some(before), after).expect("a warning");

        assert!(note.contains("syntax error"), "{note}");
        assert!(note.contains("line"), "{note}");
    }

    #[test]
    fn a_clean_edit_says_nothing() {
        let before = "fn main() {\n    let x = 1;\n}\n";
        let after = "fn main() {\n    let x = 2;\n}\n";
        assert!(regression_note(Path::new("main.rs"), Some(before), after).is_none());
    }

    #[test]
    fn a_file_that_was_already_broken_is_not_blamed_on_this_edit() {
        // Mid-refactor state: the model is fixing it, not breaking it.
        let before = "fn main() {\n    let x = ;\n";
        let after = "fn main() {\n    let x = 1;\n";
        assert!(regression_note(Path::new("main.rs"), Some(before), after).is_none());
    }

    #[test]
    fn a_new_file_with_broken_syntax_is_still_reported() {
        let note = regression_note(Path::new("new.rs"), None, "fn main( {\n").expect("a warning");
        assert!(note.contains("syntax error"), "{note}");
    }

    #[test]
    fn an_unparsed_language_is_never_second_guessed() {
        assert!(regression_note(Path::new("notes.txt"), Some("a"), "b {{{").is_none());
        assert!(regression_note(Path::new("data.json"), None, "{{{").is_none());
    }

    #[test]
    fn python_and_typescript_are_covered_too() {
        let py = regression_note(
            Path::new("app.py"),
            Some("def f():\n    return 1\n"),
            "def f(:\n    return 1\n",
        );
        assert!(py.is_some(), "python break should be reported");

        let ts = regression_note(
            Path::new("app.ts"),
            Some("export function f() {\n  return 1;\n}\n"),
            "export function f() {\n  return 1;\n",
        );
        assert!(ts.is_some(), "typescript break should be reported");
    }
}
