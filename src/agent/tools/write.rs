use std::path::Path;

use rig::completion::ToolDefinition;
use rig::tool::Tool;

use crate::agent::tools::{AskSender, PermCheck, ToolError, WriteArgs, check_perm_path};

pub struct WriteTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
}

impl WriteTool {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        WriteTool { permission, ask_tx }
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
            description: "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.".to_string(),
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
        check_perm_path(&self.permission, &self.ask_tx, "write", &args.path).await?;

        let path = Path::new(&args.path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let bytes = args.content.len();
        tokio::fs::write(path, &args.content).await?;
        Ok(format!("Written {} bytes to {}", bytes, args.path))
    }
}
