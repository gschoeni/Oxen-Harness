//! `ask_user_question` — let the model interview the user with structured,
//! multiple-choice questions when a product/design decision is ambiguous.
//!
//! This mirrors Claude Code's `AskUserQuestion` tool: the model sends 1–4
//! questions, each with a short `header`, the full `question`, 2–4 `options`
//! (`label` + `description`), and a `multiSelect` flag. The *host* (CLI or
//! desktop app) renders the picker and always offers an implicit free-text
//! "type your own" escape hatch, so the model never has to encode one.
//!
//! The rendering is host-specific, so this module defines only the data types,
//! the [`QuestionAsker`] trait a front end implements, and the [`AskUserTool`]
//! that bridges a model tool call to that asker.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{Tool, ToolError};

/// The tool name the model calls (and front ends special-case for rendering).
pub const ASK_USER_TOOL: &str = "ask_user_question";

/// One selectable choice within a question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// Concise display text the user selects (1–5 words).
    pub label: String,
    /// What this option means / its trade-offs.
    #[serde(default)]
    pub description: String,
}

/// A single multiple-choice question posed to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// The full question text (should end with a question mark).
    pub question: String,
    /// A very short label/chip for the question (≈ ≤12 chars).
    #[serde(default)]
    pub header: String,
    /// The 2–4 mutually exclusive choices (a free-text option is added by the host).
    pub options: Vec<Choice>,
    /// Whether the user may pick more than one option.
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

/// The user's answer to one [`Question`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuestionAnswer {
    /// The question's `header`, echoed back for context.
    pub header: String,
    /// The question text, echoed back for context.
    pub question: String,
    /// The selected option label(s), or the user's free-text answer.
    pub selected: Vec<String>,
}

/// A front end that can present questions to the user and collect answers.
///
/// `ask` returns `Ok(None)` when there is no interactive user available (e.g. a
/// piped/non-TTY session), so the tool can tell the model to proceed with
/// sensible defaults rather than hang.
#[async_trait]
pub trait QuestionAsker: Send + Sync {
    async fn ask(&self, questions: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError>;
}

/// The model-facing tool that asks the user clarifying questions.
pub struct AskUserTool {
    asker: Arc<dyn QuestionAsker>,
}

impl AskUserTool {
    pub fn new(asker: Arc<dyn QuestionAsker>) -> Self {
        Self { asker }
    }
}

/// Parse + validate the `questions` argument, returning a model-friendly error
/// string on malformed input.
fn parse_questions(args: &serde_json::Value) -> Result<Vec<Question>, String> {
    let raw = args
        .get("questions")
        .ok_or("missing `questions` array")?
        .clone();
    let questions: Vec<Question> =
        serde_json::from_value(raw).map_err(|e| format!("invalid `questions`: {e}"))?;
    if questions.is_empty() || questions.len() > 4 {
        return Err("provide between 1 and 4 questions".to_string());
    }
    for q in &questions {
        if q.question.trim().is_empty() {
            return Err("each question needs non-empty `question` text".to_string());
        }
        if q.options.len() < 2 || q.options.len() > 4 {
            return Err(format!(
                "question {:?} must have 2–4 options (got {})",
                q.header,
                q.options.len()
            ));
        }
    }
    Ok(questions)
}

/// Format collected answers as a compact, unambiguous block for the model.
fn format_answers(answers: &[QuestionAnswer]) -> String {
    let mut out = String::from("The user answered:\n");
    for a in answers {
        let label = if a.header.trim().is_empty() {
            a.question.clone()
        } else {
            format!("{} — {}", a.header, a.question)
        };
        out.push_str(&format!("- {label}\n  → {}\n", a.selected.join(", ")));
    }
    out.push_str(
        "\nNow act on these decisions by making the appropriate tool call in this same turn \
         (e.g. open a `canvas`, write or edit files, run a command). Do not reply with only a \
         description of what you are about to do.",
    );
    out
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        ASK_USER_TOOL
    }

