use std::path::Path;

use rig::tool::Tool;

use crate::agent::tools::{AskSender, PermCheck, ToolError, WriteArgs, check_perm_path};
#[cfg(feature = "lsp")]
use crate::extras::lsp::LspManager;

const DEFAULT_MAX_TEXT_SIZE: u64 = 1024 * 1024;

pub struct WriteTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
    pub max_text_file_size: u64,
    /// When `Some`, written files are synced to their language server and
    /// fresh diagnostics are appended to the tool result.
    #[cfg(feature = "lsp")]
    pub lsp: Option<LspManager>,
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
            #[cfg(feature = "lsp")]
            lsp: None,
        }
    }

    #[cfg(feature = "lsp")]
    pub fn with_lsp(mut self, lsp: Option<LspManager>) -> Self {
        self.lsp = lsp;
        self
    }
}

impl Tool for WriteTool {
    const NAME: &'static str = "write";

    type Error = ToolError;
    type Args = WriteArgs;
    type Output = String;

    fn description(&self) -> String {
        "Create a new file with the given content. Fails if the file already exists — use edit for existing files. Automatically creates parent directories.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
                "content": { "type": "string", "description": "Content to write to the file" }
            },
            "required": ["path", "content"]
        })
    }

    async fn call(&self, args: WriteArgs) -> Result<String, ToolError> {
        let expanded = crate::fs::expand_tilde(&args.path);
        tracing::debug!(
            "tool write start: path={}, content_len={}",
            expanded,
            args.content.len(),
        );
        let coaching = check_perm_path(&self.permission, &self.ask_tx, "write", &expanded).await?;

        let path = Path::new(&expanded);
        if path.exists() {
            tracing::warn!("tool write file exists: path={}", expanded);
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
            tracing::warn!(
                "tool write file too large: path={}, size={}, max={}",
                expanded,
                bytes,
                self.max_text_file_size,
            );
            return Err(ToolError::Msg(format!(
                "File too large ({} bytes). Maximum allowed file size is {} bytes.",
                bytes, self.max_text_file_size
            )));
        }
        crate::fs::atomic_write(path, &args.content).await?;
        crate::agent::tools::untrack_read_path(&expanded);
        tracing::debug!("tool write done: path={}, bytes={}", expanded, bytes);
        let mut result = format!("Written {} bytes to {}", bytes, expanded);
        if let Some(msg) = coaching {
            result = format!("{}\n\n{}", msg, result);
        }

        #[cfg(feature = "lsp")]
        if let Some(lsp) = &self.lsp {
            lsp.notify_changed(path).await;
            if let Some(block) = lsp.diagnostics_block_for_edit(path).await {
                result.push_str(&block);
            }
        }

        Ok(result)
    }
}
