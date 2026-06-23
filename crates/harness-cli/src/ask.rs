//! Terminal asker for the agent's `ask_user_question` tool.
//!
//! Bridges [`harness_tools::QuestionAsker`] to the interactive [`crate::picker`]:
//! each question is presented as a selectable list (single- or multi-select)
//! with a free-text escape hatch. The blocking picker runs on a `spawn_blocking`
//! thread so it never stalls the async agent loop, and a non-TTY session returns
//! `None` (the tool then tells the model to proceed with sensible defaults).

use async_trait::async_trait;
use harness_tools::{Question, QuestionAnswer, QuestionAsker, ToolError};

use crate::picker::{self, Choice};
use crate::theme::Ui;

/// Asks questions through the terminal picker.
pub struct CliAsker {
    ui: Ui,
}

impl CliAsker {
    pub fn new(ui: Ui) -> Self {
        Self { ui }
    }
}

#[async_trait]
impl QuestionAsker for CliAsker {
    async fn ask(&self, questions: &[Question]) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
        let ui = self.ui.clone();
        let questions = questions.to_vec();
        tokio::task::spawn_blocking(move || run_interview(&ui, &questions))
            .await
            .map_err(|e| ToolError::Execution(format!("ask prompt panicked: {e}")))?
    }
}

/// Present each question in turn; `None` if the user cancels any of them or the
/// session is non-interactive.
fn run_interview(
    ui: &Ui,
    questions: &[Question],
) -> Result<Option<Vec<QuestionAnswer>>, ToolError> {
    let mut answers = Vec::with_capacity(questions.len());
    for q in questions {
        let options: Vec<Choice> = q
            .options
            .iter()
            .map(|o| Choice::new(o.label.clone(), o.description.clone()))
            .collect();
        let selected = picker::select(ui, &q.header, &q.question, &options, q.multi_select)
            .map_err(|e| ToolError::Execution(format!("terminal error: {e}")))?;
        match selected {
            Some(selected) => answers.push(QuestionAnswer {
                header: q.header.clone(),
                question: q.question.clone(),
                selected,
            }),
            None => return Ok(None),
        }
    }
    Ok(Some(answers))
}
