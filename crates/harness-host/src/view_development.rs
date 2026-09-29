//! Session-owned authoring operations shared by the agent, desktop and HTTP hosts.
use crate::workbench::Workbench;
use harness_runtime::{
    view_development::{self, Development},
    view_packages::{self, Package},
};
use harness_tools::views::{DevelopAction, DevelopViewArgs};
use serde_json::{json, Value};
use std::sync::{atomic::AtomicBool, Arc};

impl Workbench {
    pub async fn develop(&self, args: DevelopViewArgs) -> Result<Value, String> {
        self.features.require_workbench_customization()?;
        use DevelopAction::*;
        if !matches!(args.action, Check | Status) {
            self.allow_action("develop_view").await?;
        }
        self.docs.resolve(&args.source).map_err(|e| e.to_string())?;
        let mut state = self.development.lock().await;
        let changed = state
            .as_ref()
            .is_none_or(|dev| dev.status.source != args.source);
        if changed {
            if matches!(
                args.action,
                Status | Test | Install | Pause | Resume | Rollback | Stop
            ) {
                return Err("open or check this view source first".into());
            }
            if let Some(old) = state.as_mut() {
                old.stop().await?;
            }
            *state = Some(Development::new(
                self.docs.clone(),
                &self.session,
                &args.source,
                view_packages::root()?,
            )?);
        }
        let dev = state
            .as_mut()
            .ok_or("view development context is unavailable")?;
        let status = match args.action {
            Scaffold => {
                view_development::scaffold(
                    &self.docs,
                    &args.source,
                    args.id.as_deref().ok_or("scaffold needs id")?,
                    args.title.as_deref().ok_or("scaffold needs title")?,
                )
                .await?;
                dev.check().await?
            }
            Check => dev.check().await?,
            Preview => {
                dev.start(
                    args.digest
                        .as_deref()
                        .ok_or("check the package first and pass its digest")?,
                )
                .await?
            }
            Status => dev.refresh().await?,
            Test => dev.request_test().await?,
            Pause => dev.pause(true).await?,
            Resume => dev.pause(false).await?,
            Rollback => dev.rollback().await?,
            Stop => dev.stop().await?,
            Install => {
                let package = dev
                    .install(
                        args.digest
                            .as_deref()
                            .ok_or("installation needs the reviewed digest")?,
                    )
                    .await?;
                return Ok(json!({"installed":package,"source":args.source}));
            }
        };
        if matches!(args.action, Scaffold | Preview) {
            self.sink.emit(harness_protocol::ProtocolEvent::ViewOpen {
                session: self.session.clone(),
                view: "view-studio".into(),
                path: Some(args.source),
            });
        }
        Ok(json!(status))
    }
    pub async fn preview_lease(&self) -> Result<(Package, Arc<AtomicBool>), String> {
        self.features.require_workbench_customization()?;
        self.development
            .lock()
            .await
            .as_ref()
            .ok_or("no development preview")?
            .lease()
    }
    pub async fn preview_report(
        &self,
        digest: &str,
        action: &str,
        payload: Value,
    ) -> Result<Value, String> {
        self.development
            .lock()
            .await
            .as_mut()
            .ok_or("no development preview")?
            .bridge(digest, action, payload)
            .await
    }
    pub async fn view_state(&self, id: &str, value: Option<Value>) -> Result<Value, String> {
        let key =
            harness_runtime::documents::revision_of(format!("{}\0{id}", self.session).as_bytes());
        let path = format!(".oxen-harness/view-state/{key}.json");
        let before = match self.docs.read(&path) {
            Ok(doc) => Some(doc),
            Err(harness_runtime::documents::DocumentError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(e) => return Err(e.to_string()),
        };
        match value {
            None => before
                .map(|doc| serde_json::from_str(&doc.content).map_err(|e| e.to_string()))
                .transpose()
                .map(|v| v.unwrap_or(Value::Null)),
            Some(value) => {
                let content = serde_json::to_string(&value).map_err(|e| e.to_string())?;
                if content.len() > 65536 {
                    return Err("retained view state exceeds 64 KiB".into());
                }
                self.docs
                    .save(
                        &path,
                        &content,
                        before.as_ref().map(|doc| doc.revision.as_str()),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
        }
    }
}
