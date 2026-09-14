//! A lane owns its registration and durable result until it settles.

use std::sync::Arc;

use harness_compress::CcrStore;
use harness_store::HistoryStore;

use crate::fleet::{LaneStop, SubagentOutcome};
use crate::lane::{lane_budget, AgentTree, SubagentResult};
use crate::worktree::{LaneWorktree, WorktreeSnapshot};
use crate::AgentError;

pub const WORKSPACE_STATE: &str = "lane_workspace";

pub(crate) struct LaneLifecycle {
    pub id: String,
    pub label: String,
    pub fleet: String,
    pub tree: Arc<AgentTree>,
    pub store: Arc<HistoryStore>,
    pub workspace: Option<Arc<LaneWorktree>>,
    pub spill: Option<Arc<CcrStore>>,
    pub model: String,
    pub started: std::time::Instant,
    pub completed: bool,
}

impl LaneLifecycle {
    fn save(&self, outcome: &SubagentOutcome, count: usize) -> Result<SubagentResult, AgentError> {
        let mut record = SubagentResult::from_outcome(
            outcome,
            &self.fleet,
            lane_budget(count),
            self.spill.as_deref(),
        );
        record.model = self.model.clone();
        record.elapsed_ms = self.started.elapsed().as_millis() as u64;
        if let Some(workspace) = &self.workspace {
            let snapshot = WorktreeSnapshot::capture(workspace, &self.id)?;
            if !snapshot.patch.is_empty() {
                record.patch = self.spill.as_ref().map(|s| s.put(&snapshot.patch));
                record.has_patch = true;
            }
            self.store
                .save_session_state(&self.id, WORKSPACE_STATE, &snapshot)?;
        }
        self.store
            .save_session_state(&self.id, harness_store::LANE_STATE, &record)?;
        Ok(record)
    }

    pub fn finish(mut self, outcome: &mut SubagentOutcome, count: usize) {
        match self.save(outcome, count) {
            Err(error) => {
                if let Some(workspace) = &self.workspace {
                    workspace.preserve();
                }
                outcome.result = Err(error);
            }
            Ok(record) => {
                outcome.record = Some(record);
                self.completed = true;
            }
        }
        self.tree.finish_lane(&self.id);
    }
}

impl Drop for LaneLifecycle {
    fn drop(&mut self) {
        if !self.completed {
            let outcome = SubagentOutcome {
                record: None,
                label: self.label.clone(),
                session: self.id.clone(),
                result: Ok(self
                    .store
                    .last_assistant_text(&self.id)
                    .ok()
                    .flatten()
                    .unwrap_or_default()),
                structured: None,
                tokens_used: 0,
                rounds: 0,
                stopped: Some(LaneStop::Cancelled),
                denied: Vec::new(),
            };
            if let Err(error) = self.save(&outcome, 1) {
                if let Some(workspace) = &self.workspace {
                    workspace.preserve();
                }
                tracing::error!(lane = %self.id, %error, "could not save interrupted agent");
            }
        }
        self.tree.finish_lane(&self.id);
    }
}
