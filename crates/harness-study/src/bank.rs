//! The per-project question cache (`questions.jsonl`).
//!
//! Every generated question is kept with its answer and history, so a replay
//! of a territory costs no model calls and a missed question can be asked
//! again. Picking favors what has never been asked, then what was missed,
//! and skips anything answered correctly in the last few days.

use std::path::Path;

use harness_protocol::StudyQuestion;
use serde::{Deserialize, Serialize};

use crate::progress::Verdict;
use crate::StudyError;

const FILE: &str = "questions.jsonl";

/// Don't re-ask a question answered fully within this window.
const RECENT_SECS: i64 = 3 * 24 * 3600;

/// A question as stored: the client-facing part plus what only the grader
/// sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredQuestion {
    #[serde(flatten)]
    pub question: StudyQuestion,
    /// For choice questions the 0-based index of the correct option (as
    /// text); for free text the reference answer.
    pub answer: String,
    pub explanation: String,
    pub created_at: i64,
    pub model: String,
    #[serde(default)]
    pub asked: u32,
    #[serde(default)]
    pub last_verdict: Option<Verdict>,
    #[serde(default)]
    pub last_asked_at: Option<i64>,
}

impl StoredQuestion {
    pub fn is_choice(&self) -> bool {
        self.question.kind != "free_text"
    }

    /// The correct answer spelled out for a result card.
    pub fn correct_answer(&self) -> String {
        if self.is_choice() {
            self.answer
                .parse::<usize>()
                .ok()
                .and_then(|i| self.question.options.get(i))
                .cloned()
                .unwrap_or_else(|| self.answer.clone())
        } else {
            self.answer.clone()
        }
    }
}

#[derive(Debug, Default)]
pub struct Bank {
    pub questions: Vec<StoredQuestion>,
}

impl Bank {
    pub fn load(dir: &Path) -> Result<Self, StudyError> {
        let path = dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(StudyError::io("read", path, source)),
        };
        let mut questions = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            // A damaged line loses one question, not the whole cache.
            if let Ok(q) = serde_json::from_str::<StoredQuestion>(line) {
                questions.push(q);
            }
        }
        Ok(Self { questions })
    }

    pub fn save(&self, dir: &Path) -> Result<(), StudyError> {
        let mut out = String::new();
        for q in &self.questions {
            let line = serde_json::to_string(q).map_err(|source| StudyError::Json {
                op: "encode questions.jsonl",
                source,
            })?;
            out.push_str(&line);
            out.push('\n');
        }
        harness_config::io::atomic_write(&dir.join(FILE), out.as_bytes())?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&StoredQuestion> {
        self.questions.iter().find(|q| q.question.id == id)
    }

    /// Add questions, skipping ids already present.
    pub fn push_new(&mut self, fresh: Vec<StoredQuestion>) -> usize {
        let mut added = 0;
        for q in fresh {
            if self.get(&q.question.id).is_none() {
                self.questions.push(q);
                added += 1;
            }
        }
        added
    }

    pub fn mark_asked(&mut self, id: &str, verdict: Verdict, at: i64) {
        if let Some(q) = self.questions.iter_mut().find(|q| q.question.id == id) {
            q.asked += 1;
            q.last_verdict = Some(verdict);
            q.last_asked_at = Some(at);
        }
    }

    /// Up to `n` questions for `territory` (any territory when `None`),
    /// never one in `exclude`: unasked first, then missed, then the
    /// longest-unasked, skipping anything answered fully in the last days.
    pub fn pick(
        &self,
        territory: Option<&str>,
        exclude: &[String],
        n: usize,
        now: i64,
    ) -> Vec<&StoredQuestion> {
        let mut candidates: Vec<&StoredQuestion> = self
            .questions
            .iter()
            .filter(|q| territory.is_none_or(|t| q.question.territory == t))
            .filter(|q| !exclude.contains(&q.question.id))
            .filter(|q| {
                !(q.last_verdict == Some(Verdict::Full)
                    && q.last_asked_at.is_some_and(|at| now - at < RECENT_SECS))
            })
            .collect();
        candidates.sort_by_key(|q| (rank(q), q.last_asked_at.unwrap_or(0)));
        candidates.truncate(n);
        candidates
    }

    /// Questions missed or half-answered before, across every territory,
    /// oldest miss first — the review mode's first draw.
    pub fn missed(&self, exclude: &[String], n: usize) -> Vec<&StoredQuestion> {
        let mut out: Vec<&StoredQuestion> = self
            .questions
            .iter()
            .filter(|q| {
                matches!(
                    q.last_verdict,
                    Some(Verdict::Wrong) | Some(Verdict::Partial)
                )
            })
            .filter(|q| !exclude.contains(&q.question.id))
            .collect();
        out.sort_by_key(|q| q.last_asked_at.unwrap_or(0));
        out.truncate(n);
        out
    }
}

fn rank(q: &StoredQuestion) -> u8 {
    match q.last_verdict {
        None => 0,
        Some(Verdict::Wrong) => 1,
        Some(Verdict::Partial) => 2,
        Some(Verdict::Full) => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn stored(id: &str, territory: &str) -> StoredQuestion {
        StoredQuestion {
            question: StudyQuestion {
                id: id.into(),
                territory: territory.into(),
                kind: "multiple_choice".into(),
                prompt: format!("q {id}?"),
                options: vec!["a".into(), "b".into()],
                source_path: "src/lib.rs".into(),
                source_lines: Some((1, 3)),
                source_excerpt: "fn a() {}".into(),
                difficulty: 1,
                cached: false,
            },
            answer: "1".into(),
            explanation: "because".into(),
            created_at: 0,
            model: "m".into(),
            asked: 0,
            last_verdict: None,
            last_asked_at: None,
        }
    }

    #[test]
    fn picking_prefers_unasked_then_missed_and_skips_recent_hits() {
        let mut bank = Bank::default();
        bank.push_new(vec![
            stored("hit", "t"),
            stored("miss", "t"),
            stored("new", "t"),
            stored("other", "u"),
        ]);
        bank.mark_asked("hit", Verdict::Full, 1000);
        bank.mark_asked("miss", Verdict::Wrong, 900);
        let ids: Vec<&str> = bank
            .pick(Some("t"), &[], 5, 1000)
            .iter()
            .map(|q| q.question.id.as_str())
            .collect();
        assert_eq!(ids, vec!["new", "miss"]);
        // Weeks later the hit is fair game again, after the others.
        let later: Vec<&str> = bank
            .pick(Some("t"), &["new".to_string()], 5, 1000 + 30 * 24 * 3600)
            .iter()
            .map(|q| q.question.id.as_str())
            .collect();
        assert_eq!(later, vec!["miss", "hit"]);
        assert_eq!(bank.missed(&[], 5).len(), 1);
    }

    #[test]
    fn the_cache_round_trips_and_survives_a_bad_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut bank = Bank::default();
        assert_eq!(bank.push_new(vec![stored("a", "t"), stored("a", "t")]), 1);
        bank.save(dir.path()).unwrap();
        let path = dir.path().join(FILE);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{not json\n");
        std::fs::write(&path, text).unwrap();
        let back = Bank::load(dir.path()).unwrap();
        assert_eq!(back.questions.len(), 1);
        assert_eq!(back.get("a").unwrap().correct_answer(), "b");
    }
}
