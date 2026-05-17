use std::collections::HashMap;

use rmcp::service::{RoleClient, RunningService, serve_client};
use rmcp::transport::child_process::TokioChildProcess;
use tokio::process::Command;

use super::config::McpServerConfig;

pub struct McpClientHandle {
    pub server_name: String,
    pub running_service: RunningService<RoleClient, ()>,
}

impl McpClientHandle {
    pub async fn connect(server_name: String, config: &McpServerConfig) -> anyhow::Result<Self> {
        match config {
            McpServerConfig::Command { command, args, env } => {
                let mut cmd = Command::new(command);
                cmd.args(args);
                for (k, v) in env {
                    cmd.env(k, v);
                }
                let transport = TokioChildProcess::new(cmd)?;
                let running_service = serve_client((), transport).await.map_err(|e| {
                    anyhow::anyhow!("MCP connection failed for '{server_name}': {e}")
                })?;
                Ok(Self {
                    server_name,
                    running_service,
                })
            }
            McpServerConfig::Url { url, headers } => {
                let custom_headers = parse_headers(headers)?;
                let cfg = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url.as_str())
                    .custom_headers(custom_headers);
                type HttpClient = rmcp::transport::StreamableHttpClientTransport<reqwest::Client>;
                let transport = HttpClient::from_config(cfg);
                let running_service = serve_client((), transport).await.map_err(|e| {
                    anyhow::anyhow!("MCP HTTP connection failed for '{server_name}': {e}")
                })?;
                Ok(Self {
                    server_name,
                    running_service,
                })
            }
        }
    }

    pub fn peer(&self) -> rmcp::service::Peer<RoleClient> {
        self.running_service.peer().clone()
    }

    pub async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, rmcp::ServiceError> {
        self.running_service.peer().list_all_tools().await
    }
}

fn parse_headers(
    headers: &HashMap<String, String>,
) -> anyhow::Result<HashMap<http::HeaderName, http::HeaderValue>> {
    let mut result = HashMap::new();
    for (name, value) in headers {
        let h_name: http::HeaderName = name
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid header name '{name}': {e}"))?;
        let h_value: http::HeaderValue = value
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid header value for '{name}': {e}"))?;
        result.insert(h_name, h_value);
    }
    Ok(result)
}
