pub mod storage;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub messages: Vec<SessionMessage>,
    pub created_at: String,
    pub updated_at: String,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub model: String,
    pub provider: String,
    pub working_dir: String,
}

impl Session {
    pub fn new(provider: &str, model: &str) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Session {
            id: Uuid::new_v4().to_string(),
            name: String::new(),
            messages: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
            total_tokens: 0,
            total_cost: 0.0,
            model: model.to_string(),
            provider: provider.to_string(),
            working_dir: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        }
    }

    pub fn add_message(&mut self, role: &str, content: &str) {
        self.messages.push(SessionMessage {
            role: role.to_string(),
            content: content.to_string(),
        });
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }
}
