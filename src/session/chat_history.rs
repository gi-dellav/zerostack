use std::path::PathBuf;

use compact_str::CompactString;
use serde::{Deserialize, Serialize};

use crate::session::storage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatHistoryEntry {
    pub content: String,
    pub timestamp: CompactString,
}

fn chat_history_path() -> PathBuf {
    storage::data_dir().join("chat_history.json")
}

pub fn append_entry(entry: &ChatHistoryEntry) -> anyhow::Result<()> {
    let path = chat_history_path();
    let mut entries: Vec<ChatHistoryEntry> = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => {
                // File is corrupt — back it up rather than silently discarding
                // all prior history.
                let bak = path.with_extension("json.bak");
                let _ = std::fs::rename(&path, &bak);
                tracing::warn!("chat history was corrupt, backed up to {:?}", bak);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    entries.push(entry.clone());
    let json = serde_json::to_string_pretty(&entries)?;
    storage::atomic_write(&path, &json)?;
    Ok(())
}

pub fn load_history() -> anyhow::Result<Vec<ChatHistoryEntry>> {
    let path = chat_history_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&content).unwrap_or_default())
}
