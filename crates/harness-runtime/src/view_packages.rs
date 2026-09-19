//! Immutable local view packages. Installation copies assets; it runs no code.

use crate::documents::Documents;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const MAX_BYTES: usize = 20 * 1024 * 1024;
const MAX_FILES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub api_version: u32,
    pub id: String,
    pub title: String,
    pub description: String,
    pub entry: String,
    #[serde(default)]
    pub file_patterns: Vec<String>,
    pub permissions: Permissions,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub manifest: Manifest,
    pub digest: String,
}

impl Permissions {
    pub fn allows(&self, operation: &str, path: &str) -> bool {
        if !relative(path) {
            return false;
        }
        let patterns = match operation {
            "read" => &self.read,
            "save" => &self.write,
            "asset" => &self.assets,
            _ => return false,
        };
        patterns.iter().any(|p| {
            globset::GlobBuilder::new(p)
                .literal_separator(true)
                .build()
                .is_ok_and(|g| g.compile_matcher().is_match(path))
        })
    }
    pub fn action(&self, action: &str) -> bool {
        self.actions.iter().any(|a| a == action)
    }
}

fn relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn collect(
    root: &Path,
    relative_dir: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    total: &mut usize,
) -> Result<(), String> {
    if relative_dir.components().count() > 16 {
        return Err("view package directories exceed 16 levels".into());
    }
    for entry in
        std::fs::read_dir(root.join(relative_dir)).map_err(|e| format!("read view assets: {e}"))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "view packages cannot contain symlinks: {}",
                entry.path().display()
            ));
        }
        let path = relative_dir.join(entry.file_name());
        if kind.is_dir() {
            collect(root, &path, files, total)?;
        } else if kind.is_file() {
            if files.len() >= MAX_FILES {
                return Err("view package exceeds 512 assets".into());
            }
            let size = entry.metadata().map_err(|e| e.to_string())?.len();
            if size > MAX_BYTES as u64 || (*total as u64).saturating_add(size) > MAX_BYTES as u64 {
                return Err("view package exceeds 20 MiB".into());
            }
            let name = path
                .to_str()
                .ok_or("asset path is not UTF-8")?
                .replace('\\', "/");
            if !relative(&name) {
                return Err("invalid view asset path".into());
            }
            let bytes = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
            *total += bytes.len();
            if *total > MAX_BYTES {
                return Err("view assets grew beyond 20 MiB during inspection".into());
            }
            files.insert(name, bytes);
        } else {
            return Err("view assets must be regular files".into());
        }
    }
    Ok(())
}

