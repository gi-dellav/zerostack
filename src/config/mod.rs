pub mod load;
pub mod types;

use std::collections::HashMap;

use compact_str::CompactString;
use serde::{Deserialize, Serialize};

pub use load::*;
pub use types::*;

use crate::permission::{PermissionConfig, PermissionConfigs};

#[cfg(feature = "mcp")]
use crate::extras::mcp::config::McpServerConfig;

#[cfg(feature = "acp")]
use crate::extras::acp::config::AcpServerConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<CompactString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<CompactString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_tools: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_context_files: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserve_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_recent_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_agent_turns: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_text_file_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_read_lines: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bash_output_lines: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_grep_results: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_find_results: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_list_dir_entries: Option<u64>,
    // --- Subagent tool limits (applied when subagents spawn) ---
    #[cfg(feature = "subagents")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_max_read_lines: Option<u64>,
    #[cfg(feature = "subagents")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_max_grep_results: Option<u64>,
    #[cfg(feature = "subagents")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_max_find_results: Option<u64>,
    #[cfg(feature = "subagents")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_max_list_dir_entries: Option<u64>,
    // --- End subagent limits ---
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compact_enabled: Option<bool>,
    /// Opt-in mid-turn compaction threshold, as a fraction of the context
    /// window (0.0–1.0) of real provider prompt pressure. `None` (default)
    /// disables mid-turn compaction entirely; the agent only compacts between
    /// turns. Honored only when `compact_enabled` is also true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid_turn_compact_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub always_show_welcome: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "auto-update-prompts"
    )]
    pub auto_update_prompts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "auto-update-themes")]
    pub auto_update_themes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_providers: Option<HashMap<String, types::CustomProviderConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "permission-regex")]
    pub permission_regex: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "permission-allow")]
    pub permission_allow: Option<HashMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "permission-ask")]
    pub permission_ask: Option<HashMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "permission-deny")]
    pub permission_deny: Option<HashMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restrictive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accept_all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yolo: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "sandbox-backend")]
    pub sandbox_backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_all_mcp_calls: Option<bool>,
    #[cfg(feature = "mcp")]
    #[serde(skip_serializing_if = "Option::is_none", rename = "enable-exa-mcp")]
    pub enable_exa_mcp: Option<bool>,
    #[cfg(feature = "mcp")]
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "enable-context7-mcp"
    )]
    pub enable_context7_mcp: Option<bool>,
    #[cfg(feature = "mcp")]
    #[serde(skip_serializing_if = "Option::is_none", rename = "enable-grepapp-mcp")]
    pub enable_grepapp_mcp: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "permission-modes")]
    pub permission_modes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_tool_details: Option<ShowToolDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_prompt: Option<CompactString>,
    #[cfg(feature = "git-worktree")]
    #[serde(skip_serializing_if = "Option::is_none", rename = "wt-auto-merge")]
    pub wt_auto_merge: Option<bool>,
    #[cfg(feature = "git-worktree")]
    #[serde(skip_serializing_if = "Option::is_none", rename = "wt-base-dir")]
    pub wt_base_dir: Option<String>,

    #[cfg(feature = "git-worktree")]
    #[serde(skip_serializing_if = "Option::is_none", rename = "wt-force")]
    pub wt_force: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_keys: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quick_models: Option<HashMap<String, types::QuickModelConfig>>,
    #[cfg(feature = "mcp")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<HashMap<String, McpServerConfig>>,
    #[cfg(feature = "acp")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp_servers: Option<HashMap<String, AcpServerConfig>>,
    #[cfg(feature = "acp")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp_host: Option<String>,
    #[cfg(feature = "acp")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acp_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_system: Option<types::EditSystem>,
    #[cfg(feature = "subagents")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_max_turns: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny_repeated_reads: Option<bool>,
    #[cfg(feature = "subagents")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_enabled: Option<bool>,
    #[cfg(feature = "subagents")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_model: Option<CompactString>,
    #[cfg(feature = "subagents")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_provider: Option<CompactString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors: Option<types::ColorsConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<types::ChainConfig>,
    #[cfg(feature = "advisor")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advisor: Option<types::AdvisorConfig>,
}

impl Config {
    pub fn custom_providers_map(&self) -> HashMap<String, types::CustomProviderConfig> {
        self.custom_providers.clone().unwrap_or_default()
    }

    pub fn resolve_context_window(&self, provider: &str, model_id: &str) -> u64 {
        if let Some(cw) = self.context_window {
            return cw;
        }
        if let Some(entries) = crate::models_catalog::catalog_entries(provider) {
            for e in entries {
                if e.id == model_id {
                    if let Some(cl) = e.context_length {
                        return cl as u64;
                    }
                    break;
                }
            }
        }
        128_000
    }

    pub fn resolve_reserve_tokens(
        &self,
        model_id: &str,
        qm: &HashMap<String, types::QuickModelConfig>,
    ) -> u64 {
        if let Some(rt) = self.reserve_tokens {
            return rt;
        }
        for qmc in qm.values() {
            if qmc.model.as_str() == model_id {
                if let Some(rt) = qmc.reserve_tokens {
                    return rt;
                }
            }
        }
        8_192
    }

    pub fn resolve_keep_recent_tokens(&self) -> u64 {
        self.keep_recent_tokens.unwrap_or(10_000)
    }

