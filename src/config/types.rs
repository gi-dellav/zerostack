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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_tokens: Option<u64>,
    /// Per-model temperature override (0.0–2.0). Takes precedence over the
    /// global `temperature` setting but is overridden by `--temperature`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Provider-specific JSON shallow-merged into the completion request body
    /// (e.g. OpenRouter `plugins` routing presets). Overrides the global
    /// `extra_body` for this model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
    /// Per-model context window override. Takes precedence over the static
    /// model catalog but is overridden by the global `context_window` setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// Status-bar statusline layout. Up to 3 lines, each an ordered list of segments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusLineConfig {
    #[serde(default)]
    pub lines: Vec<StatusLineLine>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatusLineLine {
    #[serde(default)]
    pub segments: Vec<StatusLineSegment>,
}

/// Icon for a statusline item: `true` uses the item's built-in glyph, or a
/// string sets a custom one (a named icon like `branch`, or a literal glyph).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IconSpec {
    Auto(bool),
    Custom(CompactString),
}

/// One statusline piece. `item` names the element (see `docs/CONFIG.md`).
/// `color`/`bg` are named colors or `#rrggbb`. `text` is the literal for the
/// `separator` item. `left`/`right` are powerline cap glyphs drawn before/after
/// the item. `icon` shows a glyph before the value.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatusLineSegment {
    pub item: CompactString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<CompactString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<IconSpec>,
    /// Force a numeric item (`tokens_input`, `tokens_output`, `cost`) to show
    /// even when its value is zero (normally hidden).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always: Option<bool>,
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
#[serde(rename_all = "lowercase")]
pub enum SchemeType {
    #[default]
    Full,
    Ansi,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorsConfig {
    pub chat_background: Option<CompactString>,
    pub input_background: Option<CompactString>,
    pub status_background: Option<CompactString>,
    /// Semantic role → color overrides, e.g. `{ "agent": "white",
    /// "error": "#ff5555", "tool": "yellow", "permission": "magenta" }`.
    /// Known roles: user, agent, reasoning, tool, tool_result, error,
    /// system, welcome, permission, plain.
    pub roles: Option<HashMap<String, String>>,
    #[serde(default)]
    pub scheme_type: SchemeType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChainConfig {
    #[serde(rename = "brainstorm-to-plan")]
    pub brainstorm_to_plan: bool,
    #[serde(rename = "plan-to-code")]
    pub plan_to_code: bool,
    #[serde(rename = "code-to-review")]
    pub code_to_review: bool,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            brainstorm_to_plan: true,
            plan_to_code: true,
            code_to_review: false,
        }
    }
}

/// Configuration for LSP (Language Server Protocol) integration. When
/// enabled, language servers are spawned lazily for edited files and
/// diagnostics are fed back to the agent after edits.
#[cfg(feature = "lsp")]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LspConfig {
    pub enabled: bool,
    /// Per-server overrides or custom servers, keyed by name. An entry with
    /// the same name as a built-in default replaces it.
    pub servers: HashMap<String, LspServerConfig>,
}

#[cfg(feature = "lsp")]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LspServerConfig {
    /// Server binary, resolved via PATH. Empty when the entry only disables
    /// a built-in server.
    pub command: CompactString,
    pub args: Vec<CompactString>,
    /// File extensions this server handles, e.g. [".rs"].
    pub extensions: Vec<CompactString>,
    pub env: HashMap<String, String>,
    /// Server-specific `initializationOptions` sent during `initialize`.
    pub initialization: Option<serde_json::Value>,
    pub disabled: bool,
}

/// Configuration for the rtk (https://github.com/rtk-ai/rtk) output-filtering
/// proxy. When enabled, bash commands are passed through `rtk rewrite` before
/// execution so supported commands return compact output.
#[cfg(feature = "rtk")]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RtkConfig {
    pub enabled: bool,
    /// Path to the rtk binary. Defaults to `rtk` (resolved via PATH).
    pub path: Option<CompactString>,
}

#[cfg(feature = "advisor")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvisorConfig {
    pub enabled: bool,
    pub model: Option<CompactString>,
    pub max_uses: Option<usize>,
    pub human_handoff: bool,
    pub advisor_kilobytes_limit: u32,
}

#[cfg(feature = "advisor")]
impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: Some(CompactString::new("deepseek-v4-pro")),
            max_uses: Some(3),
            human_handoff: true,
            advisor_kilobytes_limit: 256,
        }
    }
}
