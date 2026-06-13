use crate::session::MessageRole;
use crate::session::Session;
use crate::session::storage::{delete_session, find_sessions_by_prefix, save_session};
use std::env;
use std::sync::Mutex;

static STORAGE_LOCK: Mutex<()> = Mutex::new(());

struct TestEnv {
    dir: std::path::PathBuf,
    data_dir: String,
    _lock: std::sync::MutexGuard<'static, ()>,
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
