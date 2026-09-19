//! Coordinate cooperative file writers across sessions and host surfaces.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::ToolError;

type Locks = Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>;
static LOCKS: OnceLock<Locks> = OnceLock::new();

/// Hold the canonical path across the complete read/check/write operation.
/// Missing leaf paths use their closest existing ancestor so aliases of a new
/// file coordinate too. External processes do not participate in this lock.
pub async fn lock(path: &Path) -> Result<tokio::sync::OwnedMutexGuard<()>, ToolError> {
    let key = canonical_key(path)?;
    let mutex = {
        let mut locks = LOCKS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| ToolError::Execution("file coordination lock is poisoned".into()))?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(&key).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                locks.insert(key, Arc::downgrade(&lock));
                lock
            }
        }
    };
    Ok(mutex.lock_owned().await)
}

fn canonical_key(path: &Path) -> Result<PathBuf, ToolError> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
            match (parent, path.file_name()) {
                (Some(parent), Some(name)) => Ok(canonical_key(parent)?.join(name)),
                _ => Err(ToolError::Execution(format!(
                    "resolve file coordination path {}: {error}",
                    path.display()
                ))),
            }
        }
        Err(error) => Err(ToolError::Execution(format!(
            "resolve file coordination path {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn independent_callers_serialize_the_same_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new/file.txt");
        let held = lock(&path).await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), lock(&path))
                .await
                .is_err()
        );
        let other = lock(&dir.path().join("different.txt")).await.unwrap();
        drop(held);
        let next = lock(&path).await.unwrap();
        drop((other, next));
    }
}
