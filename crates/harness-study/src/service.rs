//! The host-facing orchestration: one [`StudyService`] per workspace.

use std::path::{Path, PathBuf};

use harness_protocol::{
    StudyAnswerRequest, StudyAnswerResult, StudyBatch, StudyBatchRequest, StudyProfile,
    StudyQuestion,
};

use crate::bank::{Bank, StoredQuestion};
use crate::material::{self, Material};
use crate::progress::{project_dir, project_key, Attempt, Progress, Verdict};
use crate::territories::{self, Territory};
use crate::{generate, grade, now_secs, Completer, StudyError, StudyMode};

/// Questions per batch when the request doesn't say.
pub const DEFAULT_BATCH: usize = 5;
/// The most questions one batch may hold.
pub const MAX_BATCH: usize = 8;

/// What a batch may draw on beyond the workspace.
#[derive(Debug, Default)]
pub struct BatchContext {
    /// The session's stored messages, for the ride-along mode.
    pub messages: Vec<serde_json::Value>,
}

pub struct StudyService {
    root: PathBuf,
    key: String,
    dir: PathBuf,
}

impl StudyService {
    /// Open (creating on demand) the study files for `workspace`.
    pub fn open(workspace: &Path) -> Result<Self, StudyError> {
        Ok(Self {
            root: workspace.to_path_buf(),
            key: project_key(workspace),
            dir: project_dir(workspace)?,
        })
    }

