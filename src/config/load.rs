use std::collections::HashMap;
use std::path::{Path, PathBuf};

use compact_str::CompactString;

use std::io;

use crate::config::{
    Config, EditSystem, QuickModelConfig, StatusLineConfig, StatusLineLine, StatusLineSegment,
};
#[cfg(feature = "mcp")]
use crate::extras::mcp::config::McpServerConfig;
use crate::session::storage;

/// Candidate config filenames, in priority order within each search dir.
///
/// * `config.toml` — preferred format, especially for permission rules.
/// * `config.yaml` / `config.yml` — the documented non-TOML format.
/// * `config.json` — legacy fallback. YAML is a strict superset of JSON, so
///   existing JSON configs parse transparently through the YAML reader. This
///   entry exists purely so upgrades do not silently drop a user's config.
const CONFIG_CANDIDATES: [&str; 4] = ["config.toml", "config.yaml", "config.yml", "config.json"];

/// Pick the first existing candidate in `dir`, falling back to the preferred
/// `config.toml` path when none exist (so a fresh install seeds a TOML file).
pub(crate) fn pick_existing(dir: &Path) -> PathBuf {
    for name in CONFIG_CANDIDATES {
        let p = dir.join(name);
        if p.exists() {
            return p;
        }
    }
    dir.join(CONFIG_CANDIDATES[0])
}

fn resolve_config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("ZS_CONFIG_DIR") {
        return pick_existing(&PathBuf::from(dir));
    }

    if let Some(config_dir) = dirs::config_dir() {
        let dir = config_dir.join("zerostack");
        let picked = pick_existing(&dir);
        if picked.exists() {
            return picked;
        }
    }

    pick_existing(&storage::data_dir())
}

pub fn config_file_path() -> PathBuf {
    resolve_config_path()
}

fn default_quick_models() -> HashMap<String, QuickModelConfig> {
    let mut map = HashMap::new();
    map.insert(
        "deepseek-v4-flash".to_string(),
        QuickModelConfig {
            provider: CompactString::new("openrouter"),
            model: CompactString::new("deepseek/deepseek-v4-flash"),
            input_token_cost: 0.0983,
            output_token_cost: 0.1966,
            reserve_tokens: None,
            temperature: None,
            extra_body: None,
        },
    );
    map.insert(
        "deepseek-v4-pro".to_string(),
        QuickModelConfig {
            provider: CompactString::new("openrouter"),
            model: CompactString::new("deepseek/deepseek-v4-pro"),
            input_token_cost: 0.435,
            output_token_cost: 0.87,
            reserve_tokens: None,
            temperature: None,
            extra_body: None,
        },
    );
    map
}

pub fn quick_models_map(cfg: &Config) -> HashMap<String, QuickModelConfig> {
    cfg.quick_models.clone().unwrap_or_default()
}

pub fn save_quick_model(
    name: &str,
    provider: &str,
    model: &str,
    input_token_cost: f64,
    output_token_cost: f64,
) -> std::io::Result<()> {
    let path = resolve_config_path();
    let mut cfg: Config = if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        match path.extension().and_then(|e| e.to_str()) {
            Some("toml") => toml::from_str(&content).unwrap_or_default(),
            // YAML is a superset of JSON, so this also accepts legacy
            // `config.json` files transparently.
            _ => serde_yaml_ng::from_str(&content).unwrap_or_default(),
        }
    } else {
        Config::default()
    };

    let quick_models = cfg.quick_models.get_or_insert_with(HashMap::new);
    quick_models.insert(
        name.to_string(),
        QuickModelConfig {
            provider: CompactString::new(provider),
            model: CompactString::new(model),
            input_token_cost,
            output_token_cost,
            reserve_tokens: None,
            temperature: None,
            extra_body: None,
        },
    );

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid config path")
    })?;
    std::fs::create_dir_all(parent)?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("toml") => {
            let content = toml::to_string(&cfg).map_err(std::io::Error::other)?;
            std::fs::write(&path, content)?;
        }
        _ => std::fs::write(
            &path,
            serde_yaml_ng::to_string(&cfg).map_err(std::io::Error::other)?,
        )?,
    }
    Ok(())
}

