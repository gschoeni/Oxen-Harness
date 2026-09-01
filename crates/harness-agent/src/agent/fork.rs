//! Forking a session: carry on from an earlier point without rewriting
//! what happened after it.
//!
//! A rewind ("go back to before that message and try again") is a fork of
//! the transcript up to the cut, resumed as a new session. The original
//! keeps every message, so nothing the user said is lost, and the fork
//! inherits the session state (plan, trail, rule history) it had at the
//! time.

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
        Ok(forked)
    }

    /// The points a rewind can return to: every user message so far as
    /// `(seq, preview)`, oldest first. A rewind to a turn is
    /// `fork_through(Some(seq - 1))` — everything before that message.
    pub fn user_turns(&self) -> Result<Vec<(i64, String)>, AgentError> {
        Ok(self.store.user_turns(&self.session_id)?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use harness_llm::types::ChatMessage;
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
}
