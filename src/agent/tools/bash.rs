use rig::completion::ToolDefinition;
use rig::tool::Tool;
use tokio::time::{Duration, timeout};

use crate::agent::tools::{AskSender, BashArgs, PermCheck, ToolError, check_perm};
use crate::extras::truncate::head_lines;
use crate::sandbox::Sandbox;

pub(crate) fn split_bash_commands(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            current.push(ch);
            if let Some(next) = chars.next() {
                current.push(next);
            }
        } else if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(ch);
        } else if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(ch);
        } else if ch == ';' && !in_single_quote && !in_double_quote {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                result.push(trimmed);
            }
            current = String::new();
        } else if ch == '&' && !in_single_quote && !in_double_quote {
            if chars.peek() == Some(&'&') {
                chars.next();
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    result.push(trimmed);
                }
                current = String::new();
            } else {
                current.push(ch);
            }
        } else if ch == '|' && !in_single_quote && !in_double_quote {
            if chars.peek() == Some(&'|') {
                chars.next();
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    result.push(trimmed);
                }
                current = String::new();
            } else {
                current.push(ch);
            }
        } else if ch == '>' && !in_single_quote && !in_double_quote {
            if chars.peek() == Some(&'>') {
                chars.next();
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    result.push(trimmed);
                }
                current = String::new();
            } else {
                current.push(ch);
            }
        } else {
            current.push(ch);
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }

    result
}

pub struct BashTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
    pub sandbox: Sandbox,
    /// `None` = no truncation (matches the historical behaviour). `Some(n)`
    /// = head-only truncation after `n` lines with a recovery hint.
    pub max_output_lines: Option<u64>,
}

impl BashTool {
    pub fn new(
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
        sandbox: Sandbox,
        max_output_lines: Option<u64>,
    ) -> Self {
        BashTool {
            permission,
            ask_tx,
            sandbox,
            max_output_lines,
        }
    }
}

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
                    "timeout": { "type": "integer", "description": "Timeout in milliseconds (optional)" }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: BashArgs) -> Result<String, ToolError> {
        let mut coaching: Option<String> = None;
        for cmd in split_bash_commands(&args.command) {
            if let Some(msg) = check_perm(&self.permission, &self.ask_tx, "bash", &cmd).await? {
                coaching = Some(msg);
            }
        }

        let output = if let Some(secs) = args.timeout {
            timeout(
                Duration::from_millis(secs),
                self.sandbox.wrap_command(&args.command).output(),
            )
            .await
            .map_err(|_| ToolError::Msg("Command timed out".to_string()))?
        } else {
            self.sandbox.wrap_command(&args.command).output().await
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

        let result = if let Some(cap) = self.max_output_lines {
            let cap = cap as usize;
            let (head, total) = head_lines(&result, cap);
            if total > cap {
                format!(
                    "{}\n\n[truncated after {} lines — {} more lines elided; re-run with a narrower invocation or pipe through `tail`/`grep` to see trailing output]",
                    head,
                    cap,
                    total - cap,
                )
            } else {
                result
            }
        } else {
            result
        };

        let result = match coaching {
            Some(msg) => format!("{}\n\n{}", msg, result),
            None => result,
        };
        Ok(result)
    }
}
