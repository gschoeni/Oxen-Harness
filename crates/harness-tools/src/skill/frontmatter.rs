//! The `---` frontmatter block at the top of a `SKILL.md`.
//!
//! Skills are shared between tools (Claude, Codex, this harness) as plain
//! folders, so a skill written elsewhere has to read correctly here: quoted
//! values, `>`/`|` block scalars, and keys this harness doesn't use
//! (`license`, `allowed-tools`, nested `metadata`). This is not a YAML
//! parser. It reads top-level `key: value` entries, interprets the scalars
//! `name` and `description`, and keeps every entry's raw text so a rewrite can
//! carry the keys it doesn't understand through untouched.

/// One top-level frontmatter entry: the `key:` line plus its indented
/// continuation lines, verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub raw: String,
}

impl Entry {
    /// The entry's value read as a YAML scalar, with whitespace collapsed to
    /// single spaces (names and descriptions are one line).
    pub fn scalar(&self) -> String {
        let mut lines = self.raw.lines();
        let first = lines.next().unwrap_or_default();
        let inline = first.split_once(':').map_or("", |(_, v)| v).trim();
        let rest: Vec<&str> = lines.map(str::trim).collect();

        let text = if inline.starts_with('|') || inline.starts_with('>') {
            rest.join(" ")
        } else {
            let joined = std::iter::once(inline)
                .chain(rest.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            unquote(&joined)
        };
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// Read a quoted or plain scalar. Plain scalars lose a trailing ` # comment`.
fn unquote(s: &str) -> String {
    if let Some(inner) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => {}
                },
                c => out.push(c),
            }
        }
        out
    } else if let Some(inner) = s.strip_prefix('\'') {
        let inner = match inner.rfind('\'') {
            Some(end) => &inner[..end],
            None => inner,
        };
        inner.replace("''", "'")
    } else {
        match s.find(" #") {
            Some(i) => s[..i].to_string(),
            None => s.to_string(),
        }
    }
}

/// Split a `SKILL.md` into its frontmatter entries and the markdown body.
pub fn split(contents: &str) -> Result<(Vec<Entry>, &str), String> {
    // Windows editors often prepend a byte-order mark.
    let rest = contents
        .trim_start_matches('\u{feff}')
        .strip_prefix("---")
        .ok_or("SKILL.md must start with `---` frontmatter")?;
    let (block, body) = rest
        .split_once("\n---")
        .ok_or("frontmatter is missing its closing `---`")?;

    let mut entries: Vec<Entry> = Vec::new();
    for line in block.lines() {
        let top_level = !line.starts_with([' ', '\t', '-', '#']) && line.contains(':');
        match (top_level, entries.last_mut()) {
            (true, _) => {
                let key = line.split_once(':').map_or("", |(k, _)| k).trim();
                entries.push(Entry {
                    key: key.to_string(),
                    raw: line.to_string(),
                });
            }
            // Continuations: indented lines, list items, blank lines.
            (false, Some(entry)) => {
                entry.raw.push('\n');
                entry.raw.push_str(line);
            }
            (false, None) => {} // stray text before the first key
        }
    }
    // Blank lines between entries belong to neither.
    for entry in &mut entries {
        entry.raw.truncate(entry.raw.trim_end().len());
    }
    Ok((entries, body.trim_start_matches('-')))
}

/// Render a value as a YAML scalar that any YAML parser reads back as the
/// same string: plain when that's unambiguous, double-quoted otherwise.
pub fn scalar(value: &str) -> String {
    let reserved = ["true", "false", "yes", "no", "on", "off", "null", "~"];
    let plain = !value.is_empty()
        && !value.starts_with([
            '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%',
            '@', '`', ' ',
        ])
        && !value.ends_with([':', ' '])
        && !value.contains(": ")
        && !value.contains(" #")
        && !value.contains(['\n', '\t'])
        && !reserved.contains(&value.to_ascii_lowercase().as_str())
        && value.parse::<f64>().is_err();
    if plain {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(fm: &str) -> Vec<Entry> {
        split(&format!("---\n{fm}\n---\nBody")).unwrap().0
    }

    fn value(fm: &str, key: &str) -> String {
        entries(fm)
            .into_iter()
            .find(|e| e.key == key)
            .map(|e| e.scalar())
            .unwrap_or_default()
    }

    #[test]
    fn reads_plain_quoted_and_block_scalars() {
        assert_eq!(
            value("description: Plain text.", "description"),
            "Plain text."
        );
        assert_eq!(
            value("description: \"Says \\\"hi\\\": twice\"", "description"),
            "Says \"hi\": twice"
        );
        assert_eq!(
            value("description: 'It''s fine'", "description"),
            "It's fine"
        );
        assert_eq!(
            value(
                "description: >\n  Folded across\n  two lines.",
                "description"
            ),
            "Folded across two lines."
        );
        assert_eq!(
            value("description: |-\n  Literal\n  block.", "description"),
            "Literal block."
        );
        assert_eq!(
            value("description: Plain # comment", "description"),
            "Plain"
        );
    }

    #[test]
    fn tolerates_a_byte_order_mark() {
        let (es, body) = split("\u{feff}---\nname: x\n---\nBody").unwrap();
        assert_eq!(es[0].scalar(), "x");
        assert_eq!(body.trim(), "Body");
    }

    #[test]
    fn only_top_level_keys_count() {
        let fm =
            "name: real\nmetadata:\n  name: nested\n  version: 2\nallowed-tools:\n- Bash\n- Read";
        let es = entries(fm);
        let keys: Vec<&str> = es.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["name", "metadata", "allowed-tools"]);
        assert_eq!(value(fm, "name"), "real");
        assert_eq!(es[1].raw, "metadata:\n  name: nested\n  version: 2");
        assert_eq!(es[2].raw, "allowed-tools:\n- Bash\n- Read");
    }

    #[test]
    fn scalars_round_trip() {
        for s in [
            "Writes notes.",
            "Use when: the user asks",
            "\"quoted\" start",
            "- dash start",
            "yes",
            "42",
            "back\\slash # and hash",
            "",
        ] {
            let fm = format!("description: {}", scalar(s));
            assert_eq!(value(&fm, "description"), s, "{fm}");
        }
        assert_eq!(scalar("Writes release notes."), "Writes release notes.");
    }
}
