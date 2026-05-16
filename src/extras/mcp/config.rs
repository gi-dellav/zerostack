use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum McpServerConfig {
    Command {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Url {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}