    /// Like [`Self::open`] but with the study files under `dir` — tests.
    pub fn open_at(workspace: &Path, dir: &Path) -> Result<Self, StudyError> {
        std::fs::create_dir_all(dir).map_err(|e| StudyError::io("create", dir, e))?;
        Ok(Self {
            root: workspace.to_path_buf(),
            key: project_key(workspace),
            dir: dir.to_path_buf(),
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.root
    }

    /// The player's understanding of this project right now.
    pub fn profile(&self) -> Result<StudyProfile, StudyError> {
        let territories = territories::discover(&self.root);
        let progress = Progress::load(&self.dir)?;
        Ok(progress.profile(&self.key, &self.root, &territories, now_secs()))
    }

    /// A batch of questions for a mode: cached ones first, the rest freshly
    /// written by `model` and cached for next time.
    pub async fn batch(
        &self,
        request: &StudyBatchRequest,
        context: &BatchContext,
        model: &dyn Completer,
    ) -> Result<StudyBatch, StudyError> {
        let mode = StudyMode::parse(&request.mode)
            .ok_or_else(|| StudyError::Invalid(format!("unknown study mode `{}`", request.mode)))?;
        let count = request.count.unwrap_or(DEFAULT_BATCH).clamp(1, MAX_BATCH);
        let now = now_secs();
        let territories = territories::discover(&self.root);
        if territories.is_empty() {
            return Err(StudyError::Nothing(
                "this workspace has no source files to study".into(),
            ));
        }
        let progress = Progress::load(&self.dir)?;
        let mut bank = Bank::load(&self.dir)?;
        let mut exclude = request.exclude.clone();
        let mut picked: Vec<StoredQuestion> = Vec::new();
        let mut tokens_used = 0usize;
        let mut model_used = String::new();

        // What the batch is about, and where fresh questions come from.
        let (label, source): (String, Option<Material>) = match mode {
            StudyMode::Expedition => {
                let t = self.choose_territory(&progress, &territories, now, false);
                let found = owned(bank.pick(Some(&t.id), &exclude, count, now));
                take(&mut picked, &mut exclude, found);
                (
                    t.name.clone(),
                    Some(material::for_territory(&self.root, t, now as u64)?),
                )
            }
            StudyMode::Review => {
                let found = owned(bank.missed(&exclude, count));
                take(&mut picked, &mut exclude, found);
                let t = self.choose_territory(&progress, &territories, now, true);
                if picked.len() < count {
                    let found = owned(bank.pick(Some(&t.id), &exclude, count - picked.len(), now));
                    take(&mut picked, &mut exclude, found);
                }
                (
                    format!("review · {}", t.name),
                    Some(material::for_territory(&self.root, t, now as u64)?),
                )
            }
            StudyMode::FreshTracks => {
                let m = material::fresh_tracks(&self.root, &territories)?.ok_or_else(|| {
                    StudyError::Nothing("no uncommitted changes or commits to study".into())
                })?;
                ("fresh tracks".into(), Some(m))
            }
            StudyMode::RideAlong => {
                let paths = material::ride_along_paths(&context.messages);
                if paths.is_empty() {
                    return Err(StudyError::Nothing(
                        "the agent hasn't read or edited any files in this chat yet".into(),
                    ));
                }
                let m = material::for_files(
                    &self.root,
                    &paths,
                    "Files the coding agent read or edited most recently in this chat.",
                    &territories,
                )?;
                ("ride-along".into(), Some(m))
            }
        };

        if picked.len() < count {
            if let Some(material) = source {
                let want = count - picked.len();
                let avoid: Vec<String> = bank
                    .questions
                    .iter()
                    .filter(|q| q.question.territory == material.territory_id)
                    .map(|q| q.question.prompt.clone())
                    .collect();
                let reply = model
                    .complete(generate::SYSTEM, &generate::prompt(&material, want, &avoid))
                    .await?;
                tokens_used += reply.tokens_used;
                model_used = model.model();
                let fresh = generate::parse(&reply.text, &material, &model_used, now, &self.root)?;
                let ids: Vec<String> = fresh.iter().map(|q| q.question.id.clone()).collect();
                bank.push_new(fresh);
                bank.save(&self.dir)?;
                for id in ids {
                    if picked.len() >= count {
                        break;
                    }
                    if exclude.contains(&id) {
                        continue;
                    }
                    if let Some(q) = bank.get(&id) {
                        picked.push(q.clone());
                        exclude.push(id);
                    }
                }
            }
        }

        if picked.is_empty() {
            return Err(StudyError::Nothing(
                "no questions could be found or written for this mode".into(),
            ));
        }
        Ok(StudyBatch {
            mode: mode.as_str().into(),
            questions: picked.into_iter().map(client_view).collect(),
            territory: label,
            tokens_used,
            model: model_used,
        })
    }

    /// Grade an answer, record it, and return the grade with the updated
    /// profile.
    pub async fn answer(
        &self,
        request: &StudyAnswerRequest,
        model: &dyn Completer,
    ) -> Result<StudyAnswerResult, StudyError> {
        let mut bank = Bank::load(&self.dir)?;
        let stored = bank
            .get(&request.question_id)
            .cloned()
            .ok_or_else(|| StudyError::UnknownQuestion(request.question_id.clone()))?;
        let mut tokens_used = 0;
        let grade = if stored.is_choice() {
            grade::grade_choice(&stored, &request.answer)
        } else if request.answer.trim().is_empty() {
            grade::blank_grade(&stored)
        } else {
            let reply = model
                .complete(
                    grade::GRADER_SYSTEM,
                    &grade::free_text_prompt(&stored, &request.answer),
                )
                .await?;
            tokens_used = reply.tokens_used;
            grade::parse_free_text(&reply.text, &stored)
        };
        let verdict = Verdict::parse(&grade.verdict).unwrap_or(Verdict::Wrong);
        let now = now_secs();
        let mut progress = Progress::load(&self.dir)?;
        progress.record(
            &stored.question.territory,
            Attempt {
                at: now,
                verdict,
                question_id: stored.question.id.clone(),
                hint_used: request.hint_used,
            },
        );
        progress.save(&self.dir)?;
        bank.mark_asked(&stored.question.id, verdict, now);
        bank.save(&self.dir)?;
        let territories = territories::discover(&self.root);
        Ok(StudyAnswerResult {
            grade,
            profile: progress.profile(&self.key, &self.root, &territories, now),
            tokens_used,
        })
    }

    /// Record the longest streak of an expedition when it beats the best.
    pub fn record_streak(&self, streak: u32) -> Result<u32, StudyError> {
        let mut progress = Progress::load(&self.dir)?;
        if streak > progress.best_streak {
            progress.best_streak = streak;
            progress.save(&self.dir)?;
        }
        Ok(progress.best_streak)
    }

    /// The territory to visit next. Expeditions go where mastery is lowest
    /// (with some variety among the weakest third); reviews go where it has
    /// faded most from its peak, else where it is lowest among the explored.
    fn choose_territory<'a>(
        &self,
        progress: &Progress,
        territories: &'a [Territory],
        now: i64,
        review: bool,
    ) -> &'a Territory {
        let with_files: Vec<&Territory> =
            territories.iter().filter(|t| !t.files.is_empty()).collect();
        let pool: &[&Territory] = if with_files.is_empty() {
            &[]
        } else {
            &with_files
        };
        let scored: Vec<(&Territory, crate::progress::Mastery)> = pool
            .iter()
            .map(|t| {
                let attempts = progress
                    .territories
                    .get(&t.id)
                    .map(|p| p.attempts.as_slice())
                    .unwrap_or(&[]);
                (*t, crate::progress::mastery_at(attempts, now))
            })
            .collect();
        if review {
            if let Some((t, _)) = scored
                .iter()
                .filter(|(_, m)| m.peak > 0.0)
                .max_by(|a, b| a.1.faded().total_cmp(&b.1.faded()))
            {
                return t;
            }
        }
        let mut ranked = scored;
        ranked.sort_by(|a, b| a.1.now.total_cmp(&b.1.now));
        let weakest = ranked.len().div_ceil(3).max(1).min(ranked.len());
        let index = (now as usize) % weakest;
        ranked
            .get(index)
            .map(|(t, _)| *t)
            .unwrap_or(&territories[0])
    }
}

