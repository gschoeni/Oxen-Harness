//! The host-facing orchestration: one [`StudyService`] per workspace.
//!
//! A project's files (`progress.json`, `questions.jsonl`) are read, changed,
//! and rewritten whole, and the game issues requests that overlap: it
//! prefetches the next batch while an answer is being graded. So every
//! read-modify-write happens under [`FILES`], from a fresh read — and never
//! across a model call, which is where the seconds go. A batch plans under
//! the lock, writes questions without it, and re-reads before saving them.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use harness_protocol::{
    StudyAnswerRequest, StudyAnswerResult, StudyBatch, StudyBatchRequest, StudyProfile,
    StudyQuestion,
};

use crate::bank::{Bank, StoredQuestion};
use crate::material::{self, Material};
use crate::progress::{mastery_at, project_dir, project_key, Attempt, Mastery, Progress};
use crate::territories::{self, Territory};
use crate::{generate, grade, now_secs, Completer, StudyError, StudyMode};

/// Questions per batch when the request doesn't say.
pub const DEFAULT_BATCH: usize = 5;
/// The most questions one batch may hold.
pub const MAX_BATCH: usize = 8;

/// Serializes every read-modify-write of a project's study files. One lock
/// for all projects: the sections it guards are a few file reads and writes.
static FILES: Mutex<()> = Mutex::new(());

fn lock_files() -> Result<MutexGuard<'static, ()>, StudyError> {
    FILES.lock().map_err(|_| StudyError::Poisoned)
}

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

/// What a batch decided before any model call: the cached questions it
/// already has, and what to write the rest from.
struct Plan {
    label: String,
    picked: Vec<StudyQuestion>,
    /// Ids the batch must not hand out: the request's, plus those picked.
    exclude: Vec<String>,
    /// What fresh questions are written from; `None` when the cache filled
    /// the batch and nothing needs writing.
    material: Option<Material>,
    /// Prompts already in the bank for the material's region, oldest first.
    avoid: Vec<String>,
    territories: Vec<Territory>,
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

