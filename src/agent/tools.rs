use std::io;
use std::path::Path;
use std::time::Duration;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::Deserialize;
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Msg(String),
}

impl From<io::Error> for ToolError {
    fn from(e: io::Error) -> Self {
        ToolError::Msg(e.to_string())
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(e: serde_json::Error) -> Self {
        ToolError::Msg(e.to_string())
    }
}

#[derive(Deserialize)]
pub struct ReadArgs {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

pub struct ReadTool;

impl Tool for ReadTool {
    const NAME: &'static str = "read";

    type Error = ToolError;
    type Args = ReadArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "read".to_string(),
            description: "Read the contents of a file. Supports text files. Defaults to first 2000 lines. Use offset/limit for large files.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
                    "offset": { "type": "integer", "description": "Line number to start from (1-indexed)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines to read" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: ReadArgs) -> Result<String, ToolError> {
        let content = tokio::fs::read_to_string(&args.path).await?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let offset = args.offset.unwrap_or(1).max(1) - 1;
        let limit = args.limit.unwrap_or(2000);
        let end = (offset + limit).min(total_lines);

        let excerpt = lines[offset..end].join("\n");
        let info = format!(
            "File: {} ({} lines total, showing lines {}-{})\n\n{}",
            args.path,
            total_lines,
            offset + 1,
            end,
            excerpt
        );
        Ok(info)
    }
}

#[derive(Deserialize)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
}

pub struct WriteTool;

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
        let path = Path::new(&args.path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let bytes = args.content.len();
        tokio::fs::write(path, &args.content).await?;
        Ok(format!("Written {} bytes to {}", bytes, args.path))
    }
}

#[derive(Deserialize)]
pub struct EditArgs {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
}

pub struct EditTool;

impl Tool for EditTool {
    const NAME: &'static str = "edit";

    type Error = ToolError;
    type Args = EditArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "edit".to_string(),
            description: "Edit a file by replacing exact text. The old_text must match exactly (including whitespace). Use for precise surgical edits.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
                    "old_text": { "type": "string", "description": "Exact text to find and replace" },
                    "new_text": { "type": "string", "description": "New text to replace the old text with" }
                },
                "required": ["path", "old_text", "new_text"]
            }),
        }
    }

    async fn call(&self, args: EditArgs) -> Result<String, ToolError> {
        let content = tokio::fs::read_to_string(&args.path).await?;
        if !content.contains(&args.old_text) {
            return Err(ToolError::Msg(
                "old_text not found in file. Ensure the exact text matches including whitespace."
                    .to_string(),
            ));
        }
        let new_content = content.replace(&args.old_text, &args.new_text);
        tokio::fs::write(&args.path, &new_content).await?;
        Ok(format!("Applied edit to {}", args.path))
    }
}

#[derive(Deserialize)]
pub struct BashArgs {
    pub command: String,
    pub timeout: Option<u64>,
}

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
            timeout(Duration::from_secs(secs), Command::new("bash")
                .arg("-c").arg(&args.command)
                .output()).await
                .map_err(|_| ToolError::Msg("Command timed out".to_string()))?
        } else {
            Command::new("bash")
                .arg("-c").arg(&args.command)
                .output().await
        }?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        let mut result = String::new();
        if !stdout.is_empty() { result.push_str(&stdout); }
        if !stderr.is_empty() {
            if !result.is_empty() { result.push('\n'); }
            result.push_str(&stderr);
        }
        if exit_code != 0 {
            result.push_str(&format!("\nExit code: {}", exit_code));
        }
        Ok(result)
    }
}

#[derive(Deserialize)]
pub struct SearchArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub include: Option<String>,
}

pub struct SearchTool;

impl Tool for SearchTool {
    const NAME: &'static str = "search";

    type Error = ToolError;
    type Args = SearchArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "search".to_string(),
            description: "Search file contents using a regex pattern. Uses ripgrep for fast searching. Supports filtering by file glob and directory.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for (supports full regex syntax)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (defaults to current working directory)"
                    },
                    "include": {
                        "type": "string",
                        "description": "Optional file glob pattern to filter (e.g. '*.rs', '*.{ts,tsx}')"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: SearchArgs) -> Result<String, ToolError> {
        let search_path = args.path.unwrap_or_else(|| ".".to_string());
        let mut cmd = Command::new("rg");
        cmd.arg("--line-number")
           .arg("--color=never")
           .arg("--no-heading")
           .arg(&args.pattern)
           .arg(&search_path);

        if let Some(include) = &args.include {
            cmd.arg("--glob").arg(include);
        }

        let output = cmd.output().await.map_err(|e| {
            ToolError::Msg(format!("Failed to run rg (ripgrep): {}. Is ripgrep installed?", e))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() && stdout.is_empty() {
            if stderr.contains("not found") || stderr.contains("no such") {
                return Ok("No matches found.".to_string());
            }
            return Err(ToolError::Msg(format!("rg error: {}", stderr)));
        }

        if stdout.is_empty() {
            return Ok("No matches found.".to_string());
        }

        let lines: Vec<&str> = stdout.lines().collect();
        let total = lines.len();
        let max_lines = 200;
        if total > max_lines {
            Ok(format!(
                "{} results (showing first {}):\n{}\n\n... and {} more matches",
                total,
                max_lines,
                lines[..max_lines].join("\n"),
                total - max_lines
            ))
        } else {
            Ok(format!("{} results:\n{}", total, stdout.trim_end()))
        }
    }
}
