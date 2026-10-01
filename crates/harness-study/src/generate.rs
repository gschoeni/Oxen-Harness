//! Writing questions: the prompt the study model answers with a JSON array,
//! and the lenient parser that turns its reply into stored questions.

use std::path::Path;

use harness_protocol::StudyQuestion;
use serde::Deserialize;

use crate::bank::StoredQuestion;
use crate::material::Material;
use crate::territories::{territory_for, Territory};
use crate::StudyError;

pub const SYSTEM: &str = "You write short quiz questions that help a developer understand a \
codebase they are working in. Every question must be answerable from the source you are given \
and must point at the file that holds the answer. Test understanding, not memory of names: \
what happens when this runs, what breaks if it changes, why it is shaped this way, how one \
file relies on another, where you would go to change a behavior. Never ask about line numbers, \
counts, or the exact spelling of an identifier. The questions are read on a small screen: keep \
each prompt under 220 characters and each option under 100. Reply with a JSON array only — no \
prose, no code fences.";

pub const VERIFY_SYSTEM: &str = "You check quiz questions about a codebase against the source \
they were written from. A question fails if its marked answer is wrong, if another option is \
equally right, if it can't be answered from the source, or if it is ambiguous. Be strict about \
correctness and lenient about style. Reply with a JSON array only: one {\"index\": number, \
\"ok\": true|false, \"reason\": short string} per question.";

/// The most lines a hint excerpt shows.
const EXCERPT_LINES: usize = 12;
const EXCERPT_CHARS: usize = 600;

/// How a batch of `count` splits across formats. Recall beats recognition,
/// so typed answers and orderings take the places true/false used to; a
/// lone question is multiple choice, the quickest to answer.
fn mix(count: usize) -> (usize, usize, usize) {
    let order = usize::from(count >= 4);
    let free_text = match count {
        0..=1 => 0,
        2..=4 => 1,
        _ => 2,
    };
    (count - order - free_text, free_text, order)
}

/// The prompt for `count` questions from `material`. `avoid` lists prompts
/// already in the bank for this region so the model steers clear of them.
pub fn prompt(material: &Material, count: usize, avoid: &[String]) -> String {
    let (choice, free_text, order) = mix(count);
    let where_ = if material.numbered {
        "\"source_lines\": [first, last] using the line numbers printed before each `|`"
    } else {
        "\"excerpt\": the one to six lines of the material that hold the answer, quoted verbatim"
    };
    let across = if material.files.len() > 1 {
        " At least one question must connect two of the files (how one uses the other), and \
         one multiple_choice should ask which file you would open to change a behavior."
    } else {
        ""
    };
    let mut out = format!(
        "{}\n\nWrite exactly {count} questions: {choice} multiple_choice (4 options, one \
         correct, plausible distractors — make at least one a \"what happens if…\" or \"what \
         does this return when…\" prediction), {free_text} free_text (answered in a few words \
         to a sentence, with a reference answer), {order} order (three to five steps of \
         something the code does, listed in the correct order; if the material has no real \
         sequence, write a multiple_choice instead).{across} Vary difficulty 1-3.\n\n\
         Each element: {{\"kind\": \"multiple_choice\"|\"free_text\"|\"order\", \"prompt\": \
         string, \"options\": [string] (the choices; for order, the steps in their correct \
         order; empty for free_text), \"answer\": 0-based index of the correct option (the \
         reference answer string for free_text; omit for order), \"explanation\": one or two \
         sentences, \"source_path\": the file the answer is in, exactly as it appears in the \
         material, {where}, \"difficulty\": 1|2|3}}.\n",
        material.framing,
        where = where_
    );
    if !avoid.is_empty() {
        out.push_str("\nDo not repeat these existing questions:\n");
        for a in avoid.iter().rev().take(20) {
            out.push_str("- ");
            out.push_str(&truncate_chars(a, 100));
            out.push('\n');
        }
    }
    out.push_str("\n---\n\n");
    out.push_str(&material.text);
    out
}

/// The checker's prompt: each question with its marked answer, then the
/// source it was written from.
pub fn verify_prompt(questions: &[StoredQuestion], material: &Material) -> String {
    let mut out = String::from("Questions to check:\n\n");
    for (index, q) in questions.iter().enumerate() {
        out.push_str(&format!(
            "[{index}] ({}) {}\n",
            q.question.kind, q.question.prompt
        ));
        for (i, option) in q.question.options.iter().enumerate() {
            out.push_str(&format!("    {}. {option}\n", i + 1));
        }
        out.push_str(&format!(
            "    marked answer: {}\n    file: {}\n\n",
            q.correct_answer(),
            q.question.source_path
        ));
    }
    out.push_str("---\n\nThe source:\n\n");
    out.push_str(&material.text);
    out
}

