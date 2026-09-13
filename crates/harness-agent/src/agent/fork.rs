//! Forking a session: carry on from an earlier point without rewriting
//! what happened after it.
//!
//! A rewind ("go back to before that message and try again") is a fork of
//! the transcript up to the cut, resumed as a new session. The original
//! keeps every message, so nothing the user said is lost. The fork's plan
//! and trail are rebuilt from the messages it kept — a snapshot of the
//! source's state *now* would show work the fork never did — and its rule
//! history starts fresh, so once-per-session reminders may fire again.

use crate::error::AgentError;

use super::Agent;

impl Agent {
    /// A new agent on a fork of this session that keeps every message with
    /// `seq <= through_seq` (`None` keeps everything — a plain copy). The
    /// fork shares the client, tools, and config; this agent is untouched.
    pub fn fork_through(&self, through_seq: Option<i64>) -> Result<Agent, AgentError> {
        let session = self.store.fork_session(&self.session_id, through_seq)?;
        let mut forked = Agent::resume_from_store(
            self.client.clone(),
            self.tools.clone(),
            self.store.clone(),
            session,
            self.config.clone(),
        )?;
        forked.set_rules(self.rules.clone());
        if through_seq.is_some() {
            forked.rebuild_projections()?;
        }
        Ok(forked)
    }

    /// A new agent on a fork that ends just before the user message at
    /// `turn_seq` (one of [`Self::user_turns`]). The cut backs up over an
    /// unfinished tool round so the fork never resumes mid-round (see
    /// `HistoryStore::rewind_cut`).
    pub fn rewind_before(&self, turn_seq: i64) -> Result<Agent, AgentError> {
        let cut = self.store.rewind_cut(&self.session_id, turn_seq)?;
        self.fork_through(Some(cut))
    }

    /// The points a rewind can return to: every message the user sent so far
    /// as `(seq, preview)`, oldest first. Messages the harness composed in
    /// the user's voice (delivered task output, tool-image stubs) are not
    /// offered. A rewind to a turn is [`Self::rewind_before`].
    pub fn user_turns(&self) -> Result<Vec<(i64, String)>, AgentError> {
        Ok(self.store.user_turns(&self.session_id)?)
    }

    /// Recompute the plan and trail snapshots from this session's transcript:
    /// the same fold the turn loop applies to each successful `update_plan`
    /// / `update_trail` call, replayed over the messages the fork kept.
    fn rebuild_projections(&self) -> Result<(), AgentError> {
        let mut plan: Option<harness_tools::PlanSnapshot> = None;
        let mut trail: Option<harness_tools::TrailSnapshot> = None;
        for call in self
            .messages
            .iter()
            .filter(|m| m.role == "assistant")
            .filter_map(|m| m.tool_calls.as_ref())
            .flatten()
        {
            if call.function.name == harness_tools::PLAN_TOOL {
                if let Some(items) = harness_tools::parse_plan_arguments(&call.function.arguments) {
                    plan = Some(harness_tools::plan_snapshot(&items));
                }
            } else if call.function.name == harness_tools::TRAIL_TOOL {
                if let Some(next) = harness_tools::parse_trail_arguments(&call.function.arguments) {
                    trail = Some(harness_tools::merge_trail(trail.take(), next));
                }
            }
        }
        if plan.is_some() {
            self.store
                .save_session_state(&self.session_id, harness_store::PLAN_STATE, &plan)?;
        }
        if let Some(trail) = trail {
            self.store
                .save_session_state(&self.session_id, harness_store::TRAIL_STATE, &trail)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use harness_llm::types::{ChatMessage, FunctionCall, ToolCall};
    use harness_llm::OxenClient;
    use harness_store::HistoryStore;
    use harness_tools::ToolRegistry;

    use crate::test_support::test_session;
    use crate::{Agent, AgentConfig};

    #[test]
    fn a_fork_resumes_from_the_cut_and_leaves_the_original_alone() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new("http://localhost/api/ai", "key", "claude-opus-4-8");
        let mut agent = Agent::new(
            client,
            ToolRegistry::new(),
            store.clone(),
            session.clone(),
            AgentConfig {
                system_prompt: None,
                ..AgentConfig::default()
            },
        )
        .unwrap();
        agent.inject_exchange("first", "a1").unwrap();
        agent.inject_exchange("second", "a2").unwrap();
        let turns = agent.user_turns().unwrap();
        assert_eq!(turns.len(), 2);
        let (second_seq, _) = turns[1];

        let fork = agent.fork_through(Some(second_seq - 1)).unwrap();
        assert_ne!(fork.session_id(), agent.session_id());
        let texts: Vec<String> = fork
            .messages()
            .iter()
            .filter_map(ChatMessage::content_text)
            .collect();
        assert_eq!(texts, vec!["first", "a1"]);
        assert_eq!(agent.messages().len(), 4, "the original keeps everything");
        assert_eq!(
            store
                .forked_from(fork.session_id())
                .unwrap()
                .map(|(f, _)| f),
            Some(session)
        );
    }

    fn plan_call(id: &str, status: &str) -> ChatMessage {
        let args = serde_json::json!({
            "plan": [{ "content": "Research", "active_form": "Researching", "status": status }]
        });
        ChatMessage::assistant_with_tools(
            String::new(),
            vec![ToolCall {
                id: id.into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: harness_tools::PLAN_TOOL.into(),
                    arguments: args.to_string(),
                },
            }],
        )
    }

