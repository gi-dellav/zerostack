use std::collections::HashMap;

use compact_str::CompactString;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickModelConfig {
    pub provider: CompactString,
    pub model: CompactString,
    #[serde(default)]
    pub input_token_cost: f64,
    #[serde(default)]
    pub output_token_cost: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiStyle {
    Responses,
    Completions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProviderConfig {
    pub provider_type: CompactString,
    pub base_url: String,
    pub api_key_env: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub danger_accept_invalid_certs: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_style: Option<ApiStyle>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<CompactString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditSystem {
    #[default]
    Similarity,
    Hashedit,
}

impl std::fmt::Display for EditSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditSystem::Similarity => write!(f, "similarity"),
            EditSystem::Hashedit => write!(f, "hashedit"),
        }
    }
}

impl std::str::FromStr for EditSystem {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "similarity" => Ok(EditSystem::Similarity),
            "hashedit" => Ok(EditSystem::Hashedit),
            _ => Err(format!(
                "unknown edit system '{}' (valid: similarity, hashedit)",
                s
            )),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorsConfig {
    // Background colors
    pub chat_background: Option<CompactString>,
    pub input_background: Option<CompactString>,
    pub status_background: Option<CompactString>,

    // Semantic foreground colors
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_text: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_the_way: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_block: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_text: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_marker: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_foreground: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_indicator: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picker_secondary: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picker_selected: Option<CompactString>,
}
