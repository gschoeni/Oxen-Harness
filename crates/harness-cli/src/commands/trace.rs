//! `oxen-harness trace` — export a conversation as an Oxen repository so it can
//! be versioned and shared. A trace bundles the session transcript (JSONL) with
//! the attachment files it references, committed (and optionally pushed) with
//! the `oxen` CLI.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use harness_oxen::{export_trace, Oxen, TraceAttachment, TraceBundle};
use harness_store::HistoryStore;

use crate::theme::Ui;

#[derive(Debug, clap::Subcommand)]
pub(crate) enum TraceAction {
    /// Export a session's transcript + attachments to a local Oxen repo,
    /// optionally pushing it to a hub repo to share.
    Export {
        /// The session id to export.
        session: String,
        /// Where to write the trace repo (defaults to
        /// `~/.oxen-harness/exports/<session>`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// A remote Oxen repo URL to push the trace to, e.g.
        /// `https://hub.oxen.ai/<namespace>/<repo>` (the repo must already exist).
        #[arg(long)]
        push: Option<String>,
    },
}

pub(crate) fn run_trace(action: TraceAction, ui: &Ui) -> Result<()> {
    match action {
        TraceAction::Export { session, out, push } => export(&session, out, push.as_deref(), ui),
    }
}

fn export(session: &str, out: Option<PathBuf>, push: Option<&str>, ui: &Ui) -> Result<()> {
    let oxen = Oxen::new();
    if !oxen.is_available() {
        bail!(
            "the `oxen` CLI is not installed or not on PATH — install it from \
             https://docs.oxen.ai/getting-started/install to export traces"
        );
    }

    let db = harness_config::paths::history_db()?;
    let store = HistoryStore::open(&db).context("opening history store")?;
    let meta = store
        .session_meta(session)
        .with_context(|| format!("no session `{session}` in history"))?;
    let jsonl = store.export_jsonl(session)?;

    let attachments = collect_attachments(&jsonl, Path::new(&meta.workspace));
    let dest = match out {
        Some(p) => p,
        None => harness_config::paths::base_dir()?
            .join("exports")
            .join(session),
    };

    export_trace(
        &oxen,
        &TraceBundle {
            transcript_jsonl: &jsonl,
            attachments,
        },
        &dest,
        push,
    )?;

    println!(
        "  {} {}",
        ui.green("✓ exported trace"),
        ui.dim(&dest.display().to_string())
    );
    if let Some(url) = push {
        println!("  {} {}", ui.accent("↑ pushed to"), ui.cream(url));
    } else {
        println!(
            "  {}",
            ui.dim("share it with: oxen-harness trace export <session> --push <repo-url>")
        );
    }
    Ok(())
}

/// Collect the distinct on-disk attachment references in a transcript: the
/// project-relative paths recorded on image/file content parts. Inline (`data:`)
/// and remote (`http(s):`) references carry their own bytes and are skipped. Each
/// reference is resolved against the session's recorded workspace to find the
/// source file to bundle.
fn collect_attachments(jsonl: &str, workspace: &Path) -> Vec<TraceAttachment> {
    let mut refs = BTreeSet::new();
    for line in jsonl.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(parts) = value.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        for part in parts {
            let rel = part
                .get("image_url")
                .and_then(|u| u.get("url"))
                .and_then(|v| v.as_str())
                .or_else(|| {
                    part.get("file")
                        .and_then(|f| f.get("file_data"))
                        .and_then(|v| v.as_str())
                });
            if let Some(rel) = rel {
                if !is_inline(rel) {
                    refs.insert(rel.to_string());
                }
            }
        }
    }
    refs.into_iter()
        .map(|rel| TraceAttachment {
            source: workspace.join(&rel),
            rel_path: rel,
        })
        .collect()
}

fn is_inline(value: &str) -> bool {
    value.starts_with("data:") || value.starts_with("http://") || value.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_only_on_disk_references() {
        let jsonl = concat!(
            r#"{"role":"user","content":[{"type":"text","text":"hi"},{"type":"image_url","image_url":{"url":".oxen-harness/attachments/a.png"}}]}"#,
            "\n",
            r#"{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}]}"#,
            "\n",
            r#"{"role":"assistant","content":"plain text reply"}"#,
            "\n",
        );
        let atts = collect_attachments(jsonl, Path::new("/proj"));
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].rel_path, ".oxen-harness/attachments/a.png");
        assert_eq!(
            atts[0].source,
            Path::new("/proj/.oxen-harness/attachments/a.png")
        );
    }
}
