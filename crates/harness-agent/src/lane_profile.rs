//! Durable, opt-in capability limits for research lanes.

use harness_store::HistoryStore;
use harness_tools::ToolRegistry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::AgentError;

pub(crate) const LANE_PROFILE_STATE: &str = "lane_profile";
pub(crate) const RESEARCH_TOOLS: &[&str] = &[
    "read_file",
    "find_files",
    "search_files",
    "web_search",
    "web_fetch",
    "retrieve_original",
];

/// Capabilities a spawned lane may use. Omission preserves full access.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaneProfile {
    #[default]
    Full,
    Research,
}

impl LaneProfile {
    pub(crate) fn load(store: &HistoryStore, session: &str) -> Result<Self, AgentError> {
        Ok(store
            .session_state(session, LANE_PROFILE_STATE)?
            .unwrap_or_default())
    }

    pub(crate) fn filter(self, tools: ToolRegistry) -> ToolRegistry {
        match self {
            Self::Full => tools,
            Self::Research => tools.only(RESEARCH_TOOLS),
        }
    }

    pub(crate) fn apply_prompt(self, prompt: &mut Option<String>) {
        if self == Self::Research {
            let prompt = prompt.get_or_insert_with(String::new);
            *prompt = crate::prompt::strip_delegation_sections(prompt)
                .replace(crate::prompt::LANE_APPENDIX, crate::prompt::LEAF_APPENDIX);
            if !prompt.ends_with(RESEARCH_APPENDIX) {
                prompt.push_str(RESEARCH_APPENDIX);
            }
        }
    }
}

const RESEARCH_APPENDIX: &str = "\n\n## Research profile\n\
This lane uses the research profile: only enabled read_file, find_files, search_files, \
web_search, web_fetch, and retrieve_original tools are available. Earlier instructions \
about editing, shell commands, media, UI, or delegation do not grant those capabilities. \
Investigate with read/search tools and return evidence with paths or URLs. Do not modify \
files, run commands, spawn agents, or ask the user questions. Report blockers and uncertainty \
to the caller rather than attempting unavailable operations.";
