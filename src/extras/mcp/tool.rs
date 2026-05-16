use std::borrow::Cow;
use std::fmt;

use rig::completion::ToolDefinition;
use rig::tool::{ToolDyn, ToolError};
use rig::wasm_compat::WasmBoxedFuture;
use rmcp::model::{CallToolRequestParams, JsonObject, RawContent};
use rmcp::service::{Peer, RoleClient};

use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::agent::tools::check_perm;

#[derive(Debug)]
pub struct McpToolError(pub String);

impl fmt::Display for McpToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for McpToolError {}

pub struct McpTool {
    pub server_name: String,
    pub definition: rmcp::model::Tool,
    pub peer: Peer<RoleClient>,
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
}

impl ToolDyn for McpTool {
    fn name(&self) -> String {
        self.definition.name.to_string()
    }

    fn definition(&self, _prompt: String) -> WasmBoxedFuture<'_, ToolDefinition> {
        let name = self.definition.name.to_string();
        let description = self
            .definition
            .description
            .clone()
            .unwrap_or(Cow::from(""))
            .to_string();
        let parameters = serde_json::to_value(&self.definition.input_schema).unwrap_or_default();
        Box::pin(async move {
            ToolDefinition {
                name,
                description,
                parameters,
            }
        })
    }

    fn call(&self, args: String) -> WasmBoxedFuture<'_, Result<String, ToolError>> {
        let server_name = self.server_name.clone();
        let tool_name = self.definition.name.to_string();
        let peer = self.peer.clone();
        let permission = self.permission.clone();
        let ask_tx = self.ask_tx.clone();

        Box::pin(async move {
            let perm_key = format!("mcp_tool:{server_name}:{tool_name}");
            check_perm(&permission, &ask_tx, "mcp_tool", &perm_key)
                .await
                .map_err(|e| ToolError::ToolCallError(Box::new(McpToolError(e.to_string()))))?;

            let arguments: Option<JsonObject> = serde_json::from_str(&args).unwrap_or_default();
            let params = arguments
                .map(|a| CallToolRequestParams::new(tool_name.clone()).with_arguments(a))
                .unwrap_or_else(|| CallToolRequestParams::new(tool_name.clone()));

            let result = peer
                .call_tool(params)
                .await
                .map_err(|e| ToolError::ToolCallError(Box::new(McpToolError(format!("MCP tool error: {e}")))))?;

            if result.is_error.unwrap_or(false) {
                let error_msg = result
                    .content
                    .iter()
                    .filter_map(|c| match &c.raw {
                        RawContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let msg = if error_msg.is_empty() {
                    "MCP tool returned an error".to_string()
                } else {
                    error_msg
                };
                return Err(ToolError::ToolCallError(Box::new(McpToolError(msg))));
            }

            let mut content = String::new();
            for item in result.content {
                match item.raw {
                    RawContent::Text(t) => content.push_str(&t.text),
                    RawContent::Image(img) => {
                        content.push_str(&format!("data:{};base64,{}", img.mime_type, img.data));
                    }
                    RawContent::Resource(r) => match r.resource {
                        rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
                            content.push_str(&text);
                        }
                        rmcp::model::ResourceContents::BlobResourceContents { blob, .. } => {
                            content.push_str(&blob);
                        }
                    },
                    _ => {}
                }
            }
            Ok(content)
        })
    }
}
