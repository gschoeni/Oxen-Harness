//! Judging an answer: choice questions are graded here without a model;
//! free text goes to the study model with the reference answer and comes
//! back as full / partial / wrong plus one line of feedback.

use harness_protocol::StudyGrade;
use serde::Deserialize;

use crate::bank::StoredQuestion;
use crate::generate::{extract_object, truncate_chars};
use crate::progress::Verdict;

pub const GRADER_SYSTEM: &str = "You grade a developer's short answer to a question about their \
codebase. Judge meaning, not wording: an answer that names the right mechanism in different \
words is full; one that has the right idea but misses a key part, or is right for the wrong \
reason, is partial; anything else is wrong. Reply with a JSON object only: {\"verdict\": \
\"full\"|\"partial\"|\"wrong\", \"feedback\": one sentence that corrects or confirms}.";

/// A judged answer: the verdict as the progress file records it, and the
/// grade as the client shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graded {
    pub verdict: Verdict,
    pub grade: StudyGrade,
}

fn graded(verdict: Verdict, feedback: String, q: &StoredQuestion) -> Graded {
    Graded {
        verdict,
        grade: StudyGrade {
            verdict: verdict.as_str().to_string(),
            feedback,
            correct_answer: q.correct_answer(),
            explanation: q.explanation.clone(),
        },
    }
}

/// Grade a choice answer given as an index, an option's text, a letter
/// (`A`-`D`), or `true`/`false`.
pub fn grade_choice(q: &StoredQuestion, answer: &str) -> Graded {
    let picked = choice_index(q, answer);
    let correct: Option<usize> = q.answer.parse().ok();
    let verdict = if picked.is_some() && picked == correct {
        Verdict::Full
    } else {
        Verdict::Wrong
    };
    // The result card shows the correct answer on its own line, so the
    // feedback doesn't repeat it.
    let feedback = match verdict {
        Verdict::Full => "Right.",
        _ => "Not that one.",
    };
    graded(verdict, feedback.to_string(), q)
}

fn choice_index(q: &StoredQuestion, answer: &str) -> Option<usize> {
    let a = answer.trim();
    if a.is_empty() {
        return None;
    }
    if let Ok(i) = a.parse::<usize>() {
        return (i < q.question.options.len()).then_some(i);
    }
    if a.len() == 1 {
        let letter = a.to_ascii_uppercase().as_bytes()[0];
        if (b'A'..=b'D').contains(&letter) {
            let i = (letter - b'A') as usize;
            return (i < q.question.options.len()).then_some(i);
        }
    }
    q.question
        .options
        .iter()
        .position(|o| o.eq_ignore_ascii_case(a))
}

/// The grader prompt for a free-text answer.
pub fn free_text_prompt(q: &StoredQuestion, answer: &str) -> String {
    format!(
        "Question: {}\nSource: {}{}\nReference answer: {}\nExplanation: {}\n\nThe developer \
         answered: {}",
        q.question.prompt,
        q.question.source_path,
        q.question
            .source_lines
            .map(|(a, b)| format!(" (lines {a}-{b})"))
            .unwrap_or_default(),
        q.answer,
        q.explanation,
        truncate_chars(answer.trim(), 1_000)
    )
}

#[derive(Deserialize)]
struct RawGrade {
    verdict: String,
    #[serde(default)]
    feedback: String,
}

/// Read the grader's reply. An unparseable reply is scored partial with the
/// reference answer as feedback, so a flaky grader never zeroes a player.
pub fn parse_free_text(reply: &str, q: &StoredQuestion) -> Graded {
    let parsed = extract_object(reply)
        .and_then(|json| serde_json::from_str::<RawGrade>(json).ok())
        .and_then(|raw| Verdict::parse(&raw.verdict).map(|v| (v, raw.feedback)));
    let (verdict, feedback) = match parsed {
        Some((v, feedback)) if !feedback.trim().is_empty() => (v, feedback.trim().to_string()),
        Some((v, _)) => (v, format!("The reference answer: {}", q.answer)),
        None => (
            Verdict::Partial,
            format!(
                "Couldn't grade that reliably — the reference answer: {}",
                q.answer
            ),
        ),
    };
    graded(verdict, feedback, q)
}

/// A blank free-text answer is wrong without asking anyone.
pub fn blank_grade(q: &StoredQuestion) -> Graded {
    graded(Verdict::Wrong, "No answer given.".into(), q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_protocol::StudyQuestion;

    fn choice() -> StoredQuestion {
        StoredQuestion {
            question: StudyQuestion {
                id: "q".into(),
                territory: "t".into(),
                kind: "multiple_choice".into(),
                prompt: "?".into(),
                options: vec!["alpha".into(), "beta".into(), "gamma".into()],
                source_path: "x".into(),
                source_lines: None,
                source_excerpt: String::new(),
                difficulty: 1,
                cached: false,
            },
            answer: "1".into(),
            explanation: "why".into(),
            created_at: 0,
            model: "m".into(),
            asked: 0,
            last_verdict: None,
            last_asked_at: None,
        }
    }

    #[test]
    fn choice_answers_accept_index_letter_or_text() {
        let q = choice();
        for a in ["1", "b", "B", "Beta"] {
            assert_eq!(grade_choice(&q, a).grade.verdict, "full", "{a}");
        }
        let wrong = grade_choice(&q, "gamma");
        assert_eq!(wrong.verdict, Verdict::Wrong);
        let wrong = wrong.grade;
        assert_eq!(wrong.correct_answer, "beta");
        assert_eq!(wrong.feedback, "Not that one.");
        assert_eq!(grade_choice(&q, "9").verdict, Verdict::Wrong);
    }

    #[test]
    fn free_text_grades_parse_leniently() {
        let mut q = choice();
        q.question.kind = "free_text".into();
        q.answer = "the sandbox".into();
        let full = parse_free_text("Sure: {\"verdict\":\"full\",\"feedback\":\"Yes.\"}", &q).grade;
        assert_eq!(
            (full.verdict.as_str(), full.feedback.as_str()),
            ("full", "Yes.")
        );
        let bare = parse_free_text("{\"verdict\":\"wrong\"}", &q).grade;
        assert_eq!(bare.verdict, "wrong");
        assert!(bare.feedback.contains("the sandbox"));
        let junk = parse_free_text("I cannot", &q);
        assert_eq!(junk.verdict, Verdict::Partial);
        assert!(free_text_prompt(&q, "  it's the sandbox ").ends_with("answered: it's the sandbox"));
    }
}
