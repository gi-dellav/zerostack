use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use compact_str::CompactString;
use rig::message::{MimeType, ToolResultContent};
use rig::tool::{DynamicTool, ToolContext, ToolExecutionError, ToolOutput};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientRequest, ContentBlock, JsonObject, ServerResult,
};
use rmcp::service::{Peer, PeerRequestOptions, RoleClient, ServiceError};
use tokio::sync::RwLock;

use crate::agent::tools::check_perm;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;

use super::client::McpClientHandle;

fn tool_err(msg: impl Into<String>) -> ToolExecutionError {
    ToolExecutionError::other(msg.into())
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

/// Render collected MCP tool-result content as canonical model content.
///
/// Text-only results stay a single text block. Images become real multimodal
/// `ToolResultContent::Image` blocks, so the model receives pixels rather than
/// base64 as text. Image parts sort after all text parts.
pub(crate) fn render_result(texts: Vec<String>, images: Vec<(String, String)>) -> ToolOutput {
    if images.is_empty() {
        return ToolOutput::text(texts.join("\n"));
    }
    let mut blocks: Vec<ToolResultContent> = Vec::new();
    let response = texts.join("\n\n");
    if !response.is_empty() {
        blocks.push(ToolResultContent::text(response));
    }
    for (mime_type, data) in images {
        match rig::completion::message::ImageMediaType::from_mime_type(&mime_type) {
            Some(media_type) => {
                blocks.push(ToolResultContent::image_base64(
                    data,
                    Some(media_type),
                    None,
                ));
            }
            None => blocks.push(ToolResultContent::text(format!(
                "[unsupported image mime type {mime_type} dropped]"
            ))),
        }
    }
    ToolOutput::content(blocks).unwrap_or_else(|_| ToolOutput::text(""))
}

/// A single MCP server tool as a rig [`DynamicTool`]. Built fresh per call
/// from the live server definition (the definition can change across
/// `list_tools` refreshes); the permission gate and the reconnect-on-drop
/// retry live inside the closure.
pub(crate) fn mcp_dynamic_tool(
    server_name: CompactString,
    definition: rmcp::model::Tool,
    handle: Arc<RwLock<McpClientHandle>>,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
) -> DynamicTool {
    let name = definition.name.to_string();
    let description = definition
        .description
        .clone()
        .unwrap_or(Cow::from(""))
        .to_string();
    let parameters = serde_json::to_value(&definition.input_schema).unwrap_or_default();
    DynamicTool::new_with_context(
        name,
        description,
        parameters,
        move |_: &mut ToolContext, args: serde_json::Value| {
            let server_name = server_name.clone();
            let tool_name = definition.name.to_string();
            let handle = handle.clone();
            let permission = permission.clone();
            let ask_tx = ask_tx.clone();

            Box::pin(async move {
                let perm_key = format!("mcp_tool:{server_name}:{tool_name}");
                let coaching = check_perm(&permission, &ask_tx, "mcp_tool", &perm_key)
                    .await
                    .map_err(|e| tool_err(e.to_string()))?;

                let arguments: Option<JsonObject> =
                    serde_json::from_value(args.clone()).unwrap_or_default();
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
                            result =
                                call_tool_with_timeout(&h.peer(), params, h.tool_timeout).await;
                        }
                        Err(e) => {
                            return Err(tool_err(format!(
                                "MCP server '{server_name}' transport closed and reconnect failed: {e}"
                            )));
                        }
                    }
                }

                let result = result
                    .map_err(|e| tool_err(call_error_message(&server_name, &tool_name, &e)))?;

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

                let mut texts: Vec<String> = Vec::new();
                let mut images: Vec<(String, String)> = Vec::new();
                for item in result.content {
                    match item {
                        ContentBlock::Text(t) => texts.push(t.text),
                        ContentBlock::Image(img) => images.push((img.mime_type, img.data)),
                        ContentBlock::Resource(r) => match &r.resource {
                            rmcp::model::ResourceContents::TextResourceContents {
                                text, ..
                            } => {
                                texts.push(text.clone());
                            }
                            rmcp::model::ResourceContents::BlobResourceContents {
                                mime_type,
                                blob,
                                ..
                            } => match mime_type.as_deref() {
                                Some(m) if m.starts_with("image/") => {
                                    images.push((m.to_string(), blob.clone()));
                                }
                                _ => texts.push(blob.clone()),
                            },
                            _ => {}
                        },
                        _ => {}
                    }
                }
                if let Some(msg) = coaching {
                    texts.insert(0, msg);
                }
                let rendered = render_result(texts, images);
                Ok(rendered)
            })
        },
    )
}
