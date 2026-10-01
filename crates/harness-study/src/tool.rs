//! `understanding` — the coding agent's window onto the player's study
//! profile, so it can decide how much to explain in a region the developer
//! doesn't know well yet.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use harness_tools::{CallContext, ToolError, TypedTool};
use serde::Deserialize;

use crate::StudyService;

pub const UNDERSTANDING_TOOL: &str = "understanding";

/// Arguments to `understanding`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UnderstandingArgs {
    /// Narrow to one territory (a directory relative to the workspace root,
    /// e.g. `crates/harness-agent`); omit for the whole project.
    #[serde(default)]
    pub territory: Option<String>,
}

pub struct UnderstandingTool {
    root: PathBuf,
    /// Where the study files live when not under the config directory.
    study_dir: Option<PathBuf>,
}

impl UnderstandingTool {
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            root: workspace_root.to_path_buf(),
            study_dir: None,
        }
    }

    /// Read the study files from `dir` instead of the config directory
    /// (tests, which must not touch the real one).
    pub fn with_study_dir(mut self, dir: &Path) -> Self {
        self.study_dir = Some(dir.to_path_buf());
        self
    }

    fn service(&self) -> Result<StudyService, crate::StudyError> {
        match &self.study_dir {
            Some(dir) => StudyService::open_at(&self.root, dir),
            None => StudyService::open(&self.root),
        }
    }
}

#[async_trait]
impl TypedTool for UnderstandingTool {
    const NAME: &'static str = UNDERSTANDING_TOOL;
    type Args = UnderstandingArgs;

    fn description(&self) -> &str {
        "How well the user understands this codebase, from the study game they play while you \
         work: a level, and mastery per region (0-100%). Consult it before explaining a change \
         in a region they know poorly, and keep explanations short where mastery is high."
    }

    async fn run(&self, args: UnderstandingArgs, _call: &CallContext) -> Result<String, ToolError> {
        let profile = self
            .service()
            .and_then(|s| s.profile())
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        if profile.answered == 0 {
            return Ok(
                "The user hasn't answered any study questions for this project yet, so \
                       there is no understanding profile. Assume they know it as well as any \
                       new contributor would."
                    .into(),
            );
        }
        let pct = |m: f32| format!("{:.0}%", m * 100.0);
        if let Some(want) = args.territory.as_deref().map(|t| t.trim_matches('/')) {
            let Some(t) = profile
                .territories
                .iter()
                .find(|t| t.id == want || t.name == want)
            else {
                return Ok(format!(
                    "No territory `{want}` in this project. Known: {}",
                    profile
                        .territories
                        .iter()
                        .map(|t| t.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            };
            return Ok(format!(
                "{}: mastery {} ({} answered, {} fully correct{}).",
                t.id,
                pct(t.mastery),
                t.answered,
                t.correct,
                if t.faded > 0.3 { ", fading" } else { "" }
            ));
        }
        let mut rows = profile.territories.clone();
        rows.sort_by(|a, b| a.mastery.total_cmp(&b.mastery));
        let mut out = format!(
            "Understanding of this project: level {} ({:.0}%), {} questions answered. \
             Regions, weakest first:\n",
            profile.level, profile.understanding, profile.answered
        );
        for t in rows {
            out.push_str(&format!(
                "- {} — {}{}\n",
                t.id,
                pct(t.mastery),
                if t.answered == 0 {
                    " (unexplored)"
                } else if t.faded > 0.3 {
                    " (fading)"
                } else {
                    ""
                }
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::{Attempt, Progress, Verdict};

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("README.md"), "# x").unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "fn x() {}").unwrap();
        dir
    }

    #[tokio::test]
    async fn reports_no_profile_before_any_answers() {
        let ws = workspace();
        let study = tempfile::tempdir().unwrap();
        let tool = UnderstandingTool::new(ws.path()).with_study_dir(study.path());
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        assert!(out.contains("hasn't answered"));
    }

    #[tokio::test]
    async fn lists_regions_weakest_first_and_narrows_to_one() {
        let ws = workspace();
        let study = tempfile::tempdir().unwrap();
        let mut progress = Progress::default();
        progress.record(
            "src",
            Attempt {
                at: crate::now_secs(),
                verdict: Verdict::Full,
                question_id: "q".into(),
                hint_used: false,
                kind: None,
            },
        );
        progress.save(study.path()).unwrap();
        let tool = UnderstandingTool::new(ws.path()).with_study_dir(study.path());
        let all = tool.invoke(serde_json::json!({})).await.unwrap();
        let docs = all.find("- docs").unwrap();
        let src = all.find("- src").unwrap();
        assert!(docs < src, "weakest (unexplored docs) first:\n{all}");
        assert!(all.contains("(unexplored)"));
        let one = tool
            .invoke(serde_json::json!({"territory": "src"}))
            .await
            .unwrap();
        assert!(one.starts_with("src: mastery 35%"));
        let missing = tool
            .invoke(serde_json::json!({"territory": "nope"}))
            .await
            .unwrap();
        assert!(missing.contains("No territory `nope`"));
    }
}
