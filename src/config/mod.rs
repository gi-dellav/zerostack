use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::session::storage;

#[cfg(feature = "mcp")]
use crate::extras::mcp::config::McpServerConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct CustomProviderConfig {
    pub provider_type: String,
    pub base_url: String,
    pub api_key_env: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub no_tools: Option<bool>,
    pub no_context_files: Option<bool>,
    pub context_window: Option<u64>,
    pub reserve_tokens: Option<u64>,
    pub keep_recent_tokens: Option<u64>,
    pub max_agent_turns: Option<usize>,
    pub compact_enabled: Option<bool>,
    pub custom_providers: Option<HashMap<String, CustomProviderConfig>>,
    pub permission: Option<serde_json::Value>,
    pub restrictive: Option<bool>,
    pub accept_all: Option<bool>,
    pub yolo: Option<bool>,
    pub sandbox: Option<bool>,
    pub default_permission_mode: Option<String>,
    pub show_tool_details: Option<bool>,
    pub default_prompt: Option<String>,
    #[cfg(feature = "mcp")]
    pub mcp_servers: Option<HashMap<String, McpServerConfig>>,
}

impl Config {
    pub fn custom_providers_map(&self) -> HashMap<String, CustomProviderConfig> {
        self.custom_providers.clone().unwrap_or_default()
    }

    pub fn resolve_context_window(&self) -> u64 {
        self.context_window.unwrap_or(128_000)
    }

    pub fn resolve_reserve_tokens(&self) -> u64 {
        self.reserve_tokens.unwrap_or(16_384)
    }

    pub fn resolve_keep_recent_tokens(&self) -> u64 {
        self.keep_recent_tokens.unwrap_or(20_000)
    }

    pub fn resolve_compact_enabled(&self) -> bool {
        self.compact_enabled.unwrap_or(true)
    }
}

pub fn config_file_path() -> PathBuf {
    storage::config_path().join("config.json")
}

pub fn load() -> Config {
    let path = config_file_path();
    let mut cfg: Config = if !path.exists() {
        Config::default()
    } else {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: failed to read config ({}): {}", path.display(), e);
                return Config::default();
            }
        };
        serde_json::from_str(&content).unwrap_or_else(|e| {
            eprintln!("warning: invalid config JSON ({}): {}", path.display(), e);
            Config::default()
        })
    };

    #[cfg(feature = "mcp")]
    if cfg.mcp_servers.is_none() {
        let mut headers = HashMap::new();
        if let Some(key) = std::env::var("EXA_API_KEY").ok() {
            headers.insert("x-api-key".to_string(), key);
        }
        let mut defaults = HashMap::new();
        defaults.insert(
            "Exa Web Search".to_string(),
            McpServerConfig::Url {
                url: "https://mcp.exa.ai/mcp".to_string(),
                headers,
            },
        );
        cfg.mcp_servers = Some(defaults);
    }

    cfg
}