#[derive(Debug, Deserialize)]
struct RawCheck {
    index: usize,
    ok: bool,
}

/// The indices the checker rejected, or `None` when its reply can't be
/// read — in which case the caller keeps every question rather than guess.
pub fn rejected(reply: &str) -> Option<Vec<usize>> {
    let start = reply.find('[')?;
    let end = reply.rfind(']')?;
    let checks: Vec<RawCheck> = serde_json::from_str(reply.get(start..=end)?).ok()?;
    Some(
        checks
            .into_iter()
            .filter(|c| !c.ok)
            .map(|c| c.index)
            .collect(),
    )
}

#[derive(Debug, Deserialize)]
struct Raw {
    kind: String,
    prompt: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    answer: serde_json::Value,
    #[serde(default)]
    explanation: String,
    #[serde(default)]
    source_path: String,
    #[serde(default)]
    source_lines: Option<Vec<u32>>,
    #[serde(default)]
    excerpt: Option<String>,
    #[serde(default)]
    difficulty: Option<u8>,
}

/// Parse a reply into stored questions. Bad elements are dropped; a reply
/// with no usable element is a model error the host can surface.
pub fn parse(
    reply: &str,
    material: &Material,
    territories: &[Territory],
    model: &str,
    now: i64,
    root: &Path,
) -> Result<Vec<StoredQuestion>, StudyError> {
    let raws = salvage(reply);
    if raws.is_empty() {
        return Err(StudyError::Model {
            model: model.to_string(),
            detail: if reply.trim().is_empty() {
                "the reply was empty (the model may have spent its whole reply budget thinking)"
                    .into()
            } else {
                format!(
                    "the reply held no question objects; it began: {}",
                    truncate_chars(reply.trim(), 160)
                )
            },
        });
    }
    let mut out = Vec::new();
    for raw in raws {
        if let Some(q) = validate(raw, material, territories, model, now, root) {
            out.push(q);
        }
    }
    if out.is_empty() {
        return Err(StudyError::Model {
            model: model.to_string(),
            detail: "the reply held no usable questions".into(),
        });
    }
    Ok(out)
}

fn validate(
    raw: Raw,
    material: &Material,
    territories: &[Territory],
    model: &str,
    now: i64,
    root: &Path,
) -> Option<StoredQuestion> {
    let prompt = raw.prompt.trim().to_string();
    if prompt.len() < 8 {
        return None;
    }
    let kind = raw.kind.trim().to_ascii_lowercase();
    let (kind, options, answer) = match kind.as_str() {
        "multiple_choice" | "true_false" => {
            let options: Vec<String> = if kind == "true_false" {
                vec!["True".into(), "False".into()]
            } else {
                raw.options
                    .iter()
                    .map(|o| o.trim().to_string())
                    .filter(|o| !o.is_empty())
                    .collect()
            };
            if options.len() < 2 || options.len() > 4 {
                return None;
            }
            let index = match &raw.answer {
                serde_json::Value::Number(n) => n.as_u64().map(|n| n as usize),
                serde_json::Value::String(s) => {
                    let s = s.trim();
                    s.parse::<usize>()
                        .ok()
                        .or_else(|| options.iter().position(|o| o.eq_ignore_ascii_case(s)))
                }
                serde_json::Value::Bool(b) if kind == "true_false" => Some(if *b { 0 } else { 1 }),
                _ => None,
            }?;
            if index >= options.len() {
                return None;
            }
            let (options, index) = if kind == "multiple_choice" {
                shuffled(options, index, &raw.prompt)
            } else {
                (options, index)
            };
            (kind, options, index.to_string())
        }
        "order" => {
            let steps: Vec<String> = raw
                .options
                .iter()
                .map(|o| o.trim().to_string())
                .filter(|o| !o.is_empty())
                .collect();
            if steps.len() < 3 || steps.len() > 5 {
                return None;
            }
            let (options, sequence) = shuffled_steps(steps, &raw.prompt);
            (kind, options, sequence)
        }
        "free_text" => {
            let reference = match &raw.answer {
                serde_json::Value::String(s) => s.trim().to_string(),
                other => other.to_string(),
            };
            if reference.is_empty() {
                return None;
            }
            (kind, Vec::new(), reference)
        }
        _ => return None,
    };
    let source_path = raw.source_path.trim().trim_start_matches("./").to_string();
    let source_path = if material.files.iter().any(|f| f == &source_path) {
        source_path
    } else {
        // A path the material never showed can't be trusted; pin the
        // question to the first excerpt so the hint has somewhere to go.
        material.files.first().cloned().unwrap_or(source_path)
    };
    // Line numbers only mean something when the material was file heads.
    let source_lines = raw
        .source_lines
        .as_ref()
        .filter(|_| material.numbered)
        .filter(|l| l.len() == 2 && l[0] >= 1 && l[1] >= l[0])
        .map(|l| (l[0], l[1]));
    let excerpt = excerpt_from_disk(root, &source_path, source_lines)
        .or_else(|| raw.excerpt.map(|e| truncate_chars(&e, EXCERPT_CHARS)))
        .unwrap_or_default();
    let id = crate::short_hash(&format!("{source_path}\n{prompt}"), 12);
    Some(StoredQuestion {
        question: StudyQuestion {
            id,
            // A diff or a ride-along spans regions: credit the one the
            // answer's file is in, not the batch's first file.
            territory: territory_for(territories, &source_path)
                .map(|t| t.id.clone())
                .unwrap_or_else(|| material.territory_id.clone()),
            kind,
            prompt,
            options,
            source_path,
            source_lines,
            source_excerpt: excerpt,
            difficulty: raw.difficulty.unwrap_or(2).clamp(1, 3),
            cached: false,
        },
        answer,
        explanation: raw.explanation.trim().to_string(),
        created_at: now,
        model: model.to_string(),
        asked: 0,
        last_verdict: None,
        last_asked_at: None,
        flagged: false,
        box_level: 0,
        due_at: None,
    })
}

