mod read;
mod write;
mod edit;
mod bash;
mod grep;
mod find_files;
mod list_dir;
mod todo;
mod ask;

pub use read::ReadTool;
pub use write::WriteTool;
pub use edit::EditTool;
pub use bash::BashTool;
pub use grep::GrepTool;
pub use find_files::FindFilesTool;
pub use list_dir::ListDirTool;
pub use todo::WriteTodoList;
pub use ask::{AskUserQuestion, PENDING_QUESTION};

use std::io;

use serde::Deserialize;

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
