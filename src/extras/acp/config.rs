use serde::Deserialize;

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
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
