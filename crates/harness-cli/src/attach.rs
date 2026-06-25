//! Extracting dropped files from a typed prompt line.
//!
//! Dragging a file into a terminal inserts its path into the input (usually with
//! spaces backslash-escaped, sometimes quoted). This module tokenizes the line
//! the way a shell would, loads tokens that point at attachable files as
//! [`Attachment`]s, and returns the remaining text as the prompt — so
//! "describe ~/Desktop/shot.png" sends the image plus "describe".

use std::path::Path;

use harness_llm::{Attachment, AttachmentKind};

/// What a token should become when scanning a prompt line for dropped files.
enum Candidate {
    /// Ordinary prompt text — keep it in the message.
    PromptText,
    /// A media file (image/PDF/video) the model can't read with its tools, so
    /// it's always attached regardless of how the path was written.
    Media,
    /// A non-media file referenced by an absolute path — the signature of a
    /// drag-and-drop. Attached when it's a readable document; otherwise left as
    /// prompt text.
    Dropped,
}

/// Split a prompt line into its text and any attached files.
///
/// Returns the cleaned prompt (file paths removed), the loaded attachments, and
/// human-readable warnings for files that looked attachable but couldn't be read.
pub fn extract_attachments(input: &str) -> (String, Vec<Attachment>, Vec<String>) {
    let mut text_tokens = Vec::new();
    let mut attachments = Vec::new();
    let mut warnings = Vec::new();

    for tok in tokenize(input) {
        match classify(&tok) {
            Candidate::PromptText => text_tokens.push(tok),
            Candidate::Media => match Attachment::from_path(&tok) {
                Ok(a) => attachments.push(a),
                Err(e) => warnings.push(e.to_string()),
            },
            Candidate::Dropped => match Attachment::from_path(&tok) {
                // Inline readable documents; leave opaque binaries (and any read
                // error) as prompt text rather than attaching a useless note or
                // silently swallowing the path.
                Ok(a) if a.kind != AttachmentKind::Other => attachments.push(a),
                _ => text_tokens.push(tok),
            },
        }
    }

    // Nothing file-like — return the line verbatim so ordinary prompts keep their
    // exact whitespace/formatting (re-joining tokens would collapse it).
    if attachments.is_empty() && warnings.is_empty() {
        return (input.to_string(), attachments, warnings);
    }
    (text_tokens.join(" "), attachments, warnings)
}

/// Decide how a single token should be handled.
///
/// Media files are attached however they're referenced (you don't ask the agent
/// to edit a PNG). Any other existing file is only pulled in when written as an
/// absolute path — what a terminal inserts on drag-and-drop — so typed relative
/// references like `README.md` or `src/main.rs` stay in the prompt for the
/// agent's file tools instead of being vacuumed up as attachments.
fn classify(tok: &str) -> Candidate {
    let p = Path::new(tok);
    if !p.is_file() {
        return Candidate::PromptText;
    }
    match AttachmentKind::from_path(p) {
        AttachmentKind::Image | AttachmentKind::Pdf | AttachmentKind::Video => Candidate::Media,
        _ if p.is_absolute() => Candidate::Dropped,
        _ => Candidate::PromptText,
    }
}

/// Shell-style tokenizer: splits on whitespace, but honors single/double quotes
/// and backslash escapes so drag-dropped paths with spaces stay one token.
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut has = false; // current token has content (even if empty quotes)
    let mut quote: Option<char> = None;
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    if let Some(next) = chars.next() {
                        cur.push(next);
                    }
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    has = true;
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        cur.push(next);
                        has = true;
                    }
                }
                c if c.is_whitespace() => {
                    if has {
                        tokens.push(std::mem::take(&mut cur));
                        has = false;
                    }
                }
                c => {
                    cur.push(c);
                    has = true;
                }
            },
        }
    }
    if has {
        tokens.push(cur);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tokenize_handles_quotes_and_escaped_spaces() {
        assert_eq!(tokenize("a b c"), ["a", "b", "c"]);
        assert_eq!(tokenize(r"describe /tmp/My\ File.png"), ["describe", "/tmp/My File.png"]);
        assert_eq!(tokenize(r#"look "/tmp/a b.png" now"#), ["look", "/tmp/a b.png", "now"]);
    }

    #[test]
    fn extracts_existing_image_and_keeps_text() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("shot.png");
        std::fs::File::create(&img).unwrap().write_all(&[1, 2, 3]).unwrap();

        let line = format!("describe this {}", img.display());
        let (text, attachments, warnings) = extract_attachments(&line);
        assert_eq!(text, "describe this");
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, AttachmentKind::Image);
        assert!(warnings.is_empty());
    }

    #[test]
    fn leaves_plain_text_and_nonexistent_paths_alone() {
        let (text, attachments, _) = extract_attachments("just a normal /not/here.png message");
        assert_eq!(text, "just a normal /not/here.png message");
        assert!(attachments.is_empty());
    }

    #[test]
    fn dragged_text_document_is_attached_with_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("notes.md");
        std::fs::File::create(&doc)
            .unwrap()
            .write_all(b"# Plan\n\nship it")
            .unwrap();

        // A drag-drop inserts an absolute path.
        let line = format!("summarize {}", doc.display());
        let (text, attachments, warnings) = extract_attachments(&line);
        assert_eq!(text, "summarize");
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, AttachmentKind::Text);
        assert!(warnings.is_empty());
    }

    #[test]
    fn typed_relative_document_reference_stays_in_the_prompt() {
        // `Cargo.toml` exists relative to the crate dir (the test's working
        // directory) and is readable text, yet a *relative* reference must stay
        // in the prompt for the agent's file tools rather than being attached.
        let (text, attachments, _) = extract_attachments("open Cargo.toml and fix the deps");
        assert_eq!(text, "open Cargo.toml and fix the deps");
        assert!(attachments.is_empty(), "relative refs must not be attached");
    }

    #[test]
    fn dragged_opaque_binary_is_left_as_prompt_text() {
        let dir = tempfile::tempdir().unwrap();
        let blob = dir.path().join("data.bin");
        std::fs::File::create(&blob)
            .unwrap()
            .write_all(&[0u8, 1, 2, 3, 0xff])
            .unwrap();

        let line = format!("look at {}", blob.display());
        let (text, attachments, warnings) = extract_attachments(&line);
        assert_eq!(text, line, "unreadable binary stays verbatim in the prompt");
        assert!(attachments.is_empty());
        assert!(warnings.is_empty());
    }
}
