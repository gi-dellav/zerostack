use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use compact_str::CompactString;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientRequest, ContentBlock, JsonObject, ServerResult,
};
use rmcp::service::{Peer, PeerRequestOptions, RoleClient, ServiceError};
use tokio::sync::RwLock;

use crate::agent::tools::check_perm;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;

use super::client::McpClientHandle;

#[derive(Debug)]
pub struct McpToolError(pub CompactString);

impl fmt::Display for McpToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for McpToolError {}

pub struct McpTool {
    pub server_name: CompactString,
    pub definition: rmcp::model::Tool,
    pub handle: Arc<RwLock<McpClientHandle>>,
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
}

fn tool_err(msg: impl Into<String>) -> ToolError {
    ToolError::ToolCallError(Box::new(McpToolError(CompactString::new(msg.into()))))
}

/// Map an rmcp service error to a user-facing message, spelling out timeouts.
fn call_error_message(server_name: &str, tool_name: &str, e: &ServiceError) -> String {
    match e {
        ServiceError::Timeout { timeout } => format!(
            "MCP tool '{tool_name}' on server '{server_name}' timed out after {}s",
            timeout.as_secs()
        ),
        _ => format!("MCP tool error: {e}"),
    }
}

async fn call_tool_with_timeout(
    peer: &Peer<RoleClient>,
    params: CallToolRequestParams,
    timeout: Duration,
) -> Result<rmcp::model::CallToolResult, ServiceError> {
    let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
    let result = peer
        .send_cancellable_request(request, PeerRequestOptions::with_timeout(timeout))
        .await?
        .await_response()
        .await?;
    match result {
        ServerResult::CallToolResult(result) => Ok(result),
        _ => Err(ServiceError::UnexpectedResponse),
    }
}

impl ToolDyn for McpTool {
    fn name(&self) -> String {
        self.definition.name.to_string()
    }

    fn description(&self) -> String {
        self.definition
            .description
            .clone()
            .unwrap_or(Cow::from(""))
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(&self.definition.input_schema).unwrap_or_default()
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        let server_name = self.server_name.clone();
        let tool_name = self.definition.name.to_string();
        let handle = self.handle.clone();
        let permission = self.permission.clone();
        let ask_tx = self.ask_tx.clone();

        Box::pin(async move {
            let perm_key = format!("mcp_tool:{server_name}:{tool_name}");
            let coaching = check_perm(&permission, &ask_tx, "mcp_tool", &perm_key)
                .await
                .map_err(|e| tool_err(e.to_string()))?;

            let arguments: Option<JsonObject> = serde_json::from_str(&args).unwrap_or_default();
            let params = arguments
                .map(|a| CallToolRequestParams::new(tool_name.clone()).with_arguments(a))
                .unwrap_or_else(|| CallToolRequestParams::new(tool_name.clone()));

            let (peer, timeout) = {
                let h = handle.read().await;
                (h.peer(), h.tool_timeout)
            };
            let mut result = call_tool_with_timeout(&peer, params.clone(), timeout).await;

            // The transport died (child process exited, HTTP session dropped):
            // reconnect the server once and retry the call on the fresh peer.
            if matches!(result, Err(ServiceError::TransportClosed)) {
                tracing::info!(
                    "MCP server '{}' transport closed, attempting reconnect",
                    server_name
                );
                let mut h = handle.write().await;
                match McpClientHandle::connect(server_name.clone(), &h.config).await {
                    Ok(new_handle) => {
                        *h = new_handle;
                        result = call_tool_with_timeout(&h.peer(), params, h.tool_timeout).await;
                    }
                    Err(e) => {
                        return Err(tool_err(format!(
                            "MCP server '{server_name}' transport closed and reconnect failed: {e}"
                        )));
                    }
                }
            }

            let result =
                result.map_err(|e| tool_err(call_error_message(&server_name, &tool_name, &e)))?;

            if result.is_error.unwrap_or(false) {
                let error_msg = result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ContentBlock::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let msg = if error_msg.is_empty() {
                    "MCP tool returned an error".to_string()
                } else {
                    error_msg
                };
                return Err(tool_err(msg));
            }

            let mut content = String::new();
            for item in result.content {
                match item {
                    ContentBlock::Text(t) => content.push_str(&t.text),
                    ContentBlock::Image(img) => {
                        content.push_str(&format!("data:{};base64,{}", img.mime_type, img.data));
                    }
                    ContentBlock::Resource(r) => match &r.resource {
                        rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
                            content.push_str(text);
                        }
                        rmcp::model::ResourceContents::BlobResourceContents { blob, .. } => {
                            content.push_str(blob);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            if let Some(msg) = coaching {
                content = format!("{}\n\n{}", msg, content);
            }
            Ok(content)
        })
    }
}
