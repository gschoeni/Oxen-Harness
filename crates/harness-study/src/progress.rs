//! The mastery model and its per-project file.
//!
//! Each territory keeps the list of attempts made on it. Mastery is derived
//! from that list at read time, so changing the formula never needs a
//! migration: walk the attempts in order, letting the score decay between
//! them (it halves every [`HALF_LIFE_SECS`]), nudging it up on a correct
//! answer and down on a wrong one, then decay once more to *now*. A
//! territory nobody has practiced in a month fades back toward zero, which
//! is exactly the signal the review mode and the "fading" trail events use.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use harness_protocol::{StudyProfile, StudyTerritory};
use serde::{Deserialize, Serialize};

use crate::territories::Territory;
use crate::StudyError;

/// Mastery halves every two weeks without a correct answer.
pub const HALF_LIFE_SECS: f64 = 14.0 * 24.0 * 3600.0;

const FILE: &str = "progress.json";
pub const SCHEMA_VERSION: u32 = 1;

/// How an answer was judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Full,
    Partial,
    Wrong,
}

impl Verdict {
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "full" | "correct" | "right" => Some(Self::Full),
            "partial" | "partially_correct" | "close" => Some(Self::Partial),
            "wrong" | "incorrect" => Some(Self::Wrong),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
            Self::Wrong => "wrong",
        }
    }
}

/// One answered question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    /// Unix seconds.
    pub at: i64,
    pub verdict: Verdict,
    pub question_id: String,
    #[serde(default)]
    pub hint_used: bool,
    /// The question's format, which decides how much a right answer is
    /// worth: picking from four can be a guess, typing can't. Absent in
    /// attempts recorded before formats were weighed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TerritoryProgress {
    #[serde(default)]
    pub attempts: Vec<Attempt>,
}

/// The per-project progress file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub territories: BTreeMap<String, TerritoryProgress>,
}

