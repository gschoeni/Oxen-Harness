//! The codebase study game's host surface: a profile, a batch of questions,
//! and an answer's grade, for the workspace a session is rooted in.
//!
//! The game is played *while* a turn runs, and a running turn holds its
//! session's agent lock for the duration — so nothing here touches that
//! agent. Model calls go through a detached, tool-less agent on the `study`
//! role (falling back to `smol`, then the session's model), billed to the
//! session's usage ledger under the `study` call kind.

use harness_agent::{Agent, AgentConfig, ModelRoles, Role};
use harness_protocol::{
    StudyAnswerRequest, StudyAnswerResult, StudyBatch, StudyBatchRequest, StudyProfile,
};
use harness_study::{BatchContext, Completer, Completion, StudyError, StudyMode, StudyService};

use crate::SessionService;

/// The reply ceiling for a study completion.
const STUDY_REPLY_TOKENS: usize = 8_192;

/// The usage ledger's call kind for study spend.
const USAGE_KIND: &str = "study";

/// [`Completer`] over a detached agent, built on first use: most study
/// requests (a cached batch, a multiple-choice answer) never call the model,
/// and shouldn't pay for a client and an agent they won't use.
struct LazyCompleter<'a> {
    service: &'a SessionService,
    session: &'a str,
    agent: tokio::sync::OnceCell<Agent>,
}

impl LazyCompleter<'_> {
    async fn agent(&self) -> Result<&Agent, StudyError> {
        self.agent
            .get_or_try_init(|| self.service.study_agent(self.session))
            .await
            .map_err(|detail| StudyError::Model {
                model: "study".into(),
                detail,
            })
    }
}

#[async_trait::async_trait]
impl Completer for LazyCompleter<'_> {
    async fn complete(&self, system: &str, user: &str) -> Result<Completion, StudyError> {
        let agent = self.agent().await?;
        let (text, tokens_used) =
            agent
                .complete_as(system, user, USAGE_KIND)
                .await
                .map_err(|e| StudyError::Model {
                    model: agent.config().model.clone(),
                    detail: e.to_string(),
                })?;
        Ok(Completion { text, tokens_used })
    }

    /// The model that ran — empty until a completion has.
    fn model(&self) -> String {
        self.agent
            .get()
            .map(|agent| agent.config().model.clone())
            .unwrap_or_default()
    }
}

impl SessionService {
    /// How well the user understands the project `session` is rooted in.
    pub fn study_profile(&self, session: &str) -> Result<StudyProfile, String> {
        self.study_service(session)?
            .profile()
            .map_err(|e| e.to_string())
    }

    /// Questions for one stretch of a study run: cached ones where the
    /// project has them, the rest written by the study model.
    pub async fn study_batch(
        &self,
        session: &str,
        request: StudyBatchRequest,
    ) -> Result<StudyBatch, String> {
        let service = self.study_service(session)?;
        // Only the ride-along reads the transcript; the other modes never
        // pay for deserializing a long chat.
        let rides_along = StudyMode::parse(&request.mode) == Some(StudyMode::RideAlong);
        let context = BatchContext {
            messages: if rides_along {
                self.store()?
                    .messages(session)
                    .map_err(|e| format!("reading the chat's messages for the study game: {e}"))?
            } else {
                Vec::new()
            },
        };
        let model = self.study_completer(session);
        service
            .batch(&request, &context, &model)
            .await
            .map_err(|e| e.to_string())
    }

    /// Grade an answer and record it against the project's progress.
    pub async fn study_answer(
        &self,
        session: &str,
        request: StudyAnswerRequest,
    ) -> Result<StudyAnswerResult, String> {
        let service = self.study_service(session)?;
        let model = self.study_completer(session);
        service
            .answer(&request, &model)
            .await
            .map_err(|e| e.to_string())
    }

    fn study_service(&self, session: &str) -> Result<StudyService, String> {
        StudyService::open(&self.session_workspace(session)).map_err(|e| e.to_string())
    }

    fn study_completer<'a>(&'a self, session: &'a str) -> LazyCompleter<'a> {
        LazyCompleter {
            service: self,
            session,
            agent: tokio::sync::OnceCell::new(),
        }
    }

    /// A detached one-shot agent on the study role, billed to `session`.
    async fn study_agent(&self, session: &str) -> Result<Agent, String> {
        let (client, session_model, _) = self.client_for().await?;
        let limits = harness_runtime::limits::load();
        let roles = ModelRoles {
            smol: limits.smol_model,
            summary: limits.summary_model,
            study: limits.study_model,
        };
        let model = roles.resolve(Role::Study, &session_model).to_owned();
        let config = AgentConfig {
            // Catalog limits are per model; a study model that isn't the
            // session's must not inherit the session's.
            max_output_tokens: harness_local::limits::max_output_tokens(&model),
            // A batch of questions is ~1.5k tokens of JSON; the headroom is
            // for models that think before they answer.
            response_reserve: STUDY_REPLY_TOKENS,
            model,
            system_prompt: None,
            error_log: harness_config::paths::errors_log().ok(),
            request_log: harness_config::paths::requests_log().ok(),
            ..AgentConfig::default()
        };
        Agent::detached(client, config, self.store()?, session)
            .map_err(|e| format!("starting the study model: {e}"))
    }
}
