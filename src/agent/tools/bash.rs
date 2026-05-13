use rig::completion::ToolDefinition;
use rig::tool::Tool;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use crate::agent::tools::{BashArgs, ToolError};

pub struct BashTool;

impl Tool for BashTool {
    const NAME: &'static str = "bash";

    type Error = ToolError;
    type Args = BashArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "bash".to_string(),
            description: "Execute a bash command in the current working directory. Returns stdout and stderr.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Bash command to execute" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds (optional)" }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: BashArgs) -> Result<String, ToolError> {
        let output = if let Some(secs) = args.timeout {
            timeout(
                Duration::from_secs(secs),
                Command::new("bash").arg("-c").arg(&args.command).output(),
            )
            .await
            .map_err(|_| ToolError::Msg("Command timed out".to_string()))?
        } else {
            Command::new("bash")
                .arg("-c")
                .arg(&args.command)
                .output()
                .await
        }?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code().unwrap_or(-1);

        let mut result = String::new();
        if !stdout.is_empty() {
            result.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&stderr);
        }
        if exit_code != 0 {
            result.push_str(&format!("\nExit code: {}", exit_code));
        }
        Ok(result)
    }
}
