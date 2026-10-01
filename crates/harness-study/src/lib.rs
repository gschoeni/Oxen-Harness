//! The codebase study game's backend — "Trail of Understanding".
//!
//! A developer who offloads the thinking to an agent stops learning the
//! code. This crate quizzes them on the project their chat is rooted in and
//! keeps a per-project, spaced-repetition record of what they understand:
//!
//! - [`territories`] — carve a workspace into the regions a journey visits
//!   (crates, feature directories, the docs).
//! - [`progress`] — the per-territory mastery model: every answer nudges a
//!   score that halves every two weeks without practice, and the profile a
//!   host shows is derived from it at read time.
//! - [`bank`] — the per-project cache of generated questions, so a replay
//!   costs nothing and a missed question can come back.
//! - [`material`] — what a question is written from: a territory's files,
//!   the recent git changes, or the files the session's agent just touched.
//! - [`generate`] / [`grade`] — the prompts a model answers with JSON, and
//!   the lenient parsers that read it back.
//! - [`StudyService`] — the host-facing orchestration: a profile, a batch of
//!   questions for a mode, and an answer's grade plus its recording.
//! - [`UnderstandingTool`] — the `understanding` tool, so the coding agent
//!   can read the same profile and decide how much to explain.
//!
//! The model call itself is behind the [`Completer`] trait: hosts implement
//! it with a detached agent on the `study` role; tests hand in canned JSON.
//! Everything lives under `~/.oxen-harness/study/<project-key>/` — personal
//! progress, never written into the repository.

pub mod bank;
pub mod generate;
pub mod grade;
pub mod material;
pub mod progress;
mod service;
pub mod territories;
mod tool;

use std::path::PathBuf;

pub use service::{BatchContext, StudyService, DEFAULT_BATCH, MAX_BATCH};
pub use tool::{UnderstandingTool, UNDERSTANDING_TOOL};

/// Errors from the study backend, each naming the failed operation.
#[derive(Debug, thiserror::Error)]
pub enum StudyError {
    #[error("study: {op} {path}: {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Config(#[from] harness_config::ConfigError),
    #[error("study: {op}: {source}")]
    Json {
        op: &'static str,
        #[source]
        source: serde_json::Error,
    },
    /// The model call failed, or its reply held no usable questions.
    #[error("study model ({model}): {detail}")]
    Model { model: String, detail: String },
    /// The request itself was malformed (an unknown mode, an empty answer).
    #[error("study: {0}")]
    Invalid(String),
    /// The chosen mode has nothing to ask about right now (a clean tree for
    /// fresh tracks, no tool activity for ride-along).
    #[error("nothing to study: {0}")]
    Nothing(String),
    #[error("study: no question with id {0}")]
    UnknownQuestion(String),
}

impl StudyError {
    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            op,
            path: path.into(),
            source,
        }
    }
}

/// What a batch of questions is drawn for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StudyMode {
    /// The whole workspace, territory by territory, weakest first.
    Expedition,
    /// The recent git changes: uncommitted work, else the last commits.
    FreshTracks,
    /// The files the session's agent read or edited recently.
    RideAlong,
    /// Territories whose mastery has faded, and questions missed before.
    Review,
}

impl StudyMode {
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "expedition" => Some(Self::Expedition),
            "fresh_tracks" | "fresh" => Some(Self::FreshTracks),
            "ride_along" | "ride" => Some(Self::RideAlong),
            "review" => Some(Self::Review),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expedition => "expedition",
            Self::FreshTracks => "fresh_tracks",
            Self::RideAlong => "ride_along",
            Self::Review => "review",
        }
    }
}

/// One completed model call.
#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub text: String,
    /// Prompt plus completion tokens the call cost.
    pub tokens_used: usize,
}

/// The one model seam: a tool-less completion on the study model. Hosts
/// implement it with a detached agent; tests return canned replies.
#[async_trait::async_trait]
pub trait Completer: Send + Sync {
    async fn complete(&self, system: &str, user: &str) -> Result<Completion, StudyError>;
    /// The model id the completions run on, for the batch's provenance.
    fn model(&self) -> String;
}

/// Unix seconds now.
pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A short stable hex digest, for ids and project keys.
pub(crate) fn short_hash(input: &str, len: usize) -> String {
    use sha2::{Digest, Sha256};
    let hex = format!("{:x}", Sha256::digest(input.as_bytes()));
    hex[..len.min(hex.len())].to_string()
}
