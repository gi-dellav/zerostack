pub mod storage;

use compact_str::CompactString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: MessageRole,
    pub content: CompactString,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: CompactString,
    pub name: CompactString,
    pub messages: Vec<SessionMessage>,
    pub created_at: CompactString,
    pub updated_at: CompactString,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub model: CompactString,
    pub provider: CompactString,
    pub working_dir: CompactString,
}

impl Session {
    pub fn new(provider: &str, model: &str) -> Self {
        let now = CompactString::new(chrono::Utc::now().to_rfc3339());
        Session {
            id: CompactString::new(Uuid::new_v4().to_string()),
            name: CompactString::new(""),
            messages: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
            total_tokens: 0,
            total_cost: 0.0,
            model: CompactString::new(model),
            provider: CompactString::new(provider),
            working_dir: std::env::current_dir()
                .map(|p| CompactString::new(p.to_string_lossy()))
                .unwrap_or_default(),
        }
    }

    pub fn add_message(&mut self, role: MessageRole, content: &str) {
        self.messages.push(SessionMessage {
            role,
            content: CompactString::new(content),
        });
        self.updated_at = CompactString::new(chrono::Utc::now().to_rfc3339());
    }
}
