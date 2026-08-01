pub mod client;
pub mod config;
pub mod oauth;
pub mod tool;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use compact_str::CompactString;
use tokio::sync::RwLock;
use tool::McpTool;

use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;

/// Shared reference to a server connection. Tools hold a clone so they can
/// reconnect the server themselves after a transport drop.
pub type SharedHandle = Arc<RwLock<client::McpClientHandle>>;

/// Bound on shutting down a single server connection so a wedged service
/// cannot hang application exit.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct McpClientManager {
    pub handles: Vec<SharedHandle>,
    /// Connection failures collected during `connect_all`, to be surfaced by the
    /// TUI via the renderer. We do NOT log these at `warn` because that writes to
    /// stderr, which corrupts the alt-screen TUI (overlapping the input box).
    pub notices: Vec<CompactString>,
}

impl McpClientManager {
    pub async fn connect_all(configs: &HashMap<String, config::McpServerConfig>) -> Self {
        tracing::debug!("MCP connecting to {} servers", configs.len());
        let results = futures::future::join_all(configs.iter().map(|(name, cfg)| async move {
            let connect_start = std::time::Instant::now();
            let result =
                client::McpClientHandle::connect(CompactString::new(name.clone()), cfg).await;
            (name, connect_start.elapsed(), result)
        }))
        .await;

        let mut handles = Vec::new();
        let mut notices = Vec::new();
        for (name, elapsed, result) in results {
            match result {
                Ok(handle) => {
                    tracing::info!("Connected to MCP server '{}' in {:?}", name, elapsed);
                    handles.push(Arc::new(RwLock::new(handle)));
                }
                Err(e) => {
                    tracing::debug!(
                        "Failed to connect to MCP server '{}' after {:?}: {e}",
                        name,
                        elapsed
                    );
                    notices.push(CompactString::new(format!(
                        "MCP server '{name}' not connected: {e}"
                    )));
                }
            }
        }
        notices.sort();
        Self { handles, notices }
    }

    /// Drain and return any pending connection notices.
    pub fn take_notices(&mut self) -> Vec<CompactString> {
        std::mem::take(&mut self.notices)
    }

    /// Find the shared handle for a server by name.
    pub async fn get_handle(&self, name: &str) -> Option<SharedHandle> {
        for shared in &self.handles {
            if shared.read().await.server_name == name {
                return Some(shared.clone());
            }
        }
        None
    }

    pub async fn collect_tools(
        &self,
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
    ) -> Vec<McpTool> {
        tracing::debug!("MCP collecting tools from {} handles", self.handles.len());
        let mut all_tools = Vec::new();
        for shared in &self.handles {
            let handle = shared.read().await;
            let server_name = handle.server_name.clone();
            match handle.list_tools().await {
                Ok(tools) => {
                    tracing::debug!("MCP server '{}': {} tools listed", server_name, tools.len(),);
                    for definition in tools {
                        all_tools.push(McpTool {
                            server_name: server_name.clone(),
                            definition,
                            handle: shared.clone(),
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
    /// available without restarting the session. When a handle already
    /// exists its inner connection is swapped, so tools built from it pick
    /// up the new connection.
    pub async fn reconnect(
        &mut self,
        name: &str,
        cfg: &config::McpServerConfig,
    ) -> anyhow::Result<()> {
        tracing::info!("MCP reconnecting server '{}'", name);
        let handle = client::McpClientHandle::connect(CompactString::new(name), cfg).await?;
        if let Some(shared) = self.get_handle(name).await {
            *shared.write().await = handle;
        } else {
            self.handles.push(Arc::new(RwLock::new(handle)));
        }
        Ok(())
    }

    pub async fn shutdown(self) {
        tracing::debug!("MCP shutting down {} connections", self.handles.len());
        for shared in self.handles {
            // `cancel` consumes the service, so it is only possible when no
            // tool still holds a reference to the handle. Otherwise the
            // connection is left to Drop at process exit.
            match Arc::try_unwrap(shared) {
                Ok(lock) => {
                    let handle = lock.into_inner();
                    let name = handle.server_name.clone();
                    // Explicitly shut down the running service so child
                    // processes and HTTP connections are cleaned up properly,
                    // rather than relying on Drop which may not await
                    // teardown. Bounded so a wedged service cannot hang
                    // application exit.
                    let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, handle.running_service.cancel())
                        .await;
                    tracing::debug!("Disconnected from MCP server '{}'", name);
                }
                Err(shared) => {
                    let name = shared.read().await.server_name.clone();
                    tracing::debug!(
                        "MCP server '{}' still referenced at shutdown, skipping cancel",
                        name
                    );
                }
            }
        }
    }
}
