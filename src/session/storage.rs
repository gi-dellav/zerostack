use std::path::PathBuf;

use uuid::Uuid;

use crate::session::Session;

fn session_dir() -> PathBuf {
    dirs_path().join("sessions")
}

pub fn tool_output_dir(session_id: &str) -> PathBuf {
    dirs_path()
        .join("tool-outputs")
        .join(safe_path_component(session_id))
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

/// Write `content` to `path` atomically: write to a temp file in the same
/// directory, then rename. On POSIX this is atomic; a crash mid-write leaves
/// the previous version intact.
pub fn atomic_write(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn save_session(session: &Session) -> anyhow::Result<()> {
    let dir = session_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", session.id));
    let json = serde_json::to_string(session)?;
    let json_len = json.len();
    atomic_write(&path, &json)?;
    tracing::debug!(
        "session saved: id={}, msgs={}, size={}",
        session.id,
        session.messages.len(),
        json_len,
    );
    Ok(())
}

pub fn save_tool_output(
    session_id: &str,
    tool_name: &str,
    output: &str,
) -> anyhow::Result<PathBuf> {
    let dir = tool_output_dir(session_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "{}-{}.txt",
        Uuid::new_v4(),
        safe_path_component(tool_name)
    ));
    std::fs::write(&path, output)?;
    Ok(path)
}

fn safe_path_component(value: &str) -> String {
    let safe: String = value
        .chars()
        .take(64)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if safe.is_empty() {
        "tool".to_string()
    } else {
        safe
    }
}

pub fn delete_session(id: &str) -> anyhow::Result<()> {
    let dir = session_dir();
    let path = dir.join(format!("{}.json", id));
    if path.exists() {
        std::fs::remove_file(&path)?;
        tracing::debug!("session deleted: id={}", id);
    } else {
        tracing::debug!("session delete skipped (not found): id={}", id);
    }
    Ok(())
}

pub fn find_sessions_by_prefix(prefix: &str) -> anyhow::Result<Vec<Session>> {
    let dir = session_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let lower = prefix.to_lowercase();
    let mut sessions: Vec<Session> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && let Ok(json) = std::fs::read_to_string(&path)
            && let Ok(session) = serde_json::from_str::<Session>(&json)
            && (stem.starts_with(prefix) || session.name.to_lowercase().contains(&lower))
        {
            sessions.push(session);
        }
    }
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.dedup_by(|a, b| a.id == b.id);
    tracing::debug!(
        "find_sessions_by_prefix('{}'): {} results",
        prefix,
        sessions.len(),
    );
    Ok(sessions)
}

pub fn find_session_by_name(name: &str) -> anyhow::Result<Option<Session>> {
    let dir = session_dir();
    if !dir.exists() {
        return Ok(None);
    }
    let lower = name.to_lowercase();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(json) = std::fs::read_to_string(&path)
            && let Ok(session) = serde_json::from_str::<Session>(&json)
            && session.name.to_lowercase() == lower
        {
            return Ok(Some(session));
        }
    }
    Ok(None)
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
    tracing::debug!(
        "find_recent_sessions(limit={}): {} results",
        limit,
        sessions.len(),
    );
    Ok(sessions)
}

pub fn agents_path() -> PathBuf {
    config_path().join("agent").join("AGENTS.md")
}

#[cfg(feature = "archmd")]
pub fn architecture_path() -> PathBuf {
    config_path().join("agent").join("ARCHITECTURE.md")
}

pub fn suffix_path() -> PathBuf {
    config_path().join("SUFFIX.md")
}

pub fn load_suffix() -> Option<String> {
    let path = suffix_path();
    if path.exists() {
        std::fs::read_to_string(path)
            .ok()
            .filter(|s| !s.trim().is_empty())
    } else {
        None
    }
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
