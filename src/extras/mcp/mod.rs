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
}

impl McpClientManager {
    pub async fn connect_all(configs: &HashMap<String, config::McpServerConfig>) -> Self {
        tracing::debug!("MCP connecting to {} servers", configs.len());
        let mut handles = Vec::new();
        let mut notices = Vec::new();
        for (name, cfg) in configs {
            match client::McpClientHandle::connect(CompactString::new(name.clone()), cfg).await {
                Ok(handle) => {
                    tracing::info!("Connected to MCP server '{}'", name);
                    handles.push(handle);
                }
                Err(e) => {
                    tracing::debug!("Failed to connect to MCP server '{}': {e}", name);
                    notices.push(CompactString::new(format!(
                        "MCP server '{name}' not connected: {e}"
                    )));
                }
            }
        }
        Self { handles, notices }
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
        self.handles.retain(|h| h.server_name != name);
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
