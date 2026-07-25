//! `lsp_diagnostics` agent tool: on-demand language-server diagnostics.
//! Post-edit diagnostics are appended to edit/write results automatically;
//! this tool is for querying a file before editing it, or surveying the
//! whole project.

use std::path::Path;
use std::time::Duration;

use rig::tool::Tool;
use serde::Deserialize;

use crate::agent::tools::ToolError;
use crate::extras::lsp::LspManager;

/// Longer than the post-edit wait: an explicit query justifies giving the
/// server more time to catch up.
const QUERY_WAIT: Duration = Duration::from_secs(3);

pub struct LspTool {
    pub manager: LspManager,
}

#[derive(Deserialize)]
pub struct LspArgs {
    /// File to inspect. Omit to list diagnostics for every file.
    pub path: Option<String>,
}

impl LspTool {
    pub fn new(manager: LspManager) -> Self {
        Self { manager }
    }
}

impl Tool for LspTool {
    const NAME: &'static str = "lsp_diagnostics";

    type Error = ToolError;
    type Args = LspArgs;
    type Output = String;

    fn description(&self) -> String {
        "Get language-server diagnostics (errors/warnings). With `path`: diagnostics for that file, synced from disk first. Without: every file that currently has diagnostics.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to inspect (optional; omit for all files)" }
            }
        })
    }

    async fn call(&self, args: LspArgs) -> Result<String, ToolError> {
        match args.path {
            Some(path) => {
                let expanded = crate::fs::expand_tilde(&path);
                let path = Path::new(&expanded);
                if !path.exists() {
                    return Err(ToolError::Msg(format!("File '{expanded}' does not exist.")));
                }
                self.manager.notify_changed(path).await;
                Ok(self
                    .manager
                    .diagnostics_block(path, QUERY_WAIT)
                    .await
                    .map(|block| block.trim_start().to_string())
                    .unwrap_or_else(|| format!("No diagnostics for {expanded}.")))
            }
            None => Ok(self
                .manager
                .all_diagnostics_block()
                .map(|block| format!("Files with diagnostics:\n{block}"))
                .unwrap_or_else(|| "No diagnostics.".to_string())),
        }
    }
}
