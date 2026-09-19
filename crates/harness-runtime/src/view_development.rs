//! Filesystem-backed view authoring; previews are immutable revisions, never live paths.

use crate::{
    documents::{revision_of, Documents},
    view_packages::{self, Package},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: String,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u64>,
}
impl Diagnostic {
    fn error(message: impl Into<String>) -> Self {
        Self {
            level: "error".into(),
            message: message.into(),
            file: None,
            line: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    #[serde(default)]
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestRun {
    pub id: String,
    pub digest: String,
    pub status: String,
    pub results: Vec<TestResult>,
    pub snapshot: String,
    pub requested_at: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub source: String,
    pub report_path: String,
    pub active: bool,
    pub paused: bool,
    pub dirty: bool,
    pub mounted: bool,
    #[serde(default)]
    pub generation: u64,
    pub installed_digest: Option<String>,
    pub candidate: Option<Package>,
    pub package: Option<Package>,
    #[serde(default)]
    pub previous: Option<Package>,
    pub diagnostics: Vec<Diagnostic>,
    pub runtime: Vec<Diagnostic>,
    pub test: Option<TestRun>,
}

pub struct Development {
    docs: Documents,
    cache: PathBuf,
    token: Arc<AtomicBool>,
    last_seen: u64,
    validated_digest: Option<String>,
    pub status: Status,
}
impl Development {
    pub fn new(
        docs: Documents,
        session: &str,
        source: &str,
        cache: PathBuf,
    ) -> Result<Self, String> {
        docs.resolve(source).map_err(|e| e.to_string())?;
        let report_path = format!(
            ".oxen-harness/view-dev/{}.json",
            revision_of(format!("{session}\0{source}").as_bytes())
        );
        let mut status = match docs.read(&report_path) {
            Ok(document) => serde_json::from_str::<Status>(&document.content)
                .map_err(|e| format!("read view report {report_path}: {e}"))?,
            Err(crate::documents::DocumentError::Io { source: error, .. })
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                Status {
                    source: source.into(),
                    report_path: report_path.clone(),
                    active: false,
                    paused: false,
                    dirty: false,
                    mounted: false,
                    generation: 0,
                    installed_digest: None,
                    candidate: None,
                    package: None,
                    previous: None,
                    diagnostics: vec![],
                    runtime: vec![],
                    test: None,
                }
            }
            Err(e) => return Err(e.to_string()),
        };
        if status.source != source || status.report_path != report_path {
            return Err("view report belongs to a different authoring context".into());
        }
        status.active = false;
        status.mounted = false;
        status.dirty = false;
        if let Some(test) = &mut status.test {
            if test.status == "pending" {
                test.status = "interrupted".into();
            }
        }
        Ok(Self {
            docs,
            cache,
            token: Arc::new(AtomicBool::new(false)),
            last_seen: 0,
            validated_digest: None,
            status,
        })
    }
    async fn persist(&self) -> Result<(), String> {
        let before = match self.docs.read(&self.status.report_path) {
            Ok(doc) => Some(doc),
            Err(crate::documents::DocumentError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(e) => return Err(e.to_string()),
        };
        self.docs
            .save(
                &self.status.report_path,
                &serde_json::to_string_pretty(&self.status).map_err(|e| e.to_string())?,
                before.as_ref().map(|d| d.revision.as_str()),
            )
            .await
            .map_err(|e| format!("save view development report: {e}"))?;
        Ok(())
    }
    fn inspect(
        &mut self,
    ) -> Result<(Package, std::collections::BTreeMap<String, Vec<u8>>), String> {
        let source = self
            .docs
            .resolve(&self.status.source)
            .map_err(|e| e.to_string())?;
        let (package, files) = view_packages::inspect_files(&source)?;
        if self.validated_digest.as_deref() == Some(&package.digest) {
            return Ok((package, files));
        }
        for (path, bytes) in &files {
            if matches!(
                std::path::Path::new(path)
                    .extension()
                    .and_then(|s| s.to_str()),
                Some("js" | "mjs" | "cjs" | "jsx")
            ) {
                let text = std::str::from_utf8(bytes).map_err(|e| format!("{path}: {e}"))?;
                if let Some(error) = harness_tools::fs::syntax::regression_note(
                    std::path::Path::new(path),
                    None,
                    text,
                ) {
                    return Err(format!("{path}: {error}"));
                }
            }
        }
        self.validated_digest = Some(package.digest.clone());
        Ok((package, files))
    }
    pub async fn check(&mut self) -> Result<Status, String> {
        match self.inspect() {
            Ok((package, _)) => {
                self.status.candidate = Some(package);
                self.status.diagnostics.clear();
            }
            Err(e) => {
                self.status.candidate = None;
                self.status.diagnostics = vec![Diagnostic::error(e)];
            }
        }
        self.persist().await?;
        Ok(self.status.clone())
    }
    pub async fn start(&mut self, digest: &str) -> Result<Status, String> {
        if self.status.active && self.status.dirty {
            return Err("save or retain the preview draft before restarting".into());
        }
        let (package, files) = self.inspect()?;
        if package.digest != digest {
            return Err(
                "view changed after review; check the current files before previewing".into(),
            );
        }
        self.publish(package, files)?;
        self.status.active = true;
        self.status.paused = false;
        self.status.diagnostics.clear();
        self.persist().await?;
        Ok(self.status.clone())
    }
    fn publish(
        &mut self,
        package: Package,
        files: std::collections::BTreeMap<String, Vec<u8>>,
    ) -> Result<(), String> {
        view_packages::cache_assets(&package, &files, &self.cache)?;
        self.token.store(false, Ordering::Relaxed);
        self.token = Arc::new(AtomicBool::new(true));
        self.last_seen = 0;
        self.status.generation = self.status.generation.saturating_add(1);
        self.status.mounted = false;
        self.status.dirty = false;
        self.status.runtime.clear();
        self.status.test = None;
        if self
            .status
            .package
            .as_ref()
            .is_some_and(|current| current.digest != package.digest)
        {
            self.status.previous = self.status.package.clone();
        }
        self.status.candidate = Some(package.clone());
        self.status.package = Some(package);
        Ok(())
    }
    pub async fn refresh(&mut self) -> Result<Status, String> {
        let before = self.status.clone();
        let time = now()?;
        self.status.mounted = self.status.active && time.saturating_sub(self.last_seen) < 4000;
        if let Some(test) = &mut self.status.test {
            if test.status == "pending" && time.saturating_sub(test.requested_at) > 30000 {
                test.status = "failed".into();
                test.results.push(TestResult{name:"Preview response".into(),passed:false,error:Some("The preview did not return a test result within 30 seconds. Check its runtime errors and reopen it.".into())});
            }
        }
        if self.status.active && !self.status.paused {
            match self.inspect() {
                Ok((package, files)) => {
                    self.status.candidate = Some(package.clone());
                    if let Some(current) = &self.status.package {
                        if current.manifest.id != package.manifest.id
                            || current.manifest.permissions != package.manifest.permissions
                        {
                            self.status.diagnostics=vec![Diagnostic::error("Package identity or permissions changed. Review the new manifest and restart preview to grant access.")];
                        } else if current.digest != package.digest {
                            if self.status.dirty {
                                self.status.diagnostics = vec![Diagnostic::error(
                                    "Preview has an unretained draft. Save it before reloading.",
                                )];
                            } else {
                                self.publish(package, files)?;
                                self.status.diagnostics.clear();
                            }
                        } else {
                            self.status.diagnostics.clear();
                        }
                    }
                }
                Err(e) => {
                    self.status.candidate = None;
                    self.status.diagnostics = vec![Diagnostic::error(e)];
                }
            }
        }
        if self.status != before {
            self.persist().await?;
        }
        Ok(self.status.clone())
    }
    pub async fn rollback(&mut self) -> Result<Status, String> {
        if self.status.dirty {
            return Err("save or retain the preview draft before switching revisions".into());
        }
        let previous = self
            .status
            .previous
            .clone()
            .ok_or("no previous preview revision")?;
        let files = view_packages::cached_files(&previous, &self.cache)?;
        let candidate = self.status.candidate.clone();
        self.publish(previous, files)?;
        self.status.candidate = candidate;
        self.status.active = true;
        self.status.paused = true;
        self.status.diagnostics.clear();
        self.persist().await?;
        Ok(self.status.clone())
    }
    pub async fn pause(&mut self, paused: bool) -> Result<Status, String> {
        self.status.paused = paused;
        self.persist().await?;
        Ok(self.status.clone())
    }
    pub async fn stop(&mut self) -> Result<Status, String> {
        self.token.store(false, Ordering::Relaxed);
        self.status.active = false;
        self.status.mounted = false;
        if let Some(test) = &mut self.status.test {
            if test.status == "pending" {
                test.status = "cancelled".into();
            }
        }
        self.persist().await?;
        Ok(self.status.clone())
    }
    pub fn lease(&self) -> Result<(Package, Arc<AtomicBool>), String> {
        if !self.status.active {
            return Err("start a development preview first".into());
        }
        Ok((
            self.status
                .package
                .clone()
                .ok_or("preview package is unavailable")?,
            self.token.clone(),
        ))
    }
    pub async fn request_test(&mut self) -> Result<Status, String> {
        self.refresh().await?;
        if !self.status.mounted {
            return Err("open the live preview before running browser tests".into());
        }
        if self
            .status
            .test
            .as_ref()
            .is_some_and(|test| test.status == "pending")
        {
            return Err("a browser test is already pending".into());
        }
        let package = self.status.package.as_ref().ok_or("no preview package")?;
        self.status.test = Some(TestRun {
            id: format!("{}", now()?),
            digest: package.digest.clone(),
            status: "pending".into(),
            results: vec![],
            snapshot: String::new(),
            requested_at: now()?,
        });
        self.persist().await?;
        Ok(self.status.clone())
    }
    pub async fn install(&mut self, digest: &str) -> Result<Package, String> {
        let (package, _) = self.inspect()?;
        if package.digest != digest {
            return Err(
                "view changed after review; check and test the current revision before installing"
                    .into(),
            );
        }
        let installed = view_packages::install_at(
            &self
                .docs
                .resolve(&self.status.source)
                .map_err(|e| e.to_string())?,
            digest,
            &self.cache,
        )
        .await?;
        self.status.installed_digest = Some(installed.digest.clone());
        self.persist().await?;
        Ok(installed)
    }
    pub async fn bridge(
        &mut self,
        digest: &str,
        action: &str,
        payload: Value,
    ) -> Result<Value, String> {
        if !self.status.active
            || self
                .status
                .package
                .as_ref()
                .is_none_or(|p| p.digest != digest)
        {
            return Err("development preview revision is no longer active".into());
        }
        if payload.to_string().len() > 32768 {
            return Err("preview report exceeds 32 KiB".into());
        }
        match action {
            "poll" => {
                self.last_seen = now()?;
                Ok(json!({"test":self.status.test.as_ref().filter(|t|t.status=="pending")}))
            }
            "ready" => {
                self.last_seen = now()?;
                self.status.mounted = true;
                self.persist().await?;
                Ok(Value::Null)
            }
            "dirty" => {
                self.status.dirty = payload["dirty"].as_bool().unwrap_or(false);
                self.persist().await?;
                Ok(Value::Null)
            }
            "diagnostic" => {
                let mut diagnostic: Diagnostic =
                    serde_json::from_value(payload).map_err(|e| e.to_string())?;
                diagnostic.message = diagnostic.message.chars().take(2000).collect();
                diagnostic.file = diagnostic.file.map(|file| file.chars().take(500).collect());
                diagnostic.level = diagnostic.level.chars().take(16).collect();
                if self.status.runtime.len() >= 100 {
                    self.status.runtime.remove(0);
                }
                self.status.runtime.push(diagnostic);
                self.persist().await?;
                Ok(Value::Null)
            }
            "test_result" => {
                let test = self
                    .status
                    .test
                    .as_mut()
                    .ok_or("no browser test requested")?;
                if payload["id"].as_str() != Some(&test.id) || test.status != "pending" {
                    return Err("browser test request is no longer active".into());
                }
                let results: Vec<TestResult> = serde_json::from_value(payload["results"].clone())
                    .map_err(|e| e.to_string())?;
                if results.is_empty() || results.len() > 50 {
                    return Err("browser test result needs 1 to 50 checks".into());
                }
                test.status = if results.iter().all(|r| r.passed) {
                    "passed"
                } else {
                    "failed"
                }
                .into();
                test.results = results;
                test.snapshot = payload["snapshot"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(8000)
                    .collect();
                self.persist().await?;
                Ok(Value::Null)
            }
            _ => Err("unknown preview report action".into()),
        }
    }
}
impl Drop for Development {
    fn drop(&mut self) {
        self.token.store(false, Ordering::Relaxed);
    }
}
fn now() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis() as u64)
}

pub async fn scaffold(docs: &Documents, source: &str, id: &str, title: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 80
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-'))
    {
        return Err("view id needs lowercase letters, digits, dots or dashes".into());
    }
    if title.trim().is_empty() || title.len() > 160 {
        return Err("view title needs 1 to 160 bytes".into());
    }
    let target = docs.resolve(source).map_err(|e| e.to_string())?;
    let _guard = harness_tools::path_lock::lock(&target)
        .await
        .map_err(|e| e.to_string())?;
    if target.exists() {
        return Err(format!(
            "{source} already exists; scaffold never overwrites a view"
        ));
    }
    let parent = target.parent().ok_or("view source has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let staging = tempfile::tempdir_in(parent).map_err(|e| e.to_string())?;
    let document = format!("data/{id}/document.json");
    let manifest = json!({"api_version":1,"id":id,"title":title,"description":"A view built with your agent","entry":"index.html","file_patterns":[format!("data/{id}/*.json")],"permissions":{"read":[format!("data/{id}/*.json")],"write":[format!("data/{id}/*.json")],"assets":[],"actions":[]}});
    let templates = [
        ("index.html", include_str!("../templates/view/index.html")),
        ("style.css", include_str!("../templates/view/style.css")),
        ("main.js", include_str!("../templates/view/main.js")),
        ("tests.js", include_str!("../templates/view/tests.js")),
        ("AGENTS.md", include_str!("../templates/view/AGENTS.md")),
        (
            "oxen-view.d.ts",
            include_str!("../../../packages/view-sdk/index.d.ts"),
        ),
    ];
    for (name, text) in templates {
        std::fs::write(staging.path().join(name), text)
            .map_err(|e| format!("scaffold {name}: {e}"))?;
    }
    std::fs::write(
        staging.path().join("view.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        staging.path().join("settings.json"),
        json!({"title":title,"source":source,"document":document}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    // Existing data may belong to another view; create-only preserves it.
    if !docs.resolve(&document).map_err(|e| e.to_string())?.exists() {
        docs.save(&document, "{\"text\":\"\"}\n", None)
            .await
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(staging.path(), &target).map_err(|e| format!("create view {source}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn reports_survive_restarts_without_restarting_code_and_sources_keep_separate_reports() {
        let project = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let docs = Documents::new(project.path()).unwrap();
        scaffold(&docs, "views/a", "a", "A").await.unwrap();
        let mut first =
            Development::new(docs.clone(), "chat", "views/a", cache.path().into()).unwrap();
        let digest = first.check().await.unwrap().candidate.unwrap().digest;
        first.start(&digest).await.unwrap();
        first.bridge(&digest,"diagnostic",json!({"level":"error","message":"remember this failure","file":"main.js","line":1})).await.unwrap();
        let report = first.status.report_path.clone();
        drop(first);
        let restored =
            Development::new(docs.clone(), "chat", "views/a", cache.path().into()).unwrap();
        assert!(!restored.status.active);
        assert!(!restored.status.mounted);
        assert_eq!(restored.status.runtime[0].message, "remember this failure");
        assert!(restored.lease().is_err());
        let other = Development::new(docs, "chat", "views/b", cache.path().into()).unwrap();
        assert_ne!(other.status.report_path, report);
    }
    #[tokio::test]
    async fn reload_protects_drafts_and_can_return_to_the_previous_preview_revision() {
        let project = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let docs = Documents::new(project.path()).unwrap();
        scaffold(&docs, "views/demo", "demo", "Demo").await.unwrap();
        let mut dev = Development::new(docs, "chat", "views/demo", cache.path().into()).unwrap();
        let original = dev.check().await.unwrap().candidate.unwrap().digest;
        dev.start(&original).await.unwrap();
        dev.bridge(&original, "dirty", json!({"dirty":true}))
            .await
            .unwrap();
        assert!(dev.start(&original).await.unwrap_err().contains("draft"));
        dev.bridge(&original, "dirty", json!({"dirty":false}))
            .await
            .unwrap();
        let generation = dev.status.generation;
        let old_lease = dev.lease().unwrap().1;
        dev.start(&original).await.unwrap();
        assert!(dev.status.generation > generation);
        assert!(!old_lease.load(Ordering::Relaxed));
        std::fs::write(
            project.path().join("views/demo/style.css"),
            "body { color:red }",
        )
        .unwrap();
        dev.refresh().await.unwrap();
        assert_ne!(dev.status.package.as_ref().unwrap().digest, original);
        dev.rollback().await.unwrap();
        assert_eq!(dev.status.package.as_ref().unwrap().digest, original);
        assert!(dev.status.paused);
        assert_eq!(
            std::fs::read_to_string(project.path().join("views/demo/style.css")).unwrap(),
            "body { color:red }"
        );
    }
    #[tokio::test]
    async fn paused_and_dirty_previews_keep_their_revision_and_install_is_exact() {
        let project = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let docs = Documents::new(project.path()).unwrap();
        scaffold(&docs, "views/demo", "demo", "Demo").await.unwrap();
        let mut dev = Development::new(docs, "chat", "views/demo", cache.path().into()).unwrap();
        let digest = dev.check().await.unwrap().candidate.unwrap().digest;
        dev.start(&digest).await.unwrap();
        let token = dev.lease().unwrap().1;
        dev.bridge(&digest, "dirty", json!({"dirty":true}))
            .await
            .unwrap();
        std::fs::write(
            project.path().join("views/demo/style.css"),
            "body { color:red }",
        )
        .unwrap();
        dev.refresh().await.unwrap();
        assert_eq!(dev.status.package.as_ref().unwrap().digest, digest);
        dev.bridge(&digest, "dirty", json!({"dirty":false}))
            .await
            .unwrap();
        dev.pause(true).await.unwrap();
        dev.refresh().await.unwrap();
        assert_eq!(dev.status.package.as_ref().unwrap().digest, digest);
        assert!(dev
            .install(&digest)
            .await
            .unwrap_err()
            .contains("changed after review"));
        dev.pause(false).await.unwrap();
        dev.refresh().await.unwrap();
        assert!(!token.load(Ordering::Relaxed));
        let next = dev.status.package.as_ref().unwrap().digest.clone();
        dev.install(&next).await.unwrap();
        assert_eq!(dev.status.installed_digest.as_deref(), Some(next.as_str()));
        std::fs::write(project.path().join("views/demo/style.css"), "later").unwrap();
        assert_eq!(
            std::fs::read_to_string(cache.path().join(next).join("style.css")).unwrap(),
            "body { color:red }"
        );
    }
    #[tokio::test]
    async fn scaffold_is_create_only_and_preview_tracks_code_without_expanding_grants() {
        let project = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let docs = Documents::new(project.path()).unwrap();
        scaffold(&docs, "views/notes", "my.notes", "My notes")
            .await
            .unwrap();
        assert!(scaffold(&docs, "views/notes", "my.notes", "overwrite")
            .await
            .is_err());
        let mut dev = Development::new(docs, "chat", "views/notes", cache.path().into()).unwrap();
        let checked = dev.check().await.unwrap();
        let digest = checked.candidate.unwrap().digest;
        dev.start(&digest).await.unwrap();
        let original = dev.status.package.clone().unwrap();
        std::fs::write(
            project.path().join("views/notes/style.css"),
            "body { color:red }",
        )
        .unwrap();
        dev.refresh().await.unwrap();
        assert_ne!(original.digest, dev.status.package.as_ref().unwrap().digest);
        let last_good = dev.status.package.clone().unwrap();
        std::fs::write(
            project.path().join("views/notes/main.js"),
            "const broken = ;",
        )
        .unwrap();
        dev.refresh().await.unwrap();
        assert_eq!(
            last_good.digest,
            dev.status.package.as_ref().unwrap().digest
        );
        assert!(dev
            .status
            .diagnostics
            .iter()
            .any(|d| d.message.contains("syntax")));
        std::fs::write(
            project.path().join("views/notes/main.js"),
            "console.log('repaired')",
        )
        .unwrap();
        let manifest = project.path().join("views/notes/view.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        value["permissions"]["read"] = serde_json::json!(["**"]);
        std::fs::write(manifest, value.to_string()).unwrap();
        dev.refresh().await.unwrap();
        assert_eq!(
            last_good.digest,
            dev.status.package.as_ref().unwrap().digest
        );
        assert!(dev
            .status
            .diagnostics
            .iter()
            .any(|d| d.message.contains("permissions")));
    }
    #[tokio::test]
    async fn reports_are_revision_scoped_and_tests_require_an_actual_renderer() {
        let project = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let docs = Documents::new(project.path()).unwrap();
        scaffold(&docs, "views/demo", "demo", "Demo").await.unwrap();
        let mut dev =
            Development::new(docs.clone(), "chat", "views/demo", cache.path().into()).unwrap();
        let digest = dev.check().await.unwrap().candidate.unwrap().digest;
        dev.start(&digest).await.unwrap();
        assert!(dev.request_test().await.unwrap_err().contains("preview"));
        dev.bridge(&digest, "ready", serde_json::json!({}))
            .await
            .unwrap();
        dev.request_test().await.unwrap();
        let id = dev.status.test.as_ref().unwrap().id.clone();
        assert!(dev
            .bridge("old", "diagnostic", serde_json::json!({"message":"stale"}))
            .await
            .is_err());
        dev.bridge(&digest,"test_result",serde_json::json!({"id":id,"results":[{"name":"works","passed":false,"error":"bad output"}],"snapshot":"Notes"})).await.unwrap();
        assert_eq!(dev.status.test.as_ref().unwrap().status, "failed");
        assert!(docs
            .read(&dev.status.report_path)
            .unwrap()
            .content
            .contains("bad output"));
        let token = dev.lease().unwrap().1;
        dev.stop().await.unwrap();
        assert!(!token.load(std::sync::atomic::Ordering::Relaxed));
    }
}