    fn description(&self) -> &str {
        "Ask the user one or more multiple-choice questions to resolve an \
         ambiguous product, design, or implementation decision before acting. \
         Use this instead of guessing when there are several reasonable \
         approaches with real trade-offs. The host renders an interactive \
         picker and always lets the user type their own answer, so do not add \
         an 'Other' option. Keep options distinct and concise; prefer asking \
         early rather than building the wrong thing."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "description": "1–4 questions to ask the user.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The full question text; end with a question mark."
                            },
                            "header": {
                                "type": "string",
                                "description": "Very short label/chip for the question (max ~12 chars), e.g. 'Storage', 'Auth'."
                            },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "description": "2–4 distinct, mutually exclusive choices. Do not add an 'Other' option.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Concise choice text (1–5 words)."
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "What this option means or implies."
                                        }
                                    },
                                    "required": ["label"]
                                }
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "default": false,
                                "description": "Allow selecting multiple options."
                            }
                        },
                        "required": ["question", "header", "options"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<String, ToolError> {
        let questions = parse_questions(&args).map_err(ToolError::InvalidArguments)?;
        match self.asker.ask(&questions).await? {
            Some(answers) => Ok(format_answers(&answers)),
            None => Ok("No interactive user is available to answer right now. \
                 Proceed with the most reasonable default and clearly state the \
                 assumptions you made."
                .to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted asker that echoes the first option of each question.
    struct FirstOptionAsker;

    #[async_trait]
    impl QuestionAsker for FirstOptionAsker {
        async fn ask(
            &self,
            questions: &[Question],
        ) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
            Ok(Some(
                questions
                    .iter()
                    .map(|q| QuestionAnswer {
                        header: q.header.clone(),
                        question: q.question.clone(),
                        selected: vec![q.options[0].label.clone()],
                    })
                    .collect(),
            ))
        }
    }

    struct NonInteractiveAsker;

    #[async_trait]
    impl QuestionAsker for NonInteractiveAsker {
        async fn ask(&self, _: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
            Ok(None)
        }
    }

    fn sample_args() -> serde_json::Value {
        serde_json::json!({
            "questions": [{
                "question": "Which storage backend should we use?",
                "header": "Storage",
                "options": [
                    {"label": "SQLite", "description": "Embedded, zero-config"},
                    {"label": "Postgres", "description": "Server, scales further"}
                ],
                "multiSelect": false
            }]
        })
    }

    #[test]
    fn parses_claude_code_shaped_questions() {
        let qs = parse_questions(&sample_args()).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].header, "Storage");
        assert_eq!(qs[0].options.len(), 2);
        assert!(!qs[0].multi_select);
    }

    #[test]
    fn rejects_bad_option_counts_and_empty() {
        let one_option = serde_json::json!({
            "questions": [{"question": "q?", "header": "h",
                "options": [{"label": "only"}]}]
        });
        assert!(parse_questions(&one_option).is_err());

        let no_questions = serde_json::json!({ "questions": [] });
        assert!(parse_questions(&no_questions).is_err());
    }

    #[tokio::test]
    async fn invoke_formats_selected_answers() {
        let tool = AskUserTool::new(Arc::new(FirstOptionAsker));
        let out = tool.invoke(sample_args()).await.unwrap();
        assert!(out.contains("Storage — Which storage backend"));
        assert!(out.contains("→ SQLite"));
    }

    #[tokio::test]
    async fn invoke_handles_non_interactive_sessions() {
        let tool = AskUserTool::new(Arc::new(NonInteractiveAsker));
        let out = tool.invoke(sample_args()).await.unwrap();
        assert!(out.contains("No interactive user"));
    }

    #[test]
    fn schema_advertises_questions_array() {
        let tool = AskUserTool::new(Arc::new(NonInteractiveAsker));
        let schema = tool.parameters_schema();
        assert_eq!(schema["properties"]["questions"]["maxItems"], 4);
        assert_eq!(tool.name(), ASK_USER_TOOL);
    }
}