fn rich_default_config() -> Config {
    Config {
        quick_models: Some(default_quick_models()),
        provider: Some(CompactString::new("openrouter")),
        model: Some(CompactString::new("deepseek/deepseek-v4-pro")),
        max_tokens: Some(16384),
        compact_enabled: Some(false),
        max_text_file_size: Some(1_048_576),
        edit_system: Some(EditSystem::Similarity),
        default_permission_mode: Some("standard".to_string()),
        default_prompt: Some(CompactString::new("code")),
        show_tool_details: None,
        chain: Some(crate::config::types::ChainConfig::default()),
        #[cfg(feature = "subagents")]
        subagent_max_read_lines: Some(2000),
        #[cfg(feature = "subagents")]
        subagent_max_grep_results: Some(200),
        #[cfg(feature = "subagents")]
        subagent_max_find_results: Some(200),
        #[cfg(feature = "advisor")]
        advisor: Some(crate::config::types::AdvisorConfig::default()),
        statusline: Some(StatusLineConfig {
            lines: vec![StatusLineLine {
                segments: vec![
                    StatusLineSegment {
                        item: "cwd".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some("  ".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "git_branch".into(),
                        color: Some("grey".into()),
                        left: Some("(".into()),
                        right: Some(")".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some(" | ".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "model".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some("  |  ".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "context_used".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some("/".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "context_max".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some(" ".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "context_percentage".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some("  \u{21d1}".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "tokens_input".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some(" \u{21d3}".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "tokens_output".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "flex_separator".into(),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "loop".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some(" ".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "mode".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some(" ".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "cost".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some(" ".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "btw".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "separator".into(),
                        text: Some(" ".into()),
                        ..Default::default()
                    },
                    StatusLineSegment {
                        item: "prompt".into(),
                        color: Some("grey".into()),
                        ..Default::default()
                    },
                ],
            }],
        }),
        ..Default::default()
    }
}

pub fn load() -> (Config, bool) {
    let path = resolve_config_path();
    let is_first_startup = !path.exists();
    #[allow(unused_mut)]
    let mut cfg: Config = if is_first_startup {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let default = rich_default_config();
        if path.extension().and_then(|e| e.to_str()) == Some("toml")
            && let Ok(content) = toml::to_string(&default)
        {
            std::fs::write(&path, content).ok();
        }
        default
    } else {
        let content = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!(
                "error: failed to read config file ({}): {}\n\
                 Fix the file or remove it to use defaults.",
                path.display(),
                e,
            );
            std::process::exit(1);
        });
        match path.extension().and_then(|e| e.to_str()) {
            Some("toml") => toml::from_str(&content).unwrap_or_else(|e| {
                eprintln!(
                    "error: {} is not a valid config: {}\n\
                      Fix the file or remove it to use defaults.",
                    path.display(),
                    e,
                );
                std::process::exit(1);
            }),
            _ => serde_yaml_ng::from_str(&content).unwrap_or_else(|e| {
                eprintln!(
                    "error: {} is not a valid config: {}\n\
                      Fix the file or remove it to use defaults.",
                    path.display(),
                    e,
                );
                std::process::exit(1);
            }),
        }
    };

    #[cfg(feature = "mcp")]
    inject_mcp_defaults(&mut cfg);

    (cfg, is_first_startup)
}

#[cfg(feature = "mcp")]
pub fn inject_mcp_defaults(cfg: &mut Config) {
    let mut servers = cfg.mcp_servers.take().unwrap_or_default();

    if cfg.resolve_enable_exa_mcp() {
        let mut headers = HashMap::new();
        if let Ok(key) = std::env::var("EXA_API_KEY") {
            headers.insert("x-api-key".to_string(), key);
        }
        servers
            .entry("Exa Web Search".to_string())
            .or_insert(McpServerConfig::Url {
                url: "https://mcp.exa.ai/mcp".to_string(),
                headers,
                oauth: None,
            });
    } else {
        servers.remove("Exa Web Search");
    }

    if cfg.resolve_enable_context7_mcp() {
        let mut headers = HashMap::new();
        if let Ok(key) = std::env::var("CONTEXT7_API_KEY") {
            headers.insert("authorization".to_string(), format!("Bearer {key}"));
        }
        servers
            .entry("Context7".to_string())
            .or_insert(McpServerConfig::Url {
                url: "https://mcp.context7.com/mcp".to_string(),
                headers,
                oauth: None,
            });
    } else {
        servers.remove("Context7");
    }

    if cfg.resolve_enable_grepapp_mcp() {
        let mut headers = HashMap::new();
        if let Ok(key) = std::env::var("GREP_APP_API_KEY") {
            headers.insert("authorization".to_string(), format!("Bearer {key}"));
        }
        servers
            .entry("Grep.app".to_string())
            .or_insert(McpServerConfig::Url {
                url: "https://mcp.grep.app".to_string(),
                headers,
                oauth: None,
            });
    } else {
        servers.remove("Grep.app");
    }

    cfg.mcp_servers = Some(servers);
}

pub fn save_config(cfg: &Config) -> io::Result<()> {
    let mut cfg = cfg.clone();
    #[cfg(feature = "mcp")]
    {
        if let Some(ref mut servers) = cfg.mcp_servers {
            servers.remove("Exa Web Search");
            servers.remove("Context7");
            servers.remove("Grep.app");
        }
    }
    let path = resolve_config_path();
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid config path"))?;
    std::fs::create_dir_all(parent)?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("toml") => {
            let content = toml::to_string(&cfg).map_err(io::Error::other)?;
            std::fs::write(&path, content)?;
        }
        _ => std::fs::write(
            &path,
            serde_yaml_ng::to_string(&cfg).map_err(io::Error::other)?,
        )?,
    }
    Ok(())
}
