use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::Deserialize;
use tokio::sync::oneshot;

use crate::agent::tools::ToolError;

pub struct PendingQuestion {
    pub question: String,
    pub answer_tx: oneshot::Sender<String>,
}

pub static PENDING_QUESTION: std::sync::Mutex<Option<PendingQuestion>> =
    std::sync::Mutex::new(None);

#[derive(Deserialize)]
pub struct AskArgs {
    pub question: String,
}

pub struct AskUserQuestion;

impl Tool for AskUserQuestion {
    const NAME: &'static str = "ask_user_question";

    type Error = ToolError;
    type Args = AskArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "ask_user_question".to_string(),
            description: "Ask the user a question and get their typed response. Use when you need user input, clarification, or a decision.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "The question to ask the user" }
                },
                "required": ["question"]
            }),
        }
    }

    async fn call(&self, args: AskArgs) -> Result<String, ToolError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pq = PENDING_QUESTION.lock().unwrap();
            *pq = Some(PendingQuestion {
                question: args.question,
                answer_tx: tx,
            });
        }
        match rx.await {
            Ok(answer) => Ok(answer),
            Err(_) => Err(ToolError::Msg("Question cancelled by user".to_string())),
        }
    }
}
