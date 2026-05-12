use std::path::PathBuf;

use serde::Deserialize;

use crate::session::storage;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub no_tools: Option<bool>,
    pub no_context_files: Option<bool>,
}

pub fn config_file_path() -> PathBuf {
    storage::config_path().join("config.json")
}

pub fn load() -> Config {
    let path = config_file_path();
    if !path.exists() {
        return Config::default();
    }
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
}
