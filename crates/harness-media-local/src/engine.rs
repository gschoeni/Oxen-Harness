//! The engine seam: what a runtime implements to run local models.
//!
//! An engine turns one prompt and one set of resolved settings into one
//! encoded output. It owns everything runtime-specific: starting a server
//! or loading a library, keeping weights resident between jobs, mapping
//! weight-file roles to its own inputs, reporting progress, honoring cancel,
//! and letting go of memory when asked. The service in front of it handles
//! the catalog, the store, queueing, and cancellation bookkeeping, so a new
//! engine — an in-house Rust/Metal one, a video engine — is one type plus a
//! line in [`crate::service::LocalMediaService::with_default_engines`], and
//! catalog entries opt into it by naming its [`MediaEngine::id`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use harness_media::{LocalMediaError, LocalOutput, LocalProgress};
use tokio_util::sync::CancellationToken;

use crate::catalog::{ModelSpec, Settings};

/// One output for an engine to render.
pub struct EngineRequest<'a> {
    pub spec: &'a ModelSpec,
    /// Each weight role's file on disk.
    pub weights: &'a BTreeMap<String, PathBuf>,
    pub prompt: &'a str,
    pub settings: &'a Settings,
    /// Reference images, in prompt order.
    pub refs: &'a [PathBuf],
}

/// Where an engine is, for a status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineStatus {
    /// Ready to run; holds the runtime's version.
    Ready(String),
    /// Missing a runtime the user can install; holds how.
    NotInstalled(String),
}

/// A runtime that renders local models.
#[async_trait]
pub trait MediaEngine: Send + Sync {
    /// The id catalog entries name in `engine` (`sd.cpp`).
    fn id(&self) -> &'static str;

    /// Whether the runtime is present.
    fn status(&self) -> EngineStatus;

    /// Render one output. The service queues jobs, so an engine sees one at
    /// a time and may rely on that; `cancel` can fire at any point and must
    /// end the job promptly with [`LocalMediaError::Cancelled`], and a
    /// dropped future must not leave the runtime rendering.
    async fn render(
        &self,
        req: EngineRequest<'_>,
        progress: &(dyn Fn(LocalProgress) + Send + Sync),
        cancel: &CancellationToken,
    ) -> Result<LocalOutput, LocalMediaError>;

    /// Release whatever the engine holds in memory (an idle timeout, the app
    /// quitting, a user freeing memory for something else).
    async fn unload(&self);
}