/// Reorder a question's options and return where the correct one landed.
/// Models tend to write the right answer first, which turns a quiz into
/// "press 1"; the order is derived from the prompt so a cached question
/// keeps its layout between runs.
fn shuffled(options: Vec<String>, answer: usize, seed: &str) -> (Vec<String>, usize) {
    let mut keyed: Vec<(String, bool, String)> = options
        .into_iter()
        .enumerate()
        .map(|(i, option)| {
            let key = crate::short_hash(&format!("{seed}\n{option}"), 16);
            (key, i == answer, option)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let index = keyed
        .iter()
        .position(|(_, correct, _)| *correct)
        .unwrap_or(0);
    (
        keyed.into_iter().map(|(_, _, option)| option).collect(),
        index,
    )
}

/// Shuffle an ordering question's steps. Returns the shuffled options and
/// the answer: the option numbers (1-based, as the screen shows them) in
/// the correct order, e.g. `"3142"`.
fn shuffled_steps(steps: Vec<String>, seed: &str) -> (Vec<String>, String) {
    let mut keyed: Vec<(String, usize, String)> = steps
        .into_iter()
        .enumerate()
        .map(|(i, step)| (crate::short_hash(&format!("{seed}\n{step}"), 16), i, step))
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    // A shuffle that happens to be the right order would give the answer
    // away as "1234"; turning it around can't be (three steps or more).
    if keyed.iter().map(|k| k.1).is_sorted() {
        keyed.reverse();
    }
    let mut sequence = vec![0usize; keyed.len()];
    for (shown, (_, original, _)) in keyed.iter().enumerate() {
        sequence[*original] = shown + 1;
    }
    (
        keyed.into_iter().map(|(_, _, step)| step).collect(),
        sequence.iter().map(|n| n.to_string()).collect(),
    )
}

/// The hint excerpt, read from the real file so it can't be hallucinated.
fn excerpt_from_disk(root: &Path, rel: &str, lines: Option<(u32, u32)>) -> Option<String> {
    let (first, last) = lines?;
    let body = std::fs::read_to_string(root.join(rel)).ok()?;
    let start = (first as usize).saturating_sub(1);
    let end = (last as usize).min(start + EXCERPT_LINES);
    let chunk: Vec<&str> = body
        .lines()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();
    if chunk.is_empty() {
        return None;
    }
    Some(truncate_chars(&chunk.join("\n"), EXCERPT_CHARS))
}

/// Every question object that parses, read one at a time from the first
/// array in the reply. Reading element by element (rather than the array as
/// a whole) tolerates prose and fences around the JSON, an object that isn't
/// a question, and — the case seen live — a reply cut off mid-array, which
/// still yields the questions that arrived whole.
fn salvage(reply: &str) -> Vec<Raw> {
    let Some(start) = reply.find('[') else {
        return Vec::new();
    };
    let mut rest = &reply[start + 1..];
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if !rest.starts_with('{') {
            return out;
        }
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
        match stream.next() {
            Some(Ok(value)) => {
                let consumed = stream.byte_offset();
                if let Ok(raw) = serde_json::from_value::<Raw>(value) {
                    out.push(raw);
                }
                rest = &rest[consumed..];
            }
            // A truncated or malformed element ends the usable part.
            _ => return out,
        }
    }
}

/// The outermost JSON object in a reply.
pub(crate) fn extract_object(reply: &str) -> Option<&str> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    (end > start).then(|| &reply[start..=end])
}

pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(root: &Path) -> Material {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "line one\nline two\nline three\n").unwrap();
        Material {
            territory_id: "src".into(),
            territory_name: "src".into(),
            framing: "test".into(),
            files: vec!["src/lib.rs".into()],
            text: "### src/lib.rs\n…".into(),
            numbered: true,
        }
    }

    #[test]
    fn the_prompt_asks_for_a_mix_and_lists_questions_to_avoid() {
        let dir = tempfile::tempdir().unwrap();
        let m = material(dir.path());
        let p = prompt(&m, 5, &["What is line one?".into()]);
        assert!(p.contains("2 multiple_choice"));
        assert!(p.contains("2 free_text"));
        assert!(p.contains("1 order"));
        assert_eq!(mix(1), (1, 0, 0));
        assert_eq!(mix(3), (2, 1, 0));
        assert_eq!(mix(4), (2, 1, 1));
        assert!(p.contains("- What is line one?"));
        assert!(p.ends_with(m.text.as_str()));
    }

    #[test]
    fn parsing_keeps_valid_questions_and_reads_hints_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let m = material(dir.path());
        let reply = r#"Here you go:
```json
[
 {"kind":"multiple_choice","prompt":"Which line is second?","options":["one","two","three","four"],"answer":1,"explanation":"It says so.","source_path":"src/lib.rs","source_lines":[2,2],"difficulty":1},
 {"kind":"true_false","prompt":"There are three lines.","options":[],"answer":"True","explanation":"","source_path":"./src/lib.rs","source_lines":[1,3]},
 {"kind":"free_text","prompt":"What does line three say?","answer":"line three","explanation":"","source_path":"nope.rs","source_lines":[3,3],"excerpt":"line three"},
 {"kind":"multiple_choice","prompt":"Bad one","options":["a"],"answer":0},
 {"kind":"riddle","prompt":"Not a kind","answer":0}
]
```"#;
        let qs = parse(reply, &m, &[], "m", 7, dir.path()).unwrap();
        assert_eq!(qs.len(), 3);
        assert_eq!(qs[0].question.source_excerpt, "line two");
        // Options are shuffled; the stored index still points at the right one.
        let mut sorted = qs[0].question.options.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["four", "one", "three", "two"]);
        assert_eq!(qs[0].correct_answer(), "two");
        assert_eq!(qs[1].question.options, vec!["True", "False"]);
        assert_eq!(qs[1].answer, "0");
        assert_eq!(qs[1].question.source_path, "src/lib.rs");
        // An unknown path is pinned to the excerpted file; the hint falls
        // back to what the model quoted only when the lines don't resolve.
        assert_eq!(qs[2].question.source_path, "src/lib.rs");
        assert_eq!(qs[2].question.source_excerpt, "line three");
        assert_eq!(qs[2].question.kind, "free_text");
        assert!(qs.iter().all(|q| q.question.id.len() == 12));
    }

    #[test]
    fn diff_material_quotes_its_hint_instead_of_reading_lines_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let m = Material {
            numbered: false,
            ..material(dir.path())
        };
        assert!(prompt(&m, 2, &[]).contains("quoted verbatim"));
        assert!(!prompt(&m, 2, &[]).contains("source_lines"));
        let reply = r#"[{"kind":"true_false","prompt":"The diff adds a line.","answer":0,"source_path":"src/lib.rs","source_lines":[2,2],"excerpt":"+added line"}]"#;
        let qs = parse(reply, &m, &[], "m", 0, dir.path()).unwrap();
        assert_eq!(qs[0].question.source_excerpt, "+added line");
        assert_eq!(qs[0].question.source_lines, None);
    }

    #[test]
    fn a_question_is_credited_to_the_region_its_file_is_in() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("other")).unwrap();
        std::fs::write(dir.path().join("other/x.rs"), "fn x() {}\n").unwrap();
        let m = Material {
            files: vec!["src/lib.rs".into(), "other/x.rs".into()],
            ..material(dir.path())
        };
        let territories = vec![
            Territory {
                id: "src".into(),
                name: "src".into(),
                files: vec![],
            },
            Territory {
                id: "other".into(),
                name: "other".into(),
                files: vec![],
            },
        ];
        let reply = r#"[{"kind":"true_false","prompt":"x takes no arguments.","answer":0,"source_path":"other/x.rs","source_lines":[1,1]},
 {"kind":"true_false","prompt":"There are three lines.","answer":0,"source_path":"src/lib.rs","source_lines":[1,3]}]"#;
        let qs = parse(reply, &m, &territories, "m", 0, dir.path()).unwrap();
        assert_eq!(qs[0].question.territory, "other");
        assert_eq!(qs[1].question.territory, "src");
    }

    #[test]
    fn ordering_questions_shuffle_their_steps_and_keep_the_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let m = material(dir.path());
        let reply = r#"[{"kind":"order","prompt":"Order the steps of a turn.","options":["make room","stream the reply","run tool calls","repeat"],"source_path":"src/lib.rs","source_lines":[1,3]}]"#;
        let q = &parse(reply, &m, &[], "m", 0, dir.path()).unwrap()[0];
        assert_eq!(q.question.kind, "order");
        assert_ne!(
            q.question.options[0..2],
            ["make room".to_string(), "stream the reply".to_string()]
        );
        // Reading the options in the answer's order gives the steps back.
        let restored: Vec<&str> = q
            .answer
            .chars()
            .map(|c| q.question.options[c.to_digit(10).unwrap() as usize - 1].as_str())
            .collect();
        assert_eq!(
            restored,
            vec!["make room", "stream the reply", "run tool calls", "repeat"]
        );
        assert_ne!(q.answer, "1234");
        // Two steps aren't an ordering.
        let short = r#"[{"kind":"order","prompt":"Order these two.","options":["a","b"],"source_path":"src/lib.rs"}]"#;
        assert!(parse(short, &m, &[], "m", 0, dir.path()).is_err());
    }

    #[test]
    fn the_checker_names_what_to_drop_and_an_unreadable_verdict_drops_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let m = material(dir.path());
        let reply = r#"[{"kind":"true_false","prompt":"There are three lines.","answer":0,"source_path":"src/lib.rs","source_lines":[1,3]}]"#;
        let qs = parse(reply, &m, &[], "m", 0, dir.path()).unwrap();
        let p = verify_prompt(&qs, &m);
        assert!(p.contains("[0] (true_false) There are three lines."));
        assert!(p.contains("marked answer: True"));
        assert!(p.ends_with(m.text.as_str()));
        assert_eq!(
            rejected(
                r#"ok: [{"index":0,"ok":true,"reason":""},{"index":1,"ok":false,"reason":"two right"}]"#
            ),
            Some(vec![1])
        );
        assert_eq!(rejected("looks fine to me"), None);
    }

    #[test]
    fn shuffling_is_stable_and_spreads_the_correct_answer() {
        let options = || vec!["a".to_string(), "b".into(), "c".into(), "d".into()];
        let (first, index) = shuffled(options(), 0, "prompt one");
        assert_eq!(first[index], "a");
        assert_eq!(shuffled(options(), 0, "prompt one"), (first, index));
        // Across prompts the right answer doesn't sit in one slot.
        let slots: std::collections::BTreeSet<usize> = (0..24)
            .map(|i| shuffled(options(), 0, &format!("prompt {i}")).1)
            .collect();
        assert!(slots.len() >= 3, "{slots:?}");
    }

    #[test]
    fn a_reply_cut_off_mid_array_keeps_the_whole_questions() {
        let dir = tempfile::tempdir().unwrap();
        let m = material(dir.path());
        let reply = r#"[{"kind":"true_false","prompt":"There are three lines.","answer":0,"source_path":"src/lib.rs","source_lines":[1,3]},
 {"kind":"multiple_choice","prompt":"Which line is sec"#;
        let qs = parse(reply, &m, &[], "m", 0, dir.path()).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].question.kind, "true_false");
    }

    #[test]
    fn an_unusable_reply_is_a_model_error() {
        let dir = tempfile::tempdir().unwrap();
        let m = material(dir.path());
        let err = parse("I can't do that.", &m, &[], "m", 0, dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("it began: I can't do that."),
            "{err}"
        );
        let empty = parse("  ", &m, &[], "m", 0, dir.path()).unwrap_err();
        assert!(empty.to_string().contains("reply was empty"), "{empty}");
        assert!(matches!(
            parse("[]", &m, &[], "m", 0, dir.path()),
            Err(StudyError::Model { .. })
        ));
    }
}
