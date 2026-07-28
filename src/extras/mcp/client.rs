use std::collections::HashMap;
use std::process::Stdio;

use compact_str::CompactString;
use rmcp::service::{RoleClient, RunningService, serve_client};
use rmcp::transport::child_process::TokioChildProcess;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::config::McpServerConfig;

pub struct McpClientHandle {
    pub server_name: CompactString,
    pub running_service: RunningService<RoleClient, ()>,
}

impl McpClientHandle {
    pub async fn connect(
        server_name: CompactString,
        config: &McpServerConfig,
    ) -> anyhow::Result<Self> {
        match config {
            McpServerConfig::Command { command, args, env } => {
                tracing::debug!(
                    "MCP command transport: {} {:?} ({} env vars)",
                    command,
                    args,
                    env.len(),
                );
                let mut cmd = Command::new(command);
                cmd.args(args);
                for (k, v) in env {
                    cmd.env(k, v);
                }
                // rmcp's child-process builder defaults stderr to `inherit`,
                // so a chatty server's logs write straight over the
                // alt-screen TUI. Pipe it and drain into tracing (log file)
                // instead — draining also keeps a verbose server from
                // deadlocking on a full pipe buffer.
                let (transport, stderr) = TokioChildProcess::builder(cmd)
                    .stderr(Stdio::piped())
                    .spawn()?;
                if let Some(stderr) = stderr {
                    let name = server_name.clone();
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            tracing::debug!("[mcp {name}] {line}");
                        }
                    });
                }
                let running_service = serve_client((), transport).await.map_err(|e| {
                    anyhow::anyhow!("MCP connection failed for '{server_name}': {e}")
                })?;
                Ok(Self {
                    server_name,
                    running_service,
                })
            }
            McpServerConfig::Url {
                url,
                headers,
                oauth,
            } => {
                tracing::debug!(
                    "MCP HTTP transport: {} ({} headers, OAuth: {})",
                    url,
                    headers.len(),
                    oauth.is_some(),
                );
                let custom_headers = parse_headers(headers)?;
                let cfg = rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url.as_str())
                    .custom_headers(custom_headers);

                let oauth_settings = oauth.as_ref().and_then(|o| o.settings());
                let running_service = if let Some(settings) = oauth_settings {
                    let auth_client =
                        super::oauth::build_auth_client(&server_name, url, &settings).await?;
                    type AuthHttpClient = rmcp::transport::StreamableHttpClientTransport<
                        rmcp::transport::auth::AuthClient<reqwest::Client>,
                    >;
                    let transport = AuthHttpClient::with_client(auth_client, cfg);
                    serve_client((), transport).await.map_err(|e| {
                        anyhow::anyhow!("MCP HTTP connection failed for '{server_name}': {e}")
                    })?
                } else {
                    type HttpClient =
                        rmcp::transport::StreamableHttpClientTransport<reqwest::Client>;
                    let transport = HttpClient::from_config(cfg);
                    serve_client((), transport).await.map_err(|e| {
                        anyhow::anyhow!("MCP HTTP connection failed for '{server_name}': {e}")
                    })?
                };
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
