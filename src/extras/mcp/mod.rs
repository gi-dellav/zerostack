pub mod client;
pub mod config;
pub mod oauth;
pub mod tool;

use std::collections::HashMap;

use compact_str::CompactString;
use tool::McpTool;

use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;

pub struct McpClientManager {
    pub handles: Vec<client::McpClientHandle>,
    /// Connection failures collected during `connect_all`, to be surfaced by the
    /// TUI via the renderer. We do NOT log these at `warn` because that writes to
    /// stderr, which corrupts the alt-screen TUI (overlapping the input box).
    pub notices: Vec<CompactString>,
    /// Tool count per connected server, fetched right after connect so the UI
    /// can report `✓ MCP <name> — N tools` and a startup total.
    pub tool_counts: Vec<(CompactString, usize)>,
}

/// Per-server connection progress, reported by
/// [`McpClientManager::connect_all_with_progress`] so the chat UI can show
/// servers loading one by one.
pub enum ConnectProgress<'a> {
    /// The server connected and its handle is live; carries the time the
    /// connection took and how many tools the server exposes.
    Connected(&'a str, std::time::Duration, usize),
    /// The connection failed; carries the error message.
    Failed(&'a str, CompactString),
}

impl McpClientManager {
    pub async fn connect_all(configs: &HashMap<String, config::McpServerConfig>) -> Self {
        Self::connect_all_with_progress(configs, |_| {}).await
    }

    /// [`connect_all`](Self::connect_all) with per-server progress callbacks.
    /// Servers are tried in name order so progress output is deterministic.
    pub async fn connect_all_with_progress(
        configs: &HashMap<String, config::McpServerConfig>,
        mut progress: impl FnMut(ConnectProgress<'_>),
    ) -> Self {
        tracing::debug!("MCP connecting to {} servers", configs.len());
        let mut names: Vec<&String> = configs.keys().collect();
        names.sort();
        let mut handles = Vec::new();
        let mut notices = Vec::new();
        let mut tool_counts = Vec::new();
        for name in names {
            let cfg = &configs[name];
            let started = std::time::Instant::now();
            match client::McpClientHandle::connect(CompactString::new(name.clone()), cfg).await {
                Ok(handle) => {
                    tracing::info!(
                        "Connected to MCP server '{}' in {:?}",
                        name,
                        started.elapsed()
                    );
                    let tool_count = handle.list_tools().await.map(|t| t.len()).unwrap_or(0);
                    progress(ConnectProgress::Connected(
                        name,
                        started.elapsed(),
                        tool_count,
                    ));
                    tool_counts.push((CompactString::new(name.clone()), tool_count));
                    handles.push(handle);
                }
                Err(e) => {
                    tracing::debug!(
                        "Failed to connect to MCP server '{}' after {:?}: {e}",
                        name,
                        started.elapsed()
                    );
                    progress(ConnectProgress::Failed(
                        name,
                        CompactString::new(format!("{e}")),
                    ));
                    notices.push(CompactString::new(format!(
                        "MCP server '{name}' not connected: {e}"
                    )));
                }
            }
        }
        Self {
            handles,
            notices,
            tool_counts,
        }
    }

    /// Drain and return any pending connection notices.
    pub fn take_notices(&mut self) -> Vec<CompactString> {
        std::mem::take(&mut self.notices)
    }

    pub async fn collect_tools(
        &self,
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
    ) -> Vec<McpTool> {
        tracing::debug!("MCP collecting tools from {} handles", self.handles.len());
        let mut all_tools = Vec::new();
        for handle in &self.handles {
            let peer = handle.peer();
            let server_name = handle.server_name.clone();
            match handle.list_tools().await {
                Ok(tools) => {
                    tracing::debug!("MCP server '{}': {} tools listed", server_name, tools.len(),);
                    for definition in tools {
                        all_tools.push(McpTool {
                            server_name: server_name.clone(),
                            definition,
                            peer: peer.clone(),
                            permission: permission.clone(),
                            ask_tx: ask_tx.clone(),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to list tools from MCP server '{}': {e}",
                        server_name
                    );
                }
            }
        }
        all_tools
    }

    /// (Re)connect a single server, replacing any existing handle for it.
    /// Used after an interactive OAuth login so the server's tools become
    /// available without restarting the session.
    pub async fn reconnect(
        &mut self,
        name: &str,
        cfg: &config::McpServerConfig,
    ) -> anyhow::Result<()> {
        tracing::info!("MCP reconnecting server '{}'", name);
        let handle = client::McpClientHandle::connect(CompactString::new(name), cfg).await?;
        let tool_count = handle.list_tools().await.map(|t| t.len()).unwrap_or(0);
        self.handles.retain(|h| h.server_name != name);
        self.tool_counts.retain(|(n, _)| n.as_str() != name);
        self.tool_counts
            .push((CompactString::new(name), tool_count));
        self.handles.push(handle);
        Ok(())
    }

    pub async fn shutdown(self) {
        tracing::debug!("MCP shutting down {} connections", self.handles.len());
        for handle in self.handles {
            let name = handle.server_name.clone();
            // Explicitly shut down the running service so child processes and
            // HTTP connections are cleaned up properly, rather than relying on
            // Drop which may not await teardown.
            let _ = handle.running_service.cancel().await;
            tracing::debug!("Disconnected from MCP server '{}'", name);
        }
    }
}
