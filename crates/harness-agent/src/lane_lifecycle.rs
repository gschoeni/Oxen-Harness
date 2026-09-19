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
    /// The wallet the lane spends from; released on every exit path.
    pub budget: Arc<crate::tree::TreeBudget>,
    pub store: Arc<HistoryStore>,
    pub workspace: Option<Arc<LaneWorktree>>,
    pub spill: Option<Arc<CcrStore>>,
    pub model: String,
    pub started: std::time::Instant,
    pub completed: bool,
}

impl LaneLifecycle {
    fn record(&self, outcome: &SubagentOutcome, count: usize) -> SubagentResult {
        let mut record = SubagentResult::from_outcome(
            outcome,
            &self.fleet,
            lane_budget(count),
            self.spill.as_deref(),
        );
        record.model = self.model.clone();
        record.elapsed_ms = self.started.elapsed().as_millis() as u64;
        record
    }

    fn save(&self, outcome: &SubagentOutcome, count: usize) -> Result<SubagentResult, AgentError> {
        let mut record = self.record(outcome, count);
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
            Ok(record) => outcome.record = Some(record),
            Err(error) => {
                let recovery = if let Some(workspace) = &self.workspace {
                    workspace.preserve();
                    format!(
                        "; recover the retained checkout at {}",
                        workspace.path().display()
                    )
                } else {
                    String::new()
                };
                outcome.result = Err(AgentError::Io(std::io::Error::other(format!(
                    "could not save agent result: {error}{recovery}"
                ))));
                let record = self.record(outcome, count);
                match self
                    .store
                    .save_session_state(&self.id, harness_store::LANE_STATE, &record)
                {
                    Ok(()) => outcome.record = Some(record),
                    Err(error) => {
                        tracing::error!(lane = %self.id, %error, "could not persist agent failure")
                    }
                }
            }
        }
        // Drop releases this registration exactly once, after persistence.
        self.completed = true;
    }
}

impl Drop for LaneLifecycle {
    fn drop(&mut self) {
        if !self.completed {
            let panicked = std::thread::panicking();
            let outcome = SubagentOutcome {
                record: None,
                label: self.label.clone(),
                session: self.id.clone(),
                result: if panicked {
                    Err(AgentError::Io(std::io::Error::other("agent task panicked")))
                } else {
                    Ok(self
                        .store
                        .last_assistant_text(&self.id)
                        .ok()
                        .flatten()
                        .unwrap_or_default())
                },
                structured: None,
                tokens_used: 0,
                rounds: 0,
                stopped: (!panicked).then_some(LaneStop::Cancelled),
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
        self.budget.release_lane(&self.id);
    }
}
