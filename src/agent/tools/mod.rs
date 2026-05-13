mod bash;
mod edit;
mod find_files;
mod grep;
mod list_dir;
mod read;
mod todo;
mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use find_files::FindFilesTool;
pub use grep::GrepTool;
pub use list_dir::ListDirTool;
pub use read::ReadTool;
pub use todo::WriteTodoList;
pub use write::WriteTool;

use std::io;

use serde::Deserialize;

use crate::permission::ask::{AskRequest, AskSender, UserDecision};
use crate::permission::checker::{CheckResult, PermCheck};

pub const MAX_GREP_RESULTS: usize = 200;
pub const MAX_FIND_RESULTS: usize = 200;

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

pub fn is_skip_dir(name: &str) -> bool {
    matches!(name, "node_modules" | "target")
}

#[derive(Deserialize)]
pub struct ReadArgs {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
}

#[derive(Deserialize)]
pub struct EditArgs {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
    pub replace_all: Option<bool>,
}

#[derive(Deserialize)]
pub struct BashArgs {
    pub command: String,
    pub timeout: Option<u64>,
}

#[derive(Deserialize)]
pub struct GrepArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub include: Option<String>,
    pub context_lines: Option<usize>,
}

#[derive(Deserialize)]
pub struct FindFilesArgs {
    pub pattern: String,
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct ListDirArgs {
    pub path: Option<String>,
}

async fn handle_ask_inner(
    ask_tx: &AskSender,
    permission: &PermCheck,
    tool: &str,
    input: &str,
) -> Result<(), ToolError> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    ask_tx
        .send(AskRequest {
            tool: tool.to_string(),
            input: input.to_string(),
            reply: reply_tx,
        })
        .await
        .map_err(|_| ToolError::Msg("Permission system unavailable".to_string()))?;
    match reply_rx.await {
        Ok(UserDecision::AllowOnce) => Ok(()),
        Ok(UserDecision::AllowAlways(pattern)) => {
            permission
                .lock()
                .unwrap()
                .add_session_allowlist(tool.to_string(), &pattern);
            Ok(())
        }
        _ => Err(ToolError::Msg("Permission denied by user".to_string())),
    }
}

pub async fn check_perm(
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    tool: &str,
    input_key: &str,
) -> Result<(), ToolError> {
    let Some(perm) = permission else {
        return Ok(());
    };
    let result = {
        let mut guard = perm.lock().unwrap();
        guard.check(tool, input_key)
    };
    match result {
        CheckResult::Allowed => Ok(()),
        CheckResult::Denied(reason) => {
            Err(ToolError::Msg(format!("Permission denied: {}", reason)))
        }
        CheckResult::Ask => {
            let Some(tx) = ask_tx else {
                return Err(ToolError::Msg(
                    "Permission denied (non-interactive mode)".to_string(),
                ));
            };
            handle_ask_inner(tx, perm, tool, input_key).await
        }
    }
}

pub async fn check_perm_path(
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    tool: &str,
    path: &str,
) -> Result<(), ToolError> {
    let Some(perm) = permission else {
        return Ok(());
    };
    let result = {
        let mut guard = perm.lock().unwrap();
        guard.check_path(tool, path)
    };
    match result {
        CheckResult::Allowed => Ok(()),
        CheckResult::Denied(reason) => {
            Err(ToolError::Msg(format!("Permission denied: {}", reason)))
        }
        CheckResult::Ask => {
            let Some(tx) = ask_tx else {
                return Err(ToolError::Msg(
                    "Permission denied (non-interactive mode)".to_string(),
                ));
            };
            handle_ask_inner(tx, perm, tool, path).await
        }
    }
}
