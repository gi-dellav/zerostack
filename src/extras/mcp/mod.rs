pub mod client;
pub mod config;
pub mod tool;

use std::collections::HashMap;

use tool::McpTool;

use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;

pub struct McpClientManager {
    pub handles: Vec<client::McpClientHandle>,
}

impl McpClientManager {
    pub async fn connect_all(
        configs: &HashMap<String, config::McpServerConfig>,
    ) -> Self {
        let mut handles = Vec::new();
        for (name, cfg) in configs {
            match client::McpClientHandle::connect(name.clone(), cfg).await {
                Ok(handle) => {
                    tracing::info!("Connected to MCP server '{}'", name);
                    handles.push(handle);
                }
                Err(e) => {
                    tracing::warn!("Failed to connect to MCP server '{}': {e}", name);
                }
            }
        }
        Self { handles }
    }

    pub async fn collect_tools(
        &self,
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
    ) -> Vec<McpTool> {
        let mut all_tools = Vec::new();
        for handle in &self.handles {
            let peer = handle.peer();
            let server_name = handle.server_name.clone();
            match handle.list_tools().await {
                Ok(tools) => {
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

    pub async fn shutdown(self) {
        for handle in self.handles {
            let name = handle.server_name.clone();
            drop(handle);
            tracing::debug!("Disconnected from MCP server '{}'", name);
        }
    }
}
