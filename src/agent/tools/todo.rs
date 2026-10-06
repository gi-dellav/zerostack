use compact_str::CompactString;
use rig::tool::PortableTool as Tool;
use serde::{Deserialize, Serialize};

use crate::agent::tools::{AskSender, PermCheck, ToolError, check_perm};

#[derive(Serialize, Deserialize, Clone)]
pub struct TodoItem {
    pub content: String,
    pub status: CompactString,
    pub priority: CompactString,
}

#[derive(Deserialize)]
pub struct TodoWriteArgs {
    pub todos: Vec<TodoItem>,
}

pub static TODO_LIST: std::sync::Mutex<Vec<TodoItem>> = std::sync::Mutex::new(Vec::new());

pub struct WriteTodoList {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
}

impl WriteTodoList {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        WriteTodoList { permission, ask_tx }
    }
}

impl Tool for WriteTodoList {
    const NAME: &'static str = "todo_write";

    type Error = ToolError;
    type Args = TodoWriteArgs;
    type Output = String;

    fn description(&self) -> String {
        "Create or update a structured task list to track progress in the current coding session. Use this for complex multi-step tasks. Replaces any existing todo list.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string", "description": "Task description" },
                            "status": { "type": "string", "description": "pending, in_progress, completed, or cancelled" },
                            "priority": { "type": "string", "description": "high, medium, or low" }
                        },
                        "required": ["content", "status", "priority"]
                    },
                    "description": "Full list of tasks to track"
                }
            },
            "required": ["todos"]
        })
    }

    async fn call(&self, args: TodoWriteArgs) -> Result<String, ToolError> {
        tracing::debug!("tool todo_write start: items={}", args.todos.len());
        let coaching = check_perm(&self.permission, &self.ask_tx, "todo_write", "").await?;

        let mut list = TODO_LIST.lock().unwrap_or_else(|e| e.into_inner());
        *list = args.todos;

        if list.is_empty() {
            let msg = "Todo list cleared.".to_string();
            return Ok(match coaching {
                Some(c) => format!("{}\n\n{}", c, msg),
                None => msg,
            });
        }

        let total = list.len();
        let completed = list.iter().filter(|t| t.status == "completed").count();
        let in_progress = list.iter().filter(|t| t.status == "in_progress").count();
        let pending = list.iter().filter(|t| t.status == "pending").count();

        let mut result = format!("Todo list ({} items, {} done):\n", total, completed);
        for item in list.iter() {
            let icon = match item.status.as_str() {
                "completed" => "[x]",
                "in_progress" => "[>]",
                "cancelled" => "[-]",
                _ => "[ ]",
            };
            result.push_str(&format!(
                "  {} [{}] {}\n",
                icon, item.priority, item.content
            ));
        }
        result.push_str(&format!(
            "\nSummary: {} pending, {} in progress, {} completed, {} cancelled",
            pending,
            in_progress,
            completed,
            list.iter().filter(|t| t.status == "cancelled").count()
        ));
        tracing::debug!(
            "tool todo_write done: total={}, pending={}, in_progress={}, completed={}",
            total,
            pending,
            in_progress,
            completed,
        );
        if let Some(msg) = coaching {
            result = format!("{}\n\n{}", msg, result);
        }
        Ok(result)
    }
}
