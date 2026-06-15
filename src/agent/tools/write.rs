use std::path::Path;

use rig::completion::ToolDefinition;
use rig::tool::Tool;

use crate::agent::tools::{AskSender, PermCheck, ToolError, WriteArgs, check_perm_path};

const DEFAULT_MAX_TEXT_SIZE: u64 = 1024 * 1024;

pub struct WriteTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
    pub max_text_file_size: u64,
}

impl WriteTool {
    pub fn new(
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
        max_text_file_size: Option<u64>,
    ) -> Self {
        WriteTool {
            permission,
            ask_tx,
            max_text_file_size: max_text_file_size.unwrap_or(DEFAULT_MAX_TEXT_SIZE),
        }
    }
}

impl Tool for WriteTool {
    const NAME: &'static str = "write";

    type Error = ToolError;
    type Args = WriteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "write".to_string(),
            description: "Create a new file with the given content. Fails if the file already exists — use edit for existing files. Automatically creates parent directories.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
                    "content": { "type": "string", "description": "Content to write to the file" }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call(&self, args: WriteArgs) -> Result<String, ToolError> {
        let expanded = crate::fs::expand_tilde(&args.path);
        let coaching = check_perm_path(&self.permission, &self.ask_tx, "write", &expanded).await?;

        let path = Path::new(&expanded);
        if path.exists() {
            return Err(ToolError::Msg(format!(
                "File '{}' already exists. Use edit for targeted changes, or delete and recreate if a full rewrite is needed.",
                expanded
            )));
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let bytes = args.content.len();
        if bytes as u64 > self.max_text_file_size {
            return Err(ToolError::Msg(format!(
                "File too large ({} bytes). Maximum allowed file size is {} bytes.",
                bytes, self.max_text_file_size
            )));
        }
        crate::fs::atomic_write(path, &args.content).await?;
        crate::agent::tools::untrack_read_path(&expanded);
        let mut result = format!("Written {} bytes to {}", bytes, expanded);
        if let Some(msg) = coaching {
            result = format!("{}\n\n{}", msg, result);
        }
        Ok(result)
    }
}
