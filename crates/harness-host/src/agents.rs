//! Agent history and explicit user actions, shared by desktop and HTTP hosts.

use harness_agent::{worktree::WorktreeSnapshot, SubagentResult};
use harness_protocol::AgentSummary;

use crate::SessionService;

impl SessionService {
    pub fn list_agents(&self, session: &str) -> Result<Vec<AgentSummary>, String> {
        let store = self.store()?;
        let live = self
            .fleet_spawner_for(session)
            .map(|s| s.tree().live())
            .unwrap_or_default();
        let mut pending = vec![(session.to_string(), 0)];
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        while let Some((parent, depth)) = pending.pop() {
            if !seen.insert(parent.clone()) {
                continue;
            }
            let lanes = store.lanes_of(&parent).map_err(|e| e.to_string())?;
            for lane in lanes {
                pending.push((lane.id.clone(), depth + 1));
                let record = lane
                    .record
                    .and_then(|value| serde_json::from_value::<SubagentResult>(value).ok());
                let running = live.iter().find(|l| l.id == lane.id);
                let status = match (&running, &record) {
                    (Some(_), _) => "running".to_string(),
                    (_, Some(r)) if r.stop.as_deref().is_some_and(|s| s.contains("cancelled")) => {
                        "cancelled".into()
                    }
                    (_, Some(r)) => format!("{:?}", r.status).to_lowercase(),
                    _ => "unknown".into(),
                };
                out.push(AgentSummary {
                    id: lane.id,
                    parent: parent.clone(),
                    depth,
                    label: running
                        .map(|l| l.label.clone())
                        .or_else(|| record.as_ref().map(|r| r.label.clone()))
                        .unwrap_or_else(|| "Interrupted agent".into()),
                    fleet: running
                        .map(|l| l.fleet.clone())
                        .or_else(|| record.as_ref().map(|r| r.fleet.clone()))
                        .unwrap_or_default(),
                    status,
                    summary: record.as_ref().map(|r| r.brief()).unwrap_or_default(),
                    tokens: record.as_ref().map(|r| r.tokens).unwrap_or_default(),
                    rounds: record.as_ref().map(|r| r.rounds).unwrap_or_default(),
                    model: record.as_ref().map(|r| r.model.clone()).unwrap_or_default(),
                    stop: record.as_ref().and_then(|r| r.stop.clone()),
                    has_patch: record.as_ref().is_some_and(|r| r.has_patch),
                    elapsed_secs: running.map(|l| l.elapsed_secs).unwrap_or_else(|| {
                        record
                            .as_ref()
                            .map(|r| r.elapsed_ms / 1000)
                            .unwrap_or_default()
                    }),
                    created_at: lane.created_at,
                });
            }
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    pub fn agent_patch(&self, session: &str, lane: &str) -> Result<String, String> {
        if !self.list_agents(session)?.iter().any(|a| a.id == lane) {
            return Err("This agent does not belong to this chat".into());
        }
        Ok(self
            .store()?
            .session_state::<WorktreeSnapshot>(lane, harness_agent::LANE_WORKSPACE_STATE)
            .map_err(|e| e.to_string())?
            .map(|s| s.patch)
            .unwrap_or_default())
    }

    pub async fn follow_up_agent(
        &self,
        session: &str,
        lane: &str,
        text: &str,
    ) -> Result<String, String> {
        let agent = self.agent_or_build(session).await?;
        // The lane answers on the chat's endpoint: have it reachable first.
        self.ready_if_idle(session, &agent).await?;
        self.fleet_spawner_for(session)
            .ok_or("Agent tools are unavailable in this chat")?
            .follow_up(lane, text)
            .await
            .map_err(|e| e.to_string())
    }

    /// Apply exactly the patch the user reviewed, refusing a changed artifact
    /// or a conflicting working tree. Git checks the complete patch atomically.
    pub async fn apply_agent_patch(
        &self,
        session: &str,
        lane: &str,
        reviewed_patch: &str,
    ) -> Result<(), String> {
        if self
            .fleet_spawner_for(session)
            .is_some_and(|s| s.tree().is_live(lane))
        {
            return Err("Stop the agent before applying its changes".into());
        }
        let patch = self.agent_patch(session, lane)?;
        if patch.is_empty() {
            return Err("This agent has no changes to apply".into());
        }
        if patch != reviewed_patch {
            return Err(
                "The agent's changes have changed. Review them again before applying".into(),
            );
        }
        let root = self.session_workspace(session);
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            use std::process::{Command, Stdio};
            let mut child = Command::new("git")
                .args(["apply", "--binary", "--whitespace=nowarn", "-"])
                .current_dir(root)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
            child
                .stdin
                .take()
                .ok_or("Could not open patch input")?
                .write_all(patch.as_bytes())
                .map_err(|e| e.to_string())?;
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            if output.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "Changes were not applied: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ))
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }
}
