use compact_str::CompactString;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisorConfig {
    pub enabled: bool,
    pub model: Option<CompactString>,
    pub provider: Option<CompactString>,
    #[serde(default = "default_advisor_max_turns")]
    pub max_turns: usize,
}

fn default_advisor_max_turns() -> usize {
    5
}
