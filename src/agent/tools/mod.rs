/// Shell command execution tool with sandboxing and timeout.
pub(crate) mod bash;
/// CRC32 checksums for optimistic-concurrency guards on edit/write.
pub(crate) mod crc;
/// Block/line-based file editing tool with similarity matching.
pub(crate) mod edit;
/// Filename search tool respecting .gitignore.
pub(crate) mod find_files;
/// Regex content search tool respecting .gitignore.
pub(crate) mod grep;
/// Directory listing tool with truncation limits.
pub(crate) mod list_dir;
#[cfg(feature = "lsp")]
/// On-demand language-server diagnostics tool.
pub(crate) mod lsp;
/// Whitespace normalization and similarity helpers for edits.
pub(crate) mod normalize;
/// File reading tool with line ranges and size limits.
pub(crate) mod read;
/// Persistent todo-list tool shared across agent turns.
pub(crate) mod todo;
/// Atomic file creation/overwrite tool.
pub(crate) mod write;

pub(crate) use normalize::{levenshtein_similarity, normalize_whitespace};

use std::sync::Mutex;

use crate::config::types::EditSystem;

static EDIT_SYSTEM: Mutex<EditSystem> = Mutex::new(EditSystem::Similarity);

/// `pub` (not `pub(crate)`): the headless [`Engine`](crate::engine::Engine)
/// reads/writes the edit system via `/editsys` without a TUI.
pub fn set_edit_system(es: EditSystem) {
    *EDIT_SYSTEM.lock().unwrap_or_else(|e| e.into_inner()) = es;
}

/// `pub` (not `pub(crate)`): see [`set_edit_system`].
pub fn edit_system() -> EditSystem {
    *EDIT_SYSTEM.lock().unwrap_or_else(|e| e.into_inner())
}

static DENY_REPEATED_READS: Mutex<bool> = Mutex::new(true);

pub(crate) fn set_deny_repeated_reads(v: bool) {
    *DENY_REPEATED_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = v;
}

pub(crate) fn deny_repeated_reads() -> bool {
    *DENY_REPEATED_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

static READ_TRACKER: Mutex<Vec<(String, usize, usize)>> = Mutex::new(Vec::new());

pub(crate) fn track_read(path: &str, offset: usize, limit: usize) -> Option<String> {
    if !deny_repeated_reads() {
        return None;
    }
    let mut tracker = READ_TRACKER.lock().unwrap_or_else(|e| e.into_inner());
    let key = (path.to_string(), offset, limit);
    if tracker.contains(&key) {
        let end = (offset + limit).saturating_sub(1);
        Some(format!(
            "read blocked: {path} (lines {}-{}) was already read and has not been modified since. Use the previous result or read a different section.",
            offset + 1,
            if end > 0 { end } else { offset + 1 }
        ))
    } else {
        tracker.push(key);
        None
    }
}

pub(crate) fn untrack_read_path(path: &str) {
    let mut tracker = READ_TRACKER.lock().unwrap_or_else(|e| e.into_inner());
    tracker.retain(|(p, _, _)| p != path);
}

pub use bash::BashTool;
pub use edit::EditTool;
pub use find_files::FindFilesTool;
pub use grep::GrepTool;
pub use list_dir::ListDirTool;
pub use read::ReadTool;
pub use todo::WriteTodoList;
pub use write::WriteTool;

use std::io;

use compact_str::CompactString;
use serde::Deserialize;

use crate::permission::ask::{AskRequest, AskSender, UserDecision};
use crate::permission::checker::{CheckResult, PermCheck};

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
    #[serde(default)]
    pub block: Option<String>,
    #[serde(default)]
    pub file_crc: Option<String>,
    #[serde(default)]
    pub edits: Option<Vec<EditOp>>,
}

#[derive(Debug, Clone)]
pub(crate) struct EditBlock {
    pub search: String,
    pub replace: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct EditOp {
    pub line: Option<String>,
    pub lines: Option<String>,
    pub text: String,
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
            tool: CompactString::new(tool),
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
                .unwrap_or_else(|e| e.into_inner())
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
) -> Result<Option<String>, ToolError> {
    let Some(perm) = permission else {
        return Ok(None);
    };
    let result = {
        let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
        guard.check(tool, input_key)
    };
    match result {
        CheckResult::Allowed => Ok(None),
        CheckResult::AllowedWithCoaching(msg) => Ok(Some(msg)),
        CheckResult::Denied(reason) => {
            Err(ToolError::Msg(format!("Permission denied: {}", reason)))
        }
        CheckResult::Ask => {
            let Some(tx) = ask_tx else {
                return Err(ToolError::Msg(
                    "Permission denied (non-interactive mode)".to_string(),
                ));
            };
            handle_ask_inner(tx, perm, tool, input_key).await?;
            Ok(None)
        }
    }
}

pub async fn check_perm_path(
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    tool: &str,
    path: &str,
) -> Result<Option<String>, ToolError> {
    let Some(perm) = permission else {
        return Ok(None);
    };
    let result = {
        let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
        guard.check_path(tool, path)
    };
    match result {
        CheckResult::Allowed => Ok(None),
        CheckResult::AllowedWithCoaching(msg) => Ok(Some(msg)),
        CheckResult::Denied(reason) => {
            Err(ToolError::Msg(format!("Permission denied: {}", reason)))
        }
        CheckResult::Ask => {
            let Some(tx) = ask_tx else {
                return Err(ToolError::Msg(
                    "Permission denied (non-interactive mode)".to_string(),
                ));
            };
            handle_ask_inner(tx, perm, tool, path).await?;
            Ok(None)
        }
    }
}
