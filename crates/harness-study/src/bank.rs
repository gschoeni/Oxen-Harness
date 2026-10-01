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

const DAY: i64 = 24 * 3600;

/// How long a question rests after a full answer, by the box it has reached:
/// each success pushes the next review further out, a miss starts it over.
const INTERVALS: [i64; 5] = [DAY, 3 * DAY, 7 * DAY, 14 * DAY, 30 * DAY];
/// A missed question is due again almost at once — next run, not next week.
const MISS_INTERVAL: i64 = 10 * 60;

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
    /// The player marked it wrong or unfair. It is never asked again, but it
    /// stays so the writer is told not to produce it a second time.
    #[serde(default)]
    pub flagged: bool,
    /// How many times in a row it has been answered in full (capped at the
    /// last interval) — the spaced-repetition box.
    #[serde(default)]
    pub box_level: u8,
    /// When it is next worth asking; `None` until first asked.
    #[serde(default)]
    pub due_at: Option<i64>,
}

impl StoredQuestion {
    /// Answered by picking one option (as opposed to typing).
    pub fn is_choice(&self) -> bool {
        matches!(
            self.question.kind.as_str(),
            "multiple_choice" | "true_false"
        )
    }

    pub fn is_order(&self) -> bool {
        self.question.kind == "order"
    }

    /// Whether it may be asked at `now`: never asked, or its rest is over.
    pub fn is_due(&self, now: i64) -> bool {
        !self.flagged && self.due_at.is_none_or(|due| due <= now)
    }

