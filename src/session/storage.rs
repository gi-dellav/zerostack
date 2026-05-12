use std::path::PathBuf;

use crate::session::Session;

fn session_dir() -> PathBuf {
    dirs_path().join("sessions")
}

fn home_fallback() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn dirs_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("ZS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let base = dirs::data_dir().unwrap_or_else(home_fallback);
    base.join("zerostack")
}

fn config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("ZS_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let base = dirs::config_dir().unwrap_or_else(|| home_fallback().join(".config"));
    base.join("zerostack")
}

pub fn save_session(session: &Session) -> anyhow::Result<()> {
    let dir = session_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", session.id));
    let json = serde_json::to_string_pretty(session)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn load_session(id: &str) -> anyhow::Result<Session> {
    let dir = session_dir();
    let path = dir.join(format!("{}.json", id));
    let json = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&json)?)
}

pub fn find_recent_sessions(limit: usize) -> anyhow::Result<Vec<Session>> {
    let dir = session_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions: Vec<Session> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(json) = std::fs::read_to_string(&path)
            && let Ok(session) = serde_json::from_str::<Session>(&json)
        {
            sessions.push(session);
        }
    }
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.truncate(limit);
    Ok(sessions)
}

pub fn agents_path() -> PathBuf {
    config_path().join("agent").join("AGENTS.md")
}
