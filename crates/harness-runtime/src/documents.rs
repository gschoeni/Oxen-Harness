//! Revision-aware project documents shared by renderers and agent services.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use harness_tools::Workspace;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Full editable documents are bounded; larger files use the existing reader.
pub const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocumentSnapshot {
    pub path: String,
    pub content: String,
    pub revision: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("{operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} changed on disk; reload or compare before saving (expected {expected:?}, current {current:?})")]
    Conflict {
        path: String,
        expected: Option<String>,
        current: Option<String>,
    },
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Tool(#[from] harness_tools::ToolError),
}

#[derive(Clone, Debug)]
pub struct Documents {
    workspace: Workspace,
}

impl Documents {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, DocumentError> {
        Ok(Self {
            workspace: Workspace::new(root)?,
        })
    }

    pub fn root(&self) -> &Path {
        self.workspace.root()
    }

    /// Refuse symlink traversal in host-facing file APIs, including missing
    /// leaves. The existing agent workspace resolver alone is lexical.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf, DocumentError> {
        if relative.starts_with(".oxen-harness/recovery/") {
            return Err(DocumentError::Invalid(
                "recovery copies are host-owned".into(),
            ));
        }
        secure_path(&self.workspace, relative)
    }

    pub fn read(&self, relative: &str) -> Result<DocumentSnapshot, DocumentError> {
        read_snapshot(&self.resolve(relative)?, relative)
    }

    /// `None` means create, never unconditional overwrite. A stale save keeps
    /// both the on-disk document and the caller's unsaved buffer intact.
    pub async fn save(
        &self,
        relative: &str,
        content: &str,
        expected_revision: Option<&str>,
    ) -> Result<DocumentSnapshot, DocumentError> {
        if content.len() > MAX_DOCUMENT_BYTES {
            return Err(size_error(relative));
        }
        let path = self.resolve(relative)?;
        let _guard = harness_tools::path_lock::lock(&path).await?;
        let path = self.resolve(relative)?;
        let before = match read_snapshot(&path, relative) {
            Ok(doc) => Some(doc),
            Err(DocumentError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let revision = before.as_ref().map(|doc| doc.revision.as_str());
        if revision != expected_revision {
            return Err(DocumentError::Conflict {
                path: relative.into(),
                expected: expected_revision.map(str::to_string),
                current: revision.map(str::to_string),
            });
        }
        if let Some(before) = before {
            let backup = self.recovery_path(relative)?;
            atomic_write(&backup, before.content.as_bytes())?;
        }
        atomic_write(&path, content.as_bytes())?;
        Ok(DocumentSnapshot {
            path: relative.into(),
            content: content.into(),
            revision: revision_of(content.as_bytes()),
        })
    }

    pub fn previous(&self, relative: &str) -> Result<Option<DocumentSnapshot>, DocumentError> {
        self.resolve(relative)?;
        match read_snapshot(&self.recovery_path(relative)?, relative) {
            Ok(doc) => Ok(Some(doc)),
            Err(DocumentError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn recovery_path(&self, relative: &str) -> Result<PathBuf, DocumentError> {
        secure_path(
            &self.workspace,
            &format!(
                ".oxen-harness/recovery/{}.previous",
                revision_of(relative.as_bytes())
            ),
        )
    }
}

pub fn revision_of(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn secure_path(workspace: &Workspace, relative: &str) -> Result<PathBuf, DocumentError> {
    let candidate = Path::new(relative);
    if relative.is_empty()
        || relative.contains('\\')
        || relative.contains('\0')
        || candidate
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(DocumentError::Invalid(format!(
            "invalid workspace-relative document path {relative:?}"
        )));
    }
    let resolved = workspace.resolve(candidate)?;
    let mut part = workspace.root().to_path_buf();
    for component in candidate.components() {
        part.push(component);
        match std::fs::symlink_metadata(&part) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(DocumentError::Invalid(format!(
                    "document path {relative} traverses a symlink"
                )))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(io_error("inspect document path", &part, source)),
        }
    }
    Ok(resolved)
}

fn read_snapshot(path: &Path, relative: &str) -> Result<DocumentSnapshot, DocumentError> {
    let metadata = std::fs::metadata(path).map_err(|e| io_error("inspect document", path, e))?;
    if !metadata.is_file() {
        return Err(DocumentError::Invalid(format!(
            "{relative} is not a regular file"
        )));
    }
    let file = std::fs::File::open(path).map_err(|e| io_error("read document", path, e))?;
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error("read document", path, e))?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(size_error(relative));
    }
    let revision = revision_of(&bytes);
    let content = String::from_utf8(bytes).map_err(|_| {
        DocumentError::Invalid(format!(
            "{relative} is not a UTF-8 document; open it in a media view"
        ))
    })?;
    Ok(DocumentSnapshot {
        path: relative.into(),
        content,
        revision,
    })
}

fn size_error(path: &str) -> DocumentError {
    DocumentError::Invalid(format!(
        "{path} exceeds the {MAX_DOCUMENT_BYTES}-byte editable document limit"
    ))
}

fn io_error(operation: &'static str, path: &Path, source: std::io::Error) -> DocumentError {
    DocumentError::Io {
        operation,
        path: path.into(),
        source,
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), DocumentError> {
    let parent = path.parent().ok_or_else(|| {
        DocumentError::Invalid(format!(
            "document {} has no parent directory",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| io_error("create document directory", parent, e))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| io_error("create document temporary file", path, e))?;
    match std::fs::metadata(path) {
        Ok(meta) => temp
            .as_file()
            .set_permissions(meta.permissions())
            .map_err(|e| io_error("preserve document permissions", path, e))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error("inspect document permissions", path, e)),
    }
    temp.write_all(bytes)
        .map_err(|e| io_error("write document", path, e))?;
    temp.as_file()
        .sync_all()
        .map_err(|e| io_error("sync document", path, e))?;
    temp.persist(path)
        .map_err(|e| io_error("replace document", path, e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saves_are_atomic_revision_checked_and_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let docs = Documents::new(dir.path()).unwrap();
        let first = docs
            .save("work/graph.json", "{\"version\":1}", None)
            .await
            .unwrap();
        let second = docs
            .save(&first.path, "{\"version\":2}", Some(&first.revision))
            .await
            .unwrap();
        assert_ne!(first.revision, second.revision);
        let error = docs
            .save(&first.path, "old change", Some(&first.revision))
            .await
            .unwrap_err();
        assert!(matches!(error, DocumentError::Conflict { .. }));
        assert!(error.to_string().contains("work/graph.json"));
        assert_eq!(docs.read(&first.path).unwrap(), second);
        assert_eq!(
            docs.previous(&first.path).unwrap().unwrap().content,
            first.content
        );
        assert!(docs
            .save(&first.path, "create over existing", None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn two_document_services_do_not_overwrite_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let a = Documents::new(dir.path()).unwrap();
        let b = Documents::new(dir.path()).unwrap();
        let initial = a.save("a.txt", "initial", None).await.unwrap();
        let (a, b) = tokio::join!(
            a.save("a.txt", "from a", Some(&initial.revision)),
            b.save("a.txt", "from b", Some(&initial.revision))
        );
        assert_ne!(a.is_ok(), b.is_ok());
    }

    #[tokio::test]
    async fn rejects_escapes_missing_files_and_oversized_documents() {
        let dir = tempfile::tempdir().unwrap();
        let docs = Documents::new(dir.path()).unwrap();
        assert!(docs
            .read("missing.txt")
            .unwrap_err()
            .to_string()
            .contains("missing.txt"));
        for path in [
            "../outside",
            "/absolute",
            "",
            "a/../b",
            ".oxen-harness/recovery/overwrite",
        ] {
            assert!(docs.save(path, "bad", None).await.is_err(), "{path}");
        }
        let oversized = "x".repeat(MAX_DOCUMENT_BYTES + 1);
        assert!(docs.save("huge.txt", &oversized, None).await.is_err());
        std::fs::write(dir.path().join("huge.txt"), oversized).unwrap();
        assert!(docs
            .read("huge.txt")
            .unwrap_err()
            .to_string()
            .contains("limit"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_symlinks_even_when_the_leaf_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        let docs = Documents::new(dir.path()).unwrap();
        assert!(docs
            .save("escape/new.txt", "bad", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("symlink"));
        assert!(!outside.path().join("new.txt").exists());
    }
}