    /// The correct answer spelled out for a result card.
    pub fn correct_answer(&self) -> String {
        if self.is_order() {
            self.answer
                .chars()
                .map(String::from)
                .collect::<Vec<_>>()
                .join(" → ")
        } else if self.is_choice() {
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
    /// Lines that didn't parse, kept verbatim and written back on save: a
    /// line this build can't read (damage, or a newer schema) is skipped,
    /// never erased.
    unreadable: Vec<String>,
}

impl Bank {
    pub fn load(dir: &Path) -> Result<Self, StudyError> {
        let path = dir.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(StudyError::io("read", path, source)),
        };
        let mut bank = Self::default();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<StoredQuestion>(line) {
                Ok(q) => bank.questions.push(q),
                Err(_) => bank.unreadable.push(line.to_string()),
            }
        }
        Ok(bank)
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
        for line in &self.unreadable {
            out.push_str(line);
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

    /// Record an answer and schedule the question's next review.
    pub fn mark_asked(&mut self, id: &str, verdict: Verdict, hint_used: bool, at: i64) {
        let Some(q) = self.questions.iter_mut().find(|q| q.question.id == id) else {
            return;
        };
        q.asked += 1;
        q.last_verdict = Some(verdict);
        q.last_asked_at = Some(at);
        let rest = match verdict {
            // A hinted answer was read, not recalled: it earns no promotion.
            Verdict::Full if hint_used => {
                INTERVALS[(q.box_level as usize).min(INTERVALS.len() - 1)]
            }
            Verdict::Full => {
                let rest = INTERVALS[(q.box_level as usize).min(INTERVALS.len() - 1)];
                q.box_level = (q.box_level + 1).min(INTERVALS.len() as u8 - 1);
                rest
            }
            Verdict::Partial => {
                q.box_level = q.box_level.saturating_sub(1);
                DAY
            }
            Verdict::Wrong => {
                q.box_level = 0;
                MISS_INTERVAL
            }
        };
        q.due_at = Some(at + rest);
    }

    /// Mark a question wrong or unfair. Returns the territory it belonged to.
    pub fn flag(&mut self, id: &str) -> Option<String> {
        let q = self.questions.iter_mut().find(|q| q.question.id == id)?;
        q.flagged = true;
        Some(q.question.territory.clone())
    }

    /// Up to `n` askable questions for `territory`, never one in `exclude`:
    /// the most overdue reviews first, then questions never asked. A
    /// question still resting after a good answer is left alone.
    pub fn pick(
        &self,
        territory: &str,
        exclude: &[String],
        n: usize,
        now: i64,
    ) -> Vec<&StoredQuestion> {
        let mut candidates: Vec<&StoredQuestion> = self
            .questions
            .iter()
            .filter(|q| q.question.territory == territory)
            .filter(|q| q.is_due(now) && !exclude.contains(&q.question.id))
            .collect();
        candidates.sort_by_key(|q| (q.due_at.is_none(), q.due_at.unwrap_or(0)));
        candidates.truncate(n);
        candidates
    }

    /// Reviews that have come due, across every territory, most overdue
    /// first — the review mode's draw.
    pub fn due(&self, exclude: &[String], n: usize, now: i64) -> Vec<&StoredQuestion> {
        let mut out: Vec<&StoredQuestion> = self
            .questions
            .iter()
            .filter(|q| q.due_at.is_some() && q.is_due(now))
            .filter(|q| !exclude.contains(&q.question.id))
            .collect();
        out.sort_by_key(|q| q.due_at.unwrap_or(0));
        out.truncate(n);
        out
    }

    /// How many reviews are due in `territory` right now.
    pub fn due_count(&self, territory: &str, now: i64) -> u32 {
        self.questions
            .iter()
            .filter(|q| q.question.territory == territory)
            .filter(|q| q.due_at.is_some() && q.is_due(now))
            .count() as u32
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
            flagged: false,
            box_level: 0,
            due_at: None,
        }
    }

    #[test]
    fn answers_schedule_the_next_review_and_picking_follows_the_schedule() {
        let mut bank = Bank::default();
        bank.push_new(vec![
            stored("hit", "t"),
            stored("miss", "t"),
            stored("new", "t"),
            stored("other", "u"),
        ]);
        bank.mark_asked("hit", Verdict::Full, false, 1000);
        bank.mark_asked("miss", Verdict::Wrong, false, 1000);
        assert_eq!(bank.get("hit").unwrap().due_at, Some(1000 + DAY));
        assert_eq!(bank.get("hit").unwrap().box_level, 1);
        let ids = |found: Vec<&StoredQuestion>| -> Vec<String> {
            found.iter().map(|q| q.question.id.clone()).collect()
        };
        // Right away: the hit rests, the miss isn't due for ten minutes.
        assert_eq!(ids(bank.pick("t", &[], 5, 1000)), vec!["new"]);
        // An hour on the miss is back, ahead of what was never asked.
        assert_eq!(
            ids(bank.pick("t", &[], 5, 1000 + 3600)),
            vec!["miss", "new"]
        );
        assert_eq!(ids(bank.due(&[], 5, 1000 + 3600)), vec!["miss"]);
        assert_eq!(bank.due_count("t", 1000 + 2 * DAY), 2);
        // A second full answer pushes the next review out to three days.
        bank.mark_asked("hit", Verdict::Full, false, 1000 + DAY);
        assert_eq!(bank.get("hit").unwrap().due_at, Some(1000 + DAY + 3 * DAY));
        // A hinted answer earns no promotion; a miss starts over.
        bank.mark_asked("hit", Verdict::Full, true, 2000);
        assert_eq!(bank.get("hit").unwrap().box_level, 2);
        bank.mark_asked("hit", Verdict::Wrong, false, 3000);
        assert_eq!(bank.get("hit").unwrap().box_level, 0);
    }

    #[test]
    fn a_flagged_question_is_never_asked_again() {
        let mut bank = Bank::default();
        bank.push_new(vec![stored("bad", "t")]);
        assert_eq!(bank.flag("bad").as_deref(), Some("t"));
        assert!(bank.pick("t", &[], 5, 0).is_empty());
        assert!(bank.due(&[], 5, i64::MAX).is_empty());
        assert_eq!(bank.flag("nope"), None);
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
        // Saving again keeps the line this build couldn't read.
        back.save(dir.path()).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("{not json"));
        assert_eq!(back.get("a").unwrap().correct_answer(), "b");
    }
}