    /// The player's understanding of this project right now.
    pub fn profile(&self) -> Result<StudyProfile, StudyError> {
        let territories = territories::discover(&self.root);
        let progress = {
            let _files = lock_files()?;
            Progress::load(&self.dir)?
        };
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
        let Plan {
            label,
            mut picked,
            mut exclude,
            material,
            avoid,
            territories,
        } = self.plan(mode, count, &request.exclude, context, now)?;

        let mut tokens_used = 0;
        let mut model_used = String::new();
        if let Some(material) = material {
            let want = count - picked.len();
            let reply = model
                .complete(generate::SYSTEM, &generate::prompt(&material, want, &avoid))
                .await?;
            tokens_used = reply.tokens_used;
            model_used = model.model();
            let fresh = generate::parse(
                &reply.text,
                &material,
                &territories,
                &model_used,
                now,
                &self.root,
            )?;
            {
                let _files = lock_files()?;
                let mut bank = Bank::load(&self.dir)?;
                bank.push_new(fresh.clone());
                bank.save(&self.dir)?;
            }
            for q in fresh {
                if picked.len() >= count {
                    break;
                }
                if !exclude.contains(&q.question.id) {
                    exclude.push(q.question.id.clone());
                    picked.push(q.question);
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
            questions: picked,
            territory: label,
            tokens_used,
            model: model_used,
        })
    }

    /// Read the files once and decide the batch: which cached questions it
    /// serves and what the rest are written from.
    fn plan(
        &self,
        mode: StudyMode,
        count: usize,
        excluded: &[String],
        context: &BatchContext,
        now: i64,
    ) -> Result<Plan, StudyError> {
        let territories = territories::discover(&self.root);
        let (progress, bank) = {
            let _files = lock_files()?;
            (Progress::load(&self.dir)?, Bank::load(&self.dir)?)
        };
        let mut exclude = excluded.to_vec();
        let mut picked = Vec::new();
        let mut take = |found: Vec<&StoredQuestion>, exclude: &mut Vec<String>| {
            for q in found {
                if !exclude.contains(&q.question.id) {
                    exclude.push(q.question.id.clone());
                    picked.push(StudyQuestion {
                        cached: true,
                        ..q.question.clone()
                    });
                }
            }
        };
        let nowhere = || StudyError::Nothing("this workspace has no source files to study".into());

        // Territory modes serve the cache first and only read source files
        // when something is left to write; the other two always write.
        let (label, material) = match mode {
            StudyMode::Expedition | StudyMode::Review => {
                let (t, label) = if mode == StudyMode::Review {
                    let found = bank.missed(&exclude, count);
                    take(found, &mut exclude);
                    let t = most_faded_territory(&progress, &territories, now)
                        .or_else(|| weakest_territory(&progress, &territories, now))
                        .ok_or_else(nowhere)?;
                    (t, format!("review · {}", t.name))
                } else {
                    let t = weakest_territory(&progress, &territories, now).ok_or_else(nowhere)?;
                    (t, t.name.clone())
                };
                let short = count.saturating_sub(exclude.len() - excluded.len());
                let found = bank.pick(Some(&t.id), &exclude, short, now);
                take(found, &mut exclude);
                let filled = exclude.len() - excluded.len() >= count;
                let material = if filled {
                    None
                } else {
                    Some(material::for_territory(&self.root, t, now as u64)?)
                };
                (label, material)
            }
            StudyMode::FreshTracks => (
                "fresh tracks".into(),
                Some(
                    material::fresh_tracks(&self.root, &territories)?.ok_or_else(|| {
                        StudyError::Nothing("no uncommitted changes or commits to study".into())
                    })?,
                ),
            ),
            StudyMode::RideAlong => {
                let paths = material::ride_along_paths(&context.messages);
                if paths.is_empty() {
                    return Err(StudyError::Nothing(
                        "the agent hasn't read or edited any files in this chat yet".into(),
                    ));
                }
                (
                    "ride-along".into(),
                    Some(material::for_files(
                        &self.root,
                        &paths,
                        "Files the coding agent read or edited most recently in this chat.",
                        &territories,
                    )?),
                )
            }
        };
        let avoid = match &material {
            Some(material) => bank
                .questions
                .iter()
                .filter(|q| q.question.territory == material.territory_id)
                .map(|q| q.question.prompt.clone())
                .collect(),
            None => Vec::new(),
        };
        Ok(Plan {
            label,
            picked,
            exclude,
            material,
            avoid,
            territories,
        })
    }

    /// Grade an answer, record it, and return the grade with the updated
    /// profile.
    pub async fn answer(
        &self,
        request: &StudyAnswerRequest,
        model: &dyn Completer,
    ) -> Result<StudyAnswerResult, StudyError> {
        let stored = {
            let _files = lock_files()?;
            Bank::load(&self.dir)?
                .get(&request.question_id)
                .cloned()
                .ok_or_else(|| StudyError::UnknownQuestion(request.question_id.clone()))?
        };
        let mut tokens_used = 0;
        let graded = if stored.is_choice() {
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
        let now = now_secs();
        let territories = territories::discover(&self.root);
        let progress = {
            let _files = lock_files()?;
            let mut progress = Progress::load(&self.dir)?;
            progress.record(
                &stored.question.territory,
                Attempt {
                    at: now,
                    verdict: graded.verdict,
                    question_id: stored.question.id.clone(),
                    hint_used: request.hint_used,
                },
            );
            // The bank first: if the second write fails and the client
            // retries, a question marked asked twice is harmless, where an
            // attempt recorded twice would move mastery twice.
            let mut bank = Bank::load(&self.dir)?;
            bank.mark_asked(&stored.question.id, graded.verdict, now);
            bank.save(&self.dir)?;
            progress.save(&self.dir)?;
            progress
        };
        Ok(StudyAnswerResult {
            grade: graded.grade,
            profile: progress.profile(&self.key, &self.root, &territories, now),
            tokens_used,
        })
    }
}

fn scored<'a>(
    progress: &Progress,
    territories: &'a [Territory],
    now: i64,
) -> Vec<(&'a Territory, Mastery)> {
    territories
        .iter()
        .filter(|t| !t.files.is_empty())
        .map(|t| {
            let attempts = progress
                .territories
                .get(&t.id)
                .map(|p| p.attempts.as_slice())
                .unwrap_or(&[]);
            (t, mastery_at(attempts, now))
        })
        .collect()
}

/// Where an expedition goes: one of the weakest third of the territories,
/// rotating with the clock so consecutive runs don't all land on one.
fn weakest_territory<'a>(
    progress: &Progress,
    territories: &'a [Territory],
    now: i64,
) -> Option<&'a Territory> {
    let mut ranked = scored(progress, territories, now);
    ranked.sort_by(|a, b| a.1.now.total_cmp(&b.1.now));
    let pool = ranked.len().div_ceil(3);
    let index = (now as usize).checked_rem(pool)?;
    ranked.get(index).map(|(t, _)| *t)
}