    /// Resolves temperature: CLI `--temperature` > quick-model `temperature` >
    /// global `temperature`. Returns `None` when no temperature is configured.
    pub fn resolve_temperature(
        &self,
        cli: &crate::cli::Cli,
        model_id: &str,
        qm: &HashMap<String, types::QuickModelConfig>,
    ) -> Option<f64> {
        if let Some(temp) = cli.temperature {
            return Some(temp.clamp(0.0, 2.0));
        }
        for qmc in qm.values() {
            if qmc.model.as_str() == model_id {
                if let Some(temp) = qmc.temperature {
                    return Some(temp.clamp(0.0, 2.0));
                }
            }
        }
        self.temperature.map(|t| t.clamp(0.0, 2.0))
    }

    pub fn resolve_compact_enabled(&self) -> bool {
        self.compact_enabled.unwrap_or(true)
    }

    /// Mid-turn compaction pressure threshold as a fraction of the context
    /// window. Unlike the other resolvers this one substitutes **no** enabling
    /// default: `None` means the mid-turn trigger never fires (preserving the
    /// historical between-turn-only behavior). Values outside `(0.0, 1.0]` are
    /// treated as unset; [`load`](crate::config::load) warns about such values
    /// once at startup, since this resolver runs in the per-call hot path and
    /// must not log. The caller must additionally check
    /// [`resolve_compact_enabled`](Self::resolve_compact_enabled), which is the
    /// master switch for all compaction.
    pub fn resolve_mid_turn_compact_threshold(&self) -> Option<f64> {
        match self.mid_turn_compact_threshold {
            Some(t) if t > 0.0 && t <= 1.0 => Some(t),
            _ => None,
        }
    }

    pub fn resolve_max_read_lines(&self) -> u64 {
        self.max_read_lines.unwrap_or(2000)
    }

    /// Returns `None` when no cap is configured — preserves the historical
    /// "no bash output truncation" behaviour.
    pub fn resolve_max_bash_output_lines(&self) -> Option<u64> {
        self.max_bash_output_lines
    }

    pub fn resolve_max_grep_results(&self) -> u64 {
        self.max_grep_results.unwrap_or(150)
    }

    pub fn resolve_max_find_results(&self) -> u64 {
        self.max_find_results.unwrap_or(150)
    }

    pub fn resolve_max_list_dir_entries(&self) -> Option<u64> {
        self.max_list_dir_entries.or(Some(150))
    }

    #[cfg(feature = "subagents")]
    pub fn resolve_subagent_max_read_lines(&self) -> u64 {
        self.subagent_max_read_lines.unwrap_or(2000)
    }

    #[cfg(feature = "subagents")]
    pub fn resolve_subagent_max_grep_results(&self) -> u64 {
        self.subagent_max_grep_results.unwrap_or(200)
    }

    #[cfg(feature = "subagents")]
    pub fn resolve_subagent_max_find_results(&self) -> u64 {
        self.subagent_max_find_results.unwrap_or(200)
    }

    #[cfg(feature = "subagents")]
    pub fn resolve_subagent_max_list_dir_entries(&self) -> Option<u64> {
        self.subagent_max_list_dir_entries
    }

    pub fn resolve_always_show_welcome(&self) -> bool {
        self.always_show_welcome.unwrap_or(false)
    }

    pub fn resolve_auto_update_prompts(&self) -> Option<bool> {
        self.auto_update_prompts
    }

    pub fn resolve_auto_update_themes(&self) -> Option<bool> {
        self.auto_update_themes
    }

    #[cfg(feature = "mcp")]
    pub fn resolve_enable_exa_mcp(&self) -> bool {
        self.enable_exa_mcp.unwrap_or(true)
    }

    #[cfg(feature = "mcp")]
    pub fn resolve_enable_context7_mcp(&self) -> bool {
        self.enable_context7_mcp.unwrap_or(false)
    }

    #[cfg(feature = "mcp")]
    pub fn resolve_enable_grepapp_mcp(&self) -> bool {
        self.enable_grepapp_mcp.unwrap_or(false)
    }

    pub fn build_permission_config(&self) -> PermissionConfigs {
        let glob: PermissionConfig = self
            .permission
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let regex: PermissionConfig = self
            .permission_regex
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let mut perm_configs = PermissionConfigs { glob, regex };

        if let Some(allow) = &self.permission_allow {
            perm_configs.glob.allow_entries = Some(allow.clone());
        }
        if let Some(ask) = &self.permission_ask {
            perm_configs.glob.ask_entries = Some(ask.clone());
        }
        if let Some(deny) = &self.permission_deny {
            perm_configs.glob.deny_entries = Some(deny.clone());
        }

        perm_configs
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ShowToolDetails {
    Bool(bool),
    Lines(usize),
}

impl Default for ShowToolDetails {
    fn default() -> Self {
        ShowToolDetails::Lines(1)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ResolvedShowToolDetails {
    Off,
    Limited(usize),
    Unlimited,
}

/// Convenience: resolves temperature with all sources (CLI, quick model, global config).
pub fn resolve_temperature(cli: &crate::cli::Cli, cfg: &Config, model_id: &str) -> Option<f64> {
    let qm = quick_models_map(cfg);
    cfg.resolve_temperature(cli, model_id, &qm)
}

impl ShowToolDetails {
    pub fn resolve(&self) -> ResolvedShowToolDetails {
        match self {
            ShowToolDetails::Bool(false) => ResolvedShowToolDetails::Off,
            ShowToolDetails::Bool(true) => ResolvedShowToolDetails::Unlimited,
            ShowToolDetails::Lines(n) => ResolvedShowToolDetails::Limited(*n),
        }
    }
}
