use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

use crate::agent::tools::ToolError;

#[derive(Serialize, Deserialize, Clone)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    pub priority: String,
}

#[derive(Deserialize)]
pub struct TodoWriteArgs {
    pub todos: Vec<TodoItem>,
}

pub static TODO_LIST: std::sync::Mutex<Vec<TodoItem>> = std::sync::Mutex::new(Vec::new());

pub struct WriteTodoList;

impl Tool for WriteTodoList {
    const NAME: &'static str = "write_todo_list";

    type Error = ToolError;
    type Args = TodoWriteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "write_todo_list".to_string(),
            description: "Create or update a structured task list to track progress in the current coding session. Use this for complex multi-step tasks. Replaces any existing todo list.".to_string(),
            parameters: serde_json::json!({
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
            }),
        }
    }

    async fn call(&self, args: TodoWriteArgs) -> Result<String, ToolError> {
        let mut list = TODO_LIST.lock().unwrap();
        *list = args.todos;

        if list.is_empty() {
            return Ok("Todo list cleared.".to_string());
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
            result.push_str(&format!("  {} [{}] {}\n", icon, item.priority, item.content));
        }
        result.push_str(&format!("\nSummary: {} pending, {} in progress, {} completed, {} cancelled",
            pending, in_progress, completed, list.iter().filter(|t| t.status == "cancelled").count()));
        Ok(result)
    }
}
