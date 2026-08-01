use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerConfig {
    Command {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        /// Timeout for the connection/handshake with the server, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_timeout_secs: Option<u64>,
        /// Timeout for individual tool calls, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_timeout_secs: Option<u64>,
        /// Number of times a failed connection attempt is retried.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_retries: Option<u32>,
    },
    Url {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<OAuthConfig>,
        /// Timeout for the connection/handshake with the server, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_timeout_secs: Option<u64>,
        /// Timeout for individual tool calls, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_timeout_secs: Option<u64>,
        /// Number of times a failed connection attempt is retried.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connect_retries: Option<u32>,
    },
}

pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;
pub const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 20;
pub const DEFAULT_CONNECT_RETRIES: u32 = 1;

impl McpServerConfig {
    fn timeout_fields(&self) -> (Option<u64>, Option<u64>, Option<u32>) {
        match self {
            McpServerConfig::Command {
                connect_timeout_secs,
                tool_timeout_secs,
                connect_retries,
                ..
            }
            | McpServerConfig::Url {
                connect_timeout_secs,
                tool_timeout_secs,
                connect_retries,
                ..
            } => (*connect_timeout_secs, *tool_timeout_secs, *connect_retries),
        }
    }

    /// Timeout for establishing the connection and MCP handshake.
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(
            self.timeout_fields()
                .0
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS),
        )
    }

    /// Timeout for individual tool calls and tool listing.
    pub fn tool_timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_fields().1.unwrap_or(DEFAULT_TOOL_TIMEOUT_SECS))
    }

    /// Number of times a failed connection attempt is retried.
    pub fn connect_retries(&self) -> u32 {
        self.timeout_fields().2.unwrap_or(DEFAULT_CONNECT_RETRIES)
    }
}

/// OAuth settings for a URL-based MCP server.
///
/// Accepts either a bare `true` (enable with all defaults: dynamic client
/// registration, no extra scopes) or an object with explicit fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OAuthConfig {
    Enabled(bool),
    Settings(OAuthSettings),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthSettings {
    /// OAuth scopes to request. Empty means none are requested explicitly.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Pre-registered client id. When absent, dynamic client registration is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Loopback port for the redirect URI. Defaults to [`DEFAULT_REDIRECT_PORT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_port: Option<u16>,
}

pub const DEFAULT_REDIRECT_PORT: u16 = 8970;

impl OAuthConfig {
    /// Returns the resolved settings if OAuth is enabled, or `None` if disabled.
    pub fn settings(&self) -> Option<OAuthSettings> {
        match self {
            OAuthConfig::Enabled(false) => None,
            OAuthConfig::Enabled(true) => Some(OAuthSettings::default()),
            OAuthConfig::Settings(s) => Some(s.clone()),
        }
    }
}

impl OAuthSettings {
    pub fn redirect_port(&self) -> u16 {
        self.redirect_port.unwrap_or(DEFAULT_REDIRECT_PORT)
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.redirect_port())
    }
}
