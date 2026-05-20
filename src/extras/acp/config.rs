use serde::{Deserialize, Serialize};

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AcpServerConfig {
    Tcp {
        host: String,
        port: u16,
        #[serde(default)]
        api_key: Option<String>,
    },
    Stdio,
}