/// Where a review goes: the explored territory that has slipped furthest
/// from what the player once knew.
fn most_faded_territory<'a>(
    progress: &Progress,
    territories: &'a [Territory],
    now: i64,
) -> Option<&'a Territory> {
    scored(progress, territories, now)
        .into_iter()
        .filter(|(_, m)| m.peak > 0.0)
        .max_by(|a, b| a.1.faded().total_cmp(&b.1.faded()))
        .map(|(t, _)| t)
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
        assert!(again.questions.iter().all(|q| q.cached));
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
                    answer: "a+b".into(),
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
    }

    /// The game prefetches a batch while an answer is being graded. The
    /// batch's save must build on the files as they are when it lands, not
    /// as they were when it started planning.
    #[tokio::test]
    async fn an_answer_that_lands_during_a_batch_is_not_overwritten() {
        struct AnswersMidFlight<'a> {
            svc: &'a StudyService,
            question_id: String,
        }

        #[async_trait::async_trait]
        impl Completer for AnswersMidFlight<'_> {
            async fn complete(&self, _system: &str, _user: &str) -> Result<Completion, StudyError> {
                self.svc
                    .answer(
                        &StudyAnswerRequest {
                            question_id: self.question_id.clone(),
                            answer: "a+b".into(),
                            hint_used: false,
                        },
                        &Canned::new(&[]),
                    )
                    .await?;
                Ok(Completion {
                    text: r#"[{"kind":"true_false","prompt":"add takes two arguments.","answer":0,"source_path":"src/lib.rs","source_lines":[1,1]}]"#.into(),
                    tokens_used: 5,
                })
            }
            fn model(&self) -> String {
                "interleaved".into()
            }
        }

        let ws = workspace();
        let study = tempfile::tempdir().unwrap();
        let svc = StudyService::open_at(ws.path(), study.path()).unwrap();
        let first = svc
            .batch(
                &StudyBatchRequest {
                    mode: "expedition".into(),
                    count: Some(2),
                    exclude: vec![],
                },
                &BatchContext::default(),
                &Canned::new(&[REPLY]),
            )
            .await
            .unwrap();
        let choice = first
            .questions
            .iter()
            .find(|q| q.kind == "multiple_choice")
            .unwrap();
        let used: Vec<String> = first.questions.iter().map(|q| q.id.clone()).collect();

        let second = svc
            .batch(
                &StudyBatchRequest {
                    mode: "expedition".into(),
                    count: Some(1),
                    exclude: used,
                },
                &BatchContext::default(),
                &AnswersMidFlight {
                    svc: &svc,
                    question_id: choice.id.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(second.questions.len(), 1);
        assert!(!second.questions[0].cached);

        let bank = Bank::load(study.path()).unwrap();
        assert_eq!(bank.questions.len(), 3);
        let answered = bank.get(&choice.id).unwrap();
        assert_eq!(
            answered.asked, 1,
            "the answer's bookkeeping survived the batch's save"
        );
        assert_eq!(answered.last_verdict, Some(crate::progress::Verdict::Full));
    }

    #[tokio::test]
    async fn a_failed_grade_records_nothing() {
        let ws = workspace();
        let study = tempfile::tempdir().unwrap();
        let svc = StudyService::open_at(ws.path(), study.path()).unwrap();
        let model = Canned::new(&[REPLY]);
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
        let free = batch
            .questions
            .iter()
            .find(|q| q.kind == "free_text")
            .unwrap();
        // The canned model has no reply left, so the grader call fails.
        let failed = svc
            .answer(
                &StudyAnswerRequest {
                    question_id: free.id.clone(),
                    answer: "add".into(),
                    hint_used: false,
                },
                &model,
            )
            .await;
        assert!(matches!(failed, Err(StudyError::Model { .. })));
        assert_eq!(svc.profile().unwrap().answered, 0);
        assert_eq!(
            Bank::load(study.path())
                .unwrap()
                .get(&free.id)
                .unwrap()
                .asked,
            0
        );
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
