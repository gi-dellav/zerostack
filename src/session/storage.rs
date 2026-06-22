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
    data_dir()
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ZS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let base = dirs::data_dir().unwrap_or_else(home_fallback);
    base.join("zerostack")
}

pub(crate) fn config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("ZS_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    data_dir()
}

pub fn save_session(session: &Session) -> anyhow::Result<()> {
    let dir = session_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", session.id));
    let json = serde_json::to_string(session)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn delete_session(id: &str) -> anyhow::Result<()> {
    let dir = session_dir();
    let path = dir.join(format!("{}.json", id));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub fn find_sessions_by_prefix(prefix: &str) -> anyhow::Result<Vec<Session>> {
    let dir = session_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions: Vec<Session> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && stem.starts_with(prefix)
            && let Ok(json) = std::fs::read_to_string(&path)
            && let Ok(session) = serde_json::from_str::<Session>(&json)
        {
            sessions.push(session);
        }
    }
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(sessions)
}

pub fn find_recent_sessions(limit: usize) -> anyhow::Result<Vec<Session>> {
    let dir = session_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    // Sort by filesystem mtime to avoid loading all sessions
    let mut entries: Vec<_> = std::fs::read_dir(&dir)?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|e| e == "json"))
        .map(|e| {
            let mtime = e.metadata().ok().and_then(|m| m.modified().ok());
            let path = e.path();
            (mtime, path)
        })
        .collect();

    // Sort newest first
    entries.sort_by_key(|b| std::cmp::Reverse(b.0));

    let mut sessions: Vec<Session> = Vec::new();
    for (_, path) in entries.iter().take(limit) {
        if let Ok(json) = std::fs::read_to_string(path)
            && let Ok(session) = serde_json::from_str::<Session>(&json)
        {
            sessions.push(session);
        }
    }
    Ok(sessions)
}

pub fn agents_path() -> PathBuf {
    config_path().join("agent").join("AGENTS.md")
}

#[cfg(feature = "archmd")]
pub fn architecture_path() -> PathBuf {
    config_path().join("agent").join("ARCHITECTURE.md")
}

fn theme_file_path() -> PathBuf {
    data_dir().join("theme.json")
}

pub fn save_theme_name(name: Option<&str>) -> anyhow::Result<()> {
    let path = theme_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let value = match name {
        Some(n) => serde_json::json!({ "theme": n }),
        None => serde_json::json!({ "theme": null }),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&value)?)?;
    Ok(())
}

pub fn load_theme_name() -> Option<String> {
    let path = theme_file_path();
    let content = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    value.get("theme")?.as_str().map(|s| s.to_string())
}