pub(crate) fn inspect_files(source: &Path) -> Result<(Package, BTreeMap<String, Vec<u8>>), String> {
    if std::fs::symlink_metadata(source)
        .map_err(|e| e.to_string())?
        .is_symlink()
    {
        return Err("view package root cannot be a symlink".into());
    }
    let mut files = BTreeMap::new();
    collect(source, Path::new(""), &mut files, &mut 0)?;
    let manifest: Manifest = serde_json::from_slice(
        files
            .get("view.json")
            .ok_or("view package needs view.json")?,
    )
    .map_err(|e| format!("invalid view manifest: {e}"))?;
    if manifest.api_version != 1 {
        return Err(format!("unsupported view API {}", manifest.api_version));
    }
    if manifest.id.is_empty()
        || manifest.id.len() > 80
        || !manifest
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
    {
        return Err("view id must use lowercase letters, digits, dots or dashes".into());
    }
    if !relative(&manifest.entry)
        || !files.contains_key(&manifest.entry)
        || !manifest.entry.ends_with(".html")
    {
        return Err("entry must name an HTML asset in the package".into());
    }
    for patterns in [
        &manifest.permissions.read,
        &manifest.permissions.write,
        &manifest.permissions.assets,
        &manifest.file_patterns,
    ] {
        if patterns.len() > 64 {
            return Err("too many view path patterns".into());
        }
        for pattern in patterns {
            if pattern.len() > 1024 || !relative(pattern) {
                return Err(format!("invalid view path pattern {pattern}"));
            }
            globset::Glob::new(pattern).map_err(|e| format!("invalid view path pattern: {e}"))?;
        }
    }
    if manifest
        .permissions
        .actions
        .iter()
        .any(|a| a != "workflow.run")
    {
        return Err("unsupported view action capability".into());
    }
    let mut hash = Sha256::new();
    for (path, bytes) in &files {
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok((
        Package {
            manifest,
            digest: format!("{:x}", hash.finalize()),
        },
        files,
    ))
}

/// Limits a manifest must meet to be previewed or installed. They are
/// checked when a package is inspected for review, never when an installed
/// revision is loaded back — so tightening one later can't orphan a view
/// that was approved under the old rules.
pub(crate) fn check_manifest_limits(manifest: &Manifest) -> Result<(), String> {
    if manifest.title.trim().is_empty()
        || manifest.title.len() > 160
        || manifest.description.len() > 2000
    {
        return Err("view title needs 1 to 160 bytes and description at most 2000 bytes".into());
    }
    Ok(())
}

pub fn inspect(source: &Path) -> Result<Package, String> {
    let (package, _) = inspect_files(source)?;
    check_manifest_limits(&package.manifest)?;
    Ok(package)
}
pub fn root() -> Result<PathBuf, String> {
    Ok(harness_config::paths::base_dir()
        .map_err(|e| e.to_string())?
        .join("views"))
}

pub fn installed() -> Result<Vec<Package>, String> {
    installed_at(&root()?)
}
fn installed_at(root: &Path) -> Result<Vec<Package>, String> {
    if !root.exists() {
        return Ok(vec![]);
    }
    let docs = Documents::new(root).map_err(|e| e.to_string())?;
    let registry = match docs.read("installed.json") {
        Ok(doc) => {
            serde_json::from_str(&doc.content).map_err(|e| format!("read installed views: {e}"))?
        }
        Err(crate::documents::DocumentError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            vec![]
        }
        Err(e) => return Err(e.to_string()),
    };
    Ok(registry)
}

pub async fn install(source: &Path, approved_digest: &str) -> Result<Package, String> {
    install_at(source, approved_digest, &root()?).await
}
pub(crate) async fn install_at(
    source: &Path,
    approved_digest: &str,
    root: &Path,
) -> Result<Package, String> {
    let (package, files) = inspect_files(source)?;
    check_manifest_limits(&package.manifest)?;
    if package.digest != approved_digest {
        return Err("view assets changed after review; inspect and approve the new package".into());
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let docs = Documents::new(root).map_err(|e| e.to_string())?;
    cache_assets(&package, &files, root)?;
    let previous = match docs.read("installed.json") {
        Ok(doc) => Some(doc),
        Err(crate::documents::DocumentError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            None
        }
        Err(e) => return Err(e.to_string()),
    };
    let mut packages: Vec<Package> = match &previous {
        Some(doc) => serde_json::from_str(&doc.content).map_err(|e| e.to_string())?,
        None => vec![],
    };
    packages.retain(|p| p.manifest.id != package.manifest.id);
    packages.push(package.clone());
    docs.save(
        "installed.json",
        &serde_json::to_string_pretty(&packages).map_err(|e| e.to_string())?,
        previous.as_ref().map(|d| d.revision.as_str()),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(package)
}

/// Cache the exact inspected bytes. Preview and installation share one asset format.
pub(crate) fn cache_assets(
    package: &Package,
    files: &BTreeMap<String, Vec<u8>>,
    root: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("create view cache: {e}"))?;
    let docs = Documents::new(root).map_err(|e| e.to_string())?;
    let destination = docs.resolve(&package.digest).map_err(|e| e.to_string())?;
    if !destination.exists() {
        let staging = tempfile::tempdir_in(root).map_err(|e| e.to_string())?;
        for (path, bytes) in files {
            let target = staging.path().join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(target, bytes).map_err(|e| e.to_string())?;
        }
        std::fs::rename(staging.path(), &destination).map_err(|e| e.to_string())?;
    }
    let cached = Documents::new(destination).map_err(|e| e.to_string())?;
    verify_assets(&cached, package)
}

pub async fn remove(id: &str) -> Result<(), String> {
    let docs = Documents::new(root()?).map_err(|e| e.to_string())?;
    let previous = docs.read("installed.json").map_err(|e| e.to_string())?;
    let mut packages: Vec<Package> =
        serde_json::from_str(&previous.content).map_err(|e| e.to_string())?;
    packages.retain(|p| p.manifest.id != id);
    docs.save(
        "installed.json",
        &serde_json::to_string_pretty(&packages).map_err(|e| e.to_string())?,
        Some(&previous.revision),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub type AssetFiles = std::collections::BTreeMap<String, Vec<u8>>;

/// Load one verified revision into the native lease. Later reads cannot observe
/// modified cache files or repeatedly hash a large bundle for each asset.
pub fn asset_files(package: &Package) -> Result<AssetFiles, String> {
    cached_files(package, &root()?)
}

pub(crate) fn cached_files(package: &Package, root: &Path) -> Result<AssetFiles, String> {
    if package.digest.len() != 64 || !package.digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid view digest".into());
    }
    let docs = Documents::new(root).map_err(|e| e.to_string())?;
    let path = docs.resolve(&package.digest).map_err(|e| e.to_string())?;
    let (actual, files) = inspect_files(&path)?;
    if actual != *package {
        return Err("installed view assets or manifest no longer match the approved hash; reinstall the package".into());
    }
    Ok(files)
}

fn verify_assets(assets: &Documents, package: &Package) -> Result<(), String> {
    if inspect_files(assets.root())?.0 != *package {
        return Err(
            "installed view assets no longer match the approved hash; reinstall the package".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn package(dir: &Path) {
        std::fs::write(dir.join("view.json"),r#"{"api_version":1,"id":"test.notes","title":"Notes","description":"Test","entry":"index.html","permissions":{"read":["notes/**"],"write":["notes/**"]}}"#).unwrap();
        std::fs::write(dir.join("index.html"), "<p>Hello</p>").unwrap();
    }
    #[tokio::test]
    async fn a_package_installed_under_older_limits_still_mounts() {
        let source = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let description = "x".repeat(2300);
        std::fs::write(source.path().join("view.json"),format!(r#"{{"api_version":1,"id":"test.old","title":"Old","description":"{description}","entry":"index.html","permissions":{{"read":[],"write":[]}}}}"#)).unwrap();
        std::fs::write(source.path().join("index.html"), "<p>Old</p>").unwrap();
        // Installed before the description limit existed: the cache holds
        // its exact bytes, approved as they were.
        let (package, files) = inspect_files(source.path()).unwrap();
        cache_assets(&package, &files, root.path()).unwrap();
        assert_eq!(cached_files(&package, root.path()).unwrap().len(), 2);
        // The same folder is refused for a fresh review or install today.
        assert!(inspect(source.path()).unwrap_err().contains("2000 bytes"));
        assert!(install_at(source.path(), &package.digest, root.path())
            .await
            .unwrap_err()
            .contains("2000 bytes"));
    }
    #[tokio::test]
    async fn install_requires_review_of_exact_assets_and_survives_source_changes() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        package(source.path());
        let reviewed = inspect(source.path()).unwrap();
        std::fs::write(source.path().join("index.html"), "changed").unwrap();
        assert!(install_at(source.path(), &reviewed.digest, target.path())
            .await
            .unwrap_err()
            .contains("changed after review"));
        let reviewed = inspect(source.path()).unwrap();
        let installed = install_at(source.path(), &reviewed.digest, target.path())
            .await
            .unwrap();
        std::fs::write(source.path().join("index.html"), "later").unwrap();
        assert_eq!(
            std::fs::read_to_string(target.path().join(&installed.digest).join("index.html"))
                .unwrap(),
            "changed"
        );
        assert_eq!(installed_at(target.path()).unwrap().len(), 1);
        assert!(installed
            .manifest
            .permissions
            .allows("save", "notes/a.json"));
        assert!(!installed
            .manifest
            .permissions
            .allows("save", "notes/../.env"));
        assert!(!installed.manifest.permissions.allows("read", "secrets/key"));
    }
    #[cfg(unix)]
    #[test]
    fn symlink_assets_are_rejected() {
        let source = tempfile::tempdir().unwrap();
        package(source.path());
        std::os::unix::fs::symlink("/etc/passwd", source.path().join("escape")).unwrap();
        assert!(inspect(source.path()).unwrap_err().contains("symlinks"));
    }
    #[test]
    fn permission_patterns_do_not_expand_single_star_across_directories() {
        let permissions = Permissions {
            read: vec!["notes/*.json".into()],
            ..Default::default()
        };
        assert!(permissions.allows("read", "notes/a.json"));
        assert!(!permissions.allows("read", "notes/private/a.json"));
    }
    #[test]
    fn loaded_assets_remain_immutable_and_manifest_metadata_must_match() {
        let source = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        package(source.path());
        let (approved, files) = inspect_files(source.path()).unwrap();
        cache_assets(&approved, &files, cache.path()).unwrap();
        let snapshot = cached_files(&approved, cache.path()).unwrap();
        let mut altered = approved.clone();
        altered.manifest.permissions.read.push("secrets/**".into());
        assert!(cached_files(&altered, cache.path())
            .unwrap_err()
            .contains("approved hash"));
        std::fs::write(
            cache.path().join(&approved.digest).join("index.html"),
            "modified after mount",
        )
        .unwrap();
        assert_eq!(snapshot["index.html"], b"<p>Hello</p>");
        assert!(cached_files(&approved, cache.path())
            .unwrap_err()
            .contains("approved hash"));
    }
    #[test]
    fn changed_installed_assets_cannot_execute_under_an_old_approval() {
        let dir = tempfile::tempdir().unwrap();
        package(dir.path());
        let approved = inspect(dir.path()).unwrap();
        let docs = Documents::new(dir.path()).unwrap();
        verify_assets(&docs, &approved).unwrap();
        std::fs::write(dir.path().join("index.html"), "changed executable").unwrap();
        assert!(verify_assets(&docs, &approved)
            .unwrap_err()
            .contains("approved hash"));
    }
}