impl Progress {
    pub fn load(dir: &Path) -> Result<Self, StudyError> {
        let path = dir.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| StudyError::Json {
                op: "parse progress.json",
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(StudyError::io("read", path, source)),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<(), StudyError> {
        let mut copy = self.clone();
        copy.schema_version = SCHEMA_VERSION;
        let bytes = serde_json::to_vec_pretty(&copy).map_err(|source| StudyError::Json {
            op: "encode progress.json",
            source,
        })?;
        harness_config::io::atomic_write(&dir.join(FILE), &bytes)?;
        Ok(())
    }

    pub fn record(&mut self, territory: &str, attempt: Attempt) {
        self.territories
            .entry(territory.to_string())
            .or_default()
            .attempts
            .push(attempt);
    }

    /// Drop every attempt at `question_id` — it was flagged as wrong, so
    /// nothing learned from it (in either direction) should stand.
    pub fn void(&mut self, question_id: &str) -> usize {
        let mut removed = 0;
        for territory in self.territories.values_mut() {
            let before = territory.attempts.len();
            territory.attempts.retain(|a| a.question_id != question_id);
            removed += before - territory.attempts.len();
        }
        removed
    }

    /// The profile a host shows: every discovered territory (unexplored ones
    /// at zero), plus any territory the file knows that discovery no longer
    /// finds (a renamed crate keeps its history). `due` reports how many
    /// reviews are waiting in a territory.
    pub fn profile(
        &self,
        project: &str,
        workspace: &Path,
        territories: &[Territory],
        due: &dyn Fn(&str) -> u32,
        now: i64,
    ) -> StudyProfile {
        let mut rows: Vec<StudyTerritory> = territories
            .iter()
            .map(|t| self.territory_row(&t.id, &t.name, t.files.len(), due(&t.id), now))
            .collect();
        for id in self.territories.keys() {
            if !territories.iter().any(|t| &t.id == id) {
                rows.push(self.territory_row(id, id, 1, due(id), now));
            }
        }
        let answered = rows.iter().map(|r| r.answered).sum();
        let understanding = understanding_of(&rows);
        StudyProfile {
            project: project.to_string(),
            workspace: workspace.display().to_string(),
            understanding,
            level: level_for(understanding),
            answered,
            due: rows.iter().map(|r| r.due).sum(),
            territories: rows,
        }
    }

    fn territory_row(
        &self,
        id: &str,
        name: &str,
        files: usize,
        due: u32,
        now: i64,
    ) -> StudyTerritory {
        let attempts = self
            .territories
            .get(id)
            .map(|t| t.attempts.as_slice())
            .unwrap_or(&[]);
        let m = mastery_at(attempts, files, now);
        StudyTerritory {
            id: id.to_string(),
            name: name.to_string(),
            mastery: m.now,
            answered: attempts.len() as u32,
            correct: attempts
                .iter()
                .filter(|a| a.verdict == Verdict::Full)
                .count() as u32,
            last_answered_at: attempts.last().map(|a| a.at),
            faded: m.faded(),
            files: files as u32,
            due,
        }
    }
}

/// A territory's mastery, now and at its best.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mastery {
    pub now: f32,
    pub peak: f32,
}

impl Mastery {
    /// How far mastery has fallen from its peak, 0..1.
    pub fn faded(self) -> f32 {
        if self.peak <= 0.0 {
            0.0
        } else {
            ((self.peak - self.now) / self.peak).clamp(0.0, 1.0)
        }
    }
}

fn decay(value: f64, seconds: i64) -> f64 {
    if seconds <= 0 {
        return value;
    }
    value * 0.5f64.powf(seconds as f64 / HALF_LIFE_SECS)
}

/// How much of the remaining distance to mastery one full answer covers,
/// by format. Recognition is cheaper than recall and partly luck: a guess
/// at four options is right a quarter of the time, at true/false half.
fn full_gain(kind: Option<&str>) -> f64 {
    match kind {
        Some("true_false") => 0.18,
        Some("multiple_choice") => 0.28,
        Some("order") => 0.32,
        _ => 0.35,
    }
}

/// A bigger region takes more answers to know: each one covers less of it.
/// Two files → ~1.25, forty → ~2.3, two hundred → ~2.9.
fn depth_of(files: usize) -> f64 {
    1.0 + (files.max(1) as f64).log2() / 4.0
}

/// Replay `attempts` (in order) to the mastery at `now` of a region of
/// `files` source files.
pub fn mastery_at(attempts: &[Attempt], files: usize, now: i64) -> Mastery {
    let depth = depth_of(files);
    let mut m = 0.0f64;
    let mut peak = 0.0f64;
    let mut last: Option<i64> = None;
    for a in attempts {
        if let Some(prev) = last {
            m = decay(m, a.at - prev);
        }
        let full = full_gain(a.kind.as_deref());
        m = match a.verdict {
            Verdict::Full => m + (1.0 - m) * if a.hint_used { full * 0.6 } else { full } / depth,
            Verdict::Partial => m + (1.0 - m) * full * 0.4 / depth,
            Verdict::Wrong => m * 0.7,
        };
        peak = peak.max(m);
        last = Some(a.at);
    }
    if let Some(prev) = last {
        m = decay(m, now - prev);
    }
    Mastery {
        now: m.clamp(0.0, 1.0) as f32,
        peak: peak.clamp(0.0, 1.0) as f32,
    }
}

/// Understanding is mastery averaged across every territory, weighted by
/// the square root of its size — a two-file crate shouldn't count like the
/// whole frontend, nor the frontend drown everything else. Unexplored
/// regions count as zero, so it reads as coverage as much as depth.
pub fn understanding_of(rows: &[StudyTerritory]) -> f32 {
    let weight = |r: &StudyTerritory| (r.files.max(1) as f32).sqrt();
    let total: f32 = rows.iter().map(weight).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let sum: f32 = rows.iter().map(|r| r.mastery * weight(r)).sum();
    (sum / total * 100.0).clamp(0.0, 100.0)
}

/// Levels start at 1 and rise every eight points of understanding.
pub fn level_for(understanding: f32) -> u32 {
    1 + (understanding / 8.0).floor() as u32
}

/// `<dirname>-<8 hex>`: readable, and unique per absolute path.
pub fn project_key(workspace: &Path) -> String {
    let name = workspace
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    format!(
        "{name}-{}",
        crate::short_hash(&workspace.display().to_string(), 8)
    )
}

/// `~/.oxen-harness/study/<project-key>/`, created on demand.
pub fn project_dir(workspace: &Path) -> Result<PathBuf, StudyError> {
    let dir = harness_config::paths::study_dir()?.join(project_key(workspace));
    std::fs::create_dir_all(&dir).map_err(|e| StudyError::io("create", dir.clone(), e))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 3600;

    fn attempt(at: i64, verdict: Verdict) -> Attempt {
        Attempt {
            at,
            verdict,
            question_id: "q".into(),
            hint_used: false,
            kind: Some("free_text".into()),
        }
    }

    fn territory(id: &str, files: usize) -> Territory {
        Territory {
            id: id.into(),
            name: id.into(),
            files: (0..files).map(|i| format!("{id}/f{i}.rs")).collect(),
        }
    }

    #[test]
    fn correct_answers_raise_mastery_and_wrong_ones_lower_it() {
        let one = mastery_at(&[attempt(0, Verdict::Full)], 1, 0);
        assert!((one.now - 0.35).abs() < 1e-4);
        let two = mastery_at(
            &[attempt(0, Verdict::Full), attempt(1, Verdict::Full)],
            1,
            1,
        );
        assert!(two.now > one.now);
        let missed = mastery_at(
            &[attempt(0, Verdict::Full), attempt(1, Verdict::Wrong)],
            1,
            1,
        );
        assert!(missed.now < one.now);
        assert!(missed.faded() > 0.0);
        let partial = mastery_at(&[attempt(0, Verdict::Partial)], 1, 0);
        assert!(partial.now < one.now && partial.now > 0.0);
    }

    #[test]
    fn mastery_halves_every_two_weeks_without_practice() {
        let fresh = mastery_at(&[attempt(0, Verdict::Full)], 1, 0);
        let later = mastery_at(&[attempt(0, Verdict::Full)], 1, 14 * DAY);
        assert!((later.now - fresh.now / 2.0).abs() < 1e-3);
        assert!((later.faded() - 0.5).abs() < 1e-3);
        assert_eq!(later.peak, fresh.peak);
    }

    #[test]
    fn hints_guessable_formats_and_big_regions_earn_less() {
        let of = |kind: &str, hint: bool, files: usize| {
            mastery_at(
                &[Attempt {
                    hint_used: hint,
                    kind: Some(kind.into()),
                    ..attempt(0, Verdict::Full)
                }],
                files,
                0,
            )
            .now
        };
        assert!(of("free_text", true, 1) < of("free_text", false, 1));
        assert!(of("true_false", false, 1) < of("multiple_choice", false, 1));
        assert!(of("multiple_choice", false, 1) < of("order", false, 1));
        assert!(of("order", false, 1) < of("free_text", false, 1));
        assert!(of("free_text", false, 200) < of("free_text", false, 2));
        // An attempt from before formats were recorded keeps its old worth.
        let legacy = Attempt {
            kind: None,
            ..attempt(0, Verdict::Full)
        };
        assert!((mastery_at(&[legacy], 1, 0).now - 0.35).abs() < 1e-4);
    }

    #[test]
    fn the_profile_weighs_regions_by_size_and_derives_a_level() {
        let mut p = Progress::default();
        p.record("small", attempt(0, Verdict::Full));
        let territories = vec![territory("small", 1), territory("big", 9)];
        let profile = p.profile(
            "proj",
            Path::new("/x"),
            &territories,
            &|id| u32::from(id == "big"),
            0,
        );
        assert_eq!(profile.territories.len(), 2);
        assert_eq!(profile.answered, 1);
        assert_eq!(profile.due, 1);
        assert_eq!(profile.territories[1].files, 9);
        // 0.35 in the small region, weights 1 and 3: 8.75%, level 2.
        assert!(
            (profile.understanding - 8.75).abs() < 0.01,
            "{}",
            profile.understanding
        );
        assert_eq!(profile.level, 2);
        assert_eq!(profile.territories[1].mastery, 0.0);
    }

    #[test]
    fn voiding_a_question_removes_what_it_taught() {
        let mut p = Progress::default();
        p.record("a", attempt(0, Verdict::Wrong));
        p.record(
            "a",
            Attempt {
                question_id: "other".into(),
                ..attempt(1, Verdict::Full)
            },
        );
        assert_eq!(p.void("q"), 1);
        assert_eq!(p.territories["a"].attempts.len(), 1);
        assert_eq!(p.void("q"), 0);
    }

    #[test]
    fn progress_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Progress::default();
        p.record("docs", attempt(5, Verdict::Partial));
        p.save(dir.path()).unwrap();
        let back = Progress::load(dir.path()).unwrap();
        assert_eq!(back.territories["docs"].attempts.len(), 1);
        assert_eq!(
            back.territories["docs"].attempts[0].kind.as_deref(),
            Some("free_text")
        );
        assert_eq!(back.schema_version, SCHEMA_VERSION);
        assert!(Progress::load(&dir.path().join("missing"))
            .unwrap()
            .territories
            .is_empty());
    }

    #[test]
    fn project_keys_are_readable_and_path_unique() {
        let a = project_key(Path::new("/home/me/code/oxen"));
        let b = project_key(Path::new("/home/you/code/oxen"));
        assert!(a.starts_with("oxen-"));
        assert_ne!(a, b);
    }
}
