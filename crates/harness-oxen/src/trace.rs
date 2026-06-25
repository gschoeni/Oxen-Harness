//! Exporting a conversation as a shareable Oxen repository.
//!
//! A trace is the session transcript (JSONL) plus the attachment files it
//! references. Bundling both — with attachments kept at the same project-relative
//! path the transcript records — means whoever clones the trace can replay it
//! with attachments intact.

use std::path::{Path, PathBuf};

use crate::{Oxen, OxenError, Runner};

/// File name the transcript is written to inside an exported trace.
pub const TRANSCRIPT_FILE: &str = "transcript.jsonl";

/// One attachment to include in a trace: the path it's recorded under in the
/// transcript (relative to the project root) and where to copy it from.
#[derive(Debug, Clone)]
pub struct TraceAttachment {
    pub rel_path: String,
    pub source: PathBuf,
}

/// The contents of a trace to export.
pub struct TraceBundle<'a> {
    /// The transcript as JSONL (one message per line).
    pub transcript_jsonl: &'a str,
    /// Attachment files the transcript references.
    pub attachments: Vec<TraceAttachment>,
}

/// Materialize `bundle` into an Oxen repository at `dest` and commit it. When
/// `remote` is `Some(url)`, also set it as `origin` and push `main` so the trace
/// can be shared on the Oxen hub.
///
/// Attachments are copied to the same relative path the transcript references, so
/// the cloned trace hydrates correctly. A referenced source that's missing is
/// skipped (best effort) rather than failing the whole export.
pub fn export_trace<R: Runner>(
    oxen: &Oxen<R>,
    bundle: &TraceBundle,
    dest: &Path,
    remote: Option<&str>,
) -> Result<(), OxenError> {
    std::fs::create_dir_all(dest)?;
    std::fs::write(dest.join(TRANSCRIPT_FILE), bundle.transcript_jsonl)?;

    for att in &bundle.attachments {
        if !att.source.exists() {
            continue;
        }
        let target = dest.join(&att.rel_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&att.source, &target)?;
    }

    oxen.snapshot(dest, "export oxen-harness trace")?;
    if let Some(url) = remote {
        oxen.set_remote(dest, "origin", url)?;
        oxen.push(dest, "origin", "main")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeRunner;

    #[test]
    fn writes_transcript_copies_attachments_and_pushes() {
        let src_dir = tempfile::tempdir().unwrap();
        let img = src_dir.path().join("shot.png");
        std::fs::write(&img, b"PNGDATA").unwrap();

        let dest = tempfile::tempdir().unwrap();
        let dest_repo = dest.path().join("trace");

        let oxen = Oxen::with_runner(FakeRunner::ok());
        let bundle = TraceBundle {
            transcript_jsonl: "{\"role\":\"user\"}\n",
            attachments: vec![TraceAttachment {
                rel_path: ".oxen-harness/attachments/shot.png".into(),
                source: img.clone(),
            }],
        };

        export_trace(
            &oxen,
            &bundle,
            &dest_repo,
            Some("https://hub.oxen.ai/me/trace"),
        )
        .unwrap();

        // Transcript + attachment landed at the expected paths.
        assert_eq!(
            std::fs::read_to_string(dest_repo.join(TRANSCRIPT_FILE)).unwrap(),
            "{\"role\":\"user\"}\n"
        );
        assert_eq!(
            std::fs::read(dest_repo.join(".oxen-harness/attachments/shot.png")).unwrap(),
            b"PNGDATA"
        );

        // It initialized, staged, committed, set the remote, and pushed.
        let argv: Vec<String> = oxen.runner.calls().into_iter().map(|c| c.args).collect();
        assert_eq!(argv[0], "init");
        assert_eq!(argv[1], "add .");
        assert_eq!(argv[2], "commit -m export oxen-harness trace");
        assert_eq!(
            argv[3],
            "config --set-remote origin https://hub.oxen.ai/me/trace"
        );
        assert_eq!(argv[4], "push origin main");
    }
}
