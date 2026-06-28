use crate::session::MessageRole;
use crate::session::Session;
use crate::session::storage::{
    delete_session, find_sessions_by_prefix, load_suffix, save_session, suffix_path,
};
use crate::session::{TOOL_RESULT_HEAD_CHARS, TOOL_RESULT_SAVE_THRESHOLD, TOOL_RESULT_TAIL_CHARS};
use std::env;
use std::path::Path;
use std::sync::Mutex;

static STORAGE_LOCK: Mutex<()> = Mutex::new(());

struct TestEnv {
    dir: std::path::PathBuf,
    data_dir: String,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn setup_test_env() -> TestEnv {
    let lock = STORAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("zs_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data_dir = dir.to_str().unwrap().to_string();
    unsafe { env::set_var("ZS_DATA_DIR", &data_dir) };
    std::fs::create_dir_all(format!("{}/sessions", data_dir)).unwrap();
    TestEnv {
        dir,
        data_dir,
        _lock: lock,
    }
}

#[test]
fn save_and_find_session_by_prefix() {
    let env = setup_test_env();
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.add_message(MessageRole::User, "hello");
    save_session(&s).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8].to_string()).unwrap();
    assert_eq!(found.len(), 1, "id prefix: {}", &s.id[..8]);
    assert_eq!(found[0].id, s.id);
    assert_eq!(found[0].model.as_str(), "gpt-4");
    drop(env);
}

#[test]
fn find_sessions_by_prefix_no_match() {
    let env = setup_test_env();
    let found = find_sessions_by_prefix("nonexistent").unwrap();
    assert!(found.is_empty());
    drop(env);
}

#[test]
fn delete_session_removes_file() {
    let env = setup_test_env();
    let s = Session::new("openai", "gpt-4", 128000);
    save_session(&s).unwrap();

    delete_session(&s.id).unwrap();
    let found = find_sessions_by_prefix(&s.id[..8].to_string()).unwrap();
    assert!(found.is_empty());
    drop(env);
}

#[test]
fn save_session_preserves_messages() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000);
    s.add_message(MessageRole::User, "question");
    s.add_message(MessageRole::Assistant, "answer");
    save_session(&s).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8].to_string()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].messages.len(), 2);
    assert_eq!(found[0].messages[0].content, "question");
    assert_eq!(found[0].messages[1].content, "answer");
    drop(env);
}

#[test]
fn save_session_preserves_tool_messages() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000);
    s.add_message(MessageRole::User, "question");
    s.add_tool_call("read", &serde_json::json!({ "path": "src/main.rs" }));
    s.add_tool_result("read", "file contents");
    s.add_subagent_tool_call("task", &serde_json::json!({ "prompts": ["find x"] }));
    s.add_message(MessageRole::Assistant, "answer");
    save_session(&s).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8].to_string()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].messages.len(), 5);
    assert_eq!(found[0].messages[1].role, MessageRole::ToolCall);
    assert!(found[0].messages[1].content.contains("read"));
    assert_eq!(found[0].messages[2].role, MessageRole::ToolResult);
    assert_eq!(found[0].messages[2].content, "read:\nfile contents");
    assert_eq!(found[0].messages[3].role, MessageRole::SubagentToolCall);
    drop(env);
}

#[test]
fn long_tool_result_is_saved_and_truncated_in_session() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000);
    let head = "H".repeat(TOOL_RESULT_HEAD_CHARS);
    let omitted = "M"
        .repeat(TOOL_RESULT_SAVE_THRESHOLD - TOOL_RESULT_HEAD_CHARS - TOOL_RESULT_TAIL_CHARS + 1);
    let tail = "T".repeat(TOOL_RESULT_TAIL_CHARS);
    let output = format!("{head}{omitted}{tail}");

    let returned = s.add_tool_result("bash/unsafe", &output);

    let content = s.messages[0].content.as_str();
    assert_eq!(returned, content);
    assert!(content.starts_with(&format!("bash/unsafe:\n{head}")));
    assert!(content.ends_with(&tail));
    assert!(content.contains("[tool output truncated: 12001 characters; 2001 omitted]"));
    assert!(!content.contains(&"M".repeat(80)));

    let path_line = content
        .lines()
        .find(|line| line.starts_with("[full output saved to: "))
        .unwrap();
    assert!(path_line.contains("use the read tool on this path"));
    let path = path_line
        .trim_start_matches("[full output saved to: ")
        .split(';')
        .next()
        .unwrap();
    assert!(Path::new(path).starts_with(&env.dir));
    assert_eq!(std::fs::read_to_string(path).unwrap(), output);
    drop(env);
}