/// Move found questions into the batch, marking their ids used.
fn take(picked: &mut Vec<StoredQuestion>, exclude: &mut Vec<String>, found: Vec<StoredQuestion>) {
    for q in found {
        if !exclude.contains(&q.question.id) {
            exclude.push(q.question.id.clone());
            picked.push(q);
        }
    }
}

fn owned(found: Vec<&StoredQuestion>) -> Vec<StoredQuestion> {
    found.into_iter().cloned().collect()
}

/// The client's view of a stored question: the answer stays here.
fn client_view(q: StoredQuestion) -> StudyQuestion {
    StudyQuestion {
        cached: q.asked > 0 || q.created_at < now_secs() - 5,
        ..q.question
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Completion;
    use std::sync::Mutex;

    /// A canned model: replies in order, recording the prompts it saw.
    struct Canned {
        replies: Mutex<Vec<String>>,
        prompts: Mutex<Vec<String>>,
    }

    impl Canned {
        fn new(replies: &[&str]) -> Self {
            Self {
                replies: Mutex::new(replies.iter().rev().map(|s| s.to_string()).collect()),
                prompts: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl Completer for Canned {
        async fn complete(&self, _system: &str, user: &str) -> Result<Completion, StudyError> {
            self.prompts.lock().unwrap().push(user.to_string());
            let text = self
                .replies
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| StudyError::Model {
                    model: "canned".into(),
                    detail: "no reply left".into(),
                })?;
            Ok(Completion {
                text,
                tokens_used: 10,
            })
        }
        fn model(&self) -> String {
            "canned".into()
        }
    }

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn add(a: u32, b: u32) -> u32 { a + b }\n",
        )
        .unwrap();
        dir
    }

    const REPLY: &str = r#"[{"kind":"multiple_choice","prompt":"What does add return?","options":["a+b","a-b","a*b","0"],"answer":0,"explanation":"It adds.","source_path":"src/lib.rs","source_lines":[1,1],"difficulty":1},
{"kind":"free_text","prompt":"Name the public function.","answer":"add","explanation":"","source_path":"src/lib.rs","source_lines":[1,1]}]"#;

    #[tokio::test]
    async fn a_batch_generates_then_replays_from_the_cache() {
        let ws = workspace();
        let study = tempfile::tempdir().unwrap();
        let svc = StudyService::open_at(ws.path(), study.path()).unwrap();
        let model = Canned::new(&[REPLY]);
        let request = StudyBatchRequest {
            mode: "expedition".into(),
            count: Some(2),
            exclude: vec![],
        };
        let batch = svc
            .batch(&request, &BatchContext::default(), &model)
            .await
            .unwrap();
        assert_eq!(batch.questions.len(), 2);
        assert_eq!(batch.tokens_used, 10);
        assert_eq!(batch.model, "canned");
        assert!(batch.questions.iter().all(|q| !q.cached));
        // The prompt carried the source.
        assert!(model.prompts.lock().unwrap()[0].contains("pub fn add"));

        // Second batch: the model has no reply left, but the cache serves.
        let again = svc
            .batch(&request, &BatchContext::default(), &model)
            .await
            .unwrap();
        assert_eq!(again.questions.len(), 2);
        assert_eq!(again.tokens_used, 0);
        assert_eq!(model.prompts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn answers_are_graded_recorded_and_reflected_in_the_profile() {
        let ws = workspace();
        let study = tempfile::tempdir().unwrap();
        let svc = StudyService::open_at(ws.path(), study.path()).unwrap();
        let model = Canned::new(&[REPLY, "{\"verdict\":\"partial\",\"feedback\":\"Close.\"}"]);
        let batch = svc
            .batch(
                &StudyBatchRequest {
                    mode: "expedition".into(),
                    count: Some(2),
                    exclude: vec![],
                },
                &BatchContext::default(),
                &model,
            )
            .await
            .unwrap();
        let choice = batch
            .questions
            .iter()
            .find(|q| q.kind == "multiple_choice")
            .unwrap();
        let free = batch
            .questions
            .iter()
            .find(|q| q.kind == "free_text")
            .unwrap();

        let right = svc
            .answer(
                &StudyAnswerRequest {
                    question_id: choice.id.clone(),
                    answer: "0".into(),
                    hint_used: false,
                },
                &model,
            )
            .await
            .unwrap();
        assert_eq!(right.grade.verdict, "full");
        assert_eq!(right.tokens_used, 0);
        assert!(right.profile.understanding > 0.0);
        assert_eq!(right.profile.answered, 1);

        let partial = svc
            .answer(
                &StudyAnswerRequest {
                    question_id: free.id.clone(),
                    answer: "add".into(),
                    hint_used: true,
                },
                &model,
            )
            .await
            .unwrap();
        assert_eq!(partial.grade.verdict, "partial");
        assert_eq!(partial.grade.feedback, "Close.");
        assert_eq!(partial.tokens_used, 10);
        assert_eq!(partial.profile.answered, 2);

        let unknown = svc
            .answer(
                &StudyAnswerRequest {
                    question_id: "nope".into(),
                    answer: "".into(),
                    hint_used: false,
                },
                &model,
            )
            .await;
        assert!(matches!(unknown, Err(StudyError::UnknownQuestion(_))));
        assert_eq!(svc.record_streak(3).unwrap(), 3);
        assert_eq!(svc.record_streak(1).unwrap(), 3);
    }

    #[tokio::test]
    async fn modes_without_material_say_so() {
        let ws = workspace();
        let study = tempfile::tempdir().unwrap();
        let svc = StudyService::open_at(ws.path(), study.path()).unwrap();
        let model = Canned::new(&[]);
        let ride = svc
            .batch(
                &StudyBatchRequest {
                    mode: "ride_along".into(),
                    count: None,
                    exclude: vec![],
                },
                &BatchContext::default(),
                &model,
            )
            .await;
        assert!(matches!(ride, Err(StudyError::Nothing(_))));
        let bad = svc
            .batch(
                &StudyBatchRequest {
                    mode: "wander".into(),
                    count: None,
                    exclude: vec![],
                },
                &BatchContext::default(),
                &model,
            )
            .await;
        assert!(matches!(bad, Err(StudyError::Invalid(_))));
    }
}