    #[tokio::test]
    async fn a_rewind_skips_synthetic_turns_and_rebuilds_the_plan_as_of_the_cut() {
        let store = Arc::new(HistoryStore::open_in_memory().unwrap());
        let session = test_session(&store, "claude-opus-4-8");
        let client = OxenClient::new("http://localhost/api/ai", "key", "claude-opus-4-8");
        let mut agent = Agent::new(
            client,
            ToolRegistry::new(),
            store.clone(),
            session.clone(),
            AgentConfig {
                system_prompt: None,
                ..AgentConfig::default()
            },
        )
        .unwrap();
        // Turn 1: the model opens a plan, a tool hands back an image (its
        // stub is a user-role message nobody typed), then it answers.
        agent.push(ChatMessage::user("start")).unwrap();
        agent.push(plan_call("c1", "in_progress")).unwrap();
        agent
            .push(ChatMessage::tool_result("c1", "plan recorded"))
            .unwrap();
        agent
            .push_synthetic(ChatMessage::user(
                "The image(s) produced by the tool call above:",
            ))
            .unwrap();
        agent.push(ChatMessage::assistant("working on it")).unwrap();
        // Turn 2: the plan is finished — and persisted as the session's
        // current projection.
        agent.push(ChatMessage::user("finish up")).unwrap();
        agent.push(plan_call("c2", "completed")).unwrap();
        agent
            .push(ChatMessage::tool_result("c2", "plan recorded"))
            .unwrap();
        agent.push(ChatMessage::assistant("done")).unwrap();
        store
            .save_session_state(
                &session,
                harness_store::PLAN_STATE,
                &Some(harness_tools::PlanSnapshot {
                    done: 1,
                    total: 1,
                    active: None,
                }),
            )
            .unwrap();

        // Only the two typed messages are offered.
        let turns = agent.user_turns().unwrap();
        let previews: Vec<&str> = turns.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(previews, vec!["start", "finish up"]);

        // Rewinding to before "finish up" keeps turn 1 whole …
        let fork = agent.rewind_before(turns[1].0).unwrap();
        assert_eq!(fork.messages().len(), 5);
        assert_eq!(fork.messages().last().unwrap().role, "assistant");
        // … and the plan is what turn 1 left it at, not the source's finished one.
        let plan: Option<harness_tools::PlanSnapshot> = store
            .session_state(fork.session_id(), harness_store::PLAN_STATE)
            .unwrap()
            .flatten();
        assert_eq!(
            plan,
            Some(harness_tools::PlanSnapshot {
                done: 0,
                total: 1,
                active: Some("Researching".into()),
            })
        );
        let source_plan: Option<harness_tools::PlanSnapshot> = store
            .session_state(&session, harness_store::PLAN_STATE)
            .unwrap()
            .flatten();
        assert_eq!(
            source_plan.map(|p| p.done),
            Some(1),
            "the source is untouched"
        );
    }
}