#[test]
fn long_tool_result_save_failure_keeps_full_output() {
    let lock = STORAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = std::env::temp_dir().join(format!("zs_data_file_{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, b"not a directory").unwrap();
    unsafe { env::set_var("ZS_DATA_DIR", path.to_str().unwrap()) };

    let mut s = Session::new("anthropic", "claude", 200000);
    let output = "x".repeat(TOOL_RESULT_SAVE_THRESHOLD + 1);
    s.add_tool_result("bash", &output);

    let content = s.messages[0].content.as_str();
    assert!(content.contains(&output));
    assert!(content.contains("failed to save long tool output separately"));
    let _ = std::fs::remove_file(path);
    drop(lock);
}

#[test]
fn find_all_sessions_returns_saved_sessions_newest_first() {
    let env = setup_test_env();
    let mut older = Session::new("openai", "gpt-4", 128000);
    older.updated_at = "2026-01-01T00:00:00Z".into();
    older.add_message(MessageRole::User, "older");
    older.updated_at = "2026-01-01T00:00:00Z".into();
    save_session(&older).unwrap();

    let mut newer = Session::new("anthropic", "claude", 200000);
    newer.updated_at = "2026-01-02T00:00:00Z".into();
    newer.add_message(MessageRole::User, "newer");
    newer.updated_at = "2026-01-02T00:00:00Z".into();
    save_session(&newer).unwrap();

    let found = find_sessions_by_prefix("").unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id, newer.id);
    assert_eq!(found[1].id, older.id);
    drop(env);
}

#[test]
fn save_session_preserves_cost_fields() {
    let env = setup_test_env();
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.total_input_tokens = 100;
    s.total_output_tokens = 50;
    s.total_cost = 0.003;
    s.input_token_cost = 0.00001;
    s.output_token_cost = 0.00003;
    save_session(&s).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8].to_string()).unwrap();
    assert_eq!(
        found.len(),
        1,
        "session id: {}, prefix: {}",
        s.id,
        &s.id[..8]
    );
    assert_eq!(found[0].total_input_tokens, 100);
    assert_eq!(found[0].total_output_tokens, 50);
    assert_eq!(found[0].total_cost, 0.003);
    drop(env);
}

#[test]
fn find_sessions_by_prefix_empty_for_nonexistent_dir() {
    let lock = STORAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("zs_nodir_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    unsafe { env::set_var("ZS_DATA_DIR", dir.to_str().unwrap()) };
    // Don't create the directory at all
    let found = find_sessions_by_prefix("anything").unwrap();
    assert!(found.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
    drop(lock);
}

#[test]
fn save_session_creates_parent_dirs() {
    let env = setup_test_env();
    // Delete sessions dir to verify save_session recreates it
    let sessions_dir = std::path::PathBuf::from(&env.data_dir).join("sessions");
    std::fs::remove_dir_all(&sessions_dir).unwrap();
    let s = Session::new("openai", "gpt-4", 128000);
    save_session(&s).unwrap();
    let found = find_sessions_by_prefix(&s.id[..8].to_string()).unwrap();
    assert_eq!(found.len(), 1);
    drop(env);
}

#[test]
fn load_suffix_returns_none_when_file_missing() {
    let env = setup_test_env();
    let result = load_suffix();
    assert!(result.is_none());
    drop(env);
}

#[test]
fn load_suffix_returns_none_when_file_is_empty() {
    let env = setup_test_env();
    let path = suffix_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, "").unwrap();
    let result = load_suffix();
    assert!(result.is_none());
    drop(env);
}

#[test]
fn load_suffix_returns_none_when_file_is_whitespace_only() {
    let env = setup_test_env();
    let path = suffix_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, "   \n  \t  \n").unwrap();
    let result = load_suffix();
    assert!(result.is_none());
    drop(env);
}

#[test]
fn load_suffix_returns_content_when_file_has_text() {
    let env = setup_test_env();
    let path = suffix_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, "Always respond in haiku form.").unwrap();
    let result = load_suffix();
    assert_eq!(result.as_deref(), Some("Always respond in haiku form."));
    drop(env);
}

#[test]
fn suffix_path_is_inside_config_dir() {
    let env = setup_test_env();
    let config = crate::session::storage::config_path();
    let suffix = suffix_path();
    assert_eq!(suffix, config.join("SUFFIX.md"));
    drop(env);
}
