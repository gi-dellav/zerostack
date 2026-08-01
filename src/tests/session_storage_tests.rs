use crate::session::MessageRole;
use crate::session::Session;
use crate::session::ToolRecord;
use crate::session::storage::{
    delete_session, find_sessions_by_prefix, load_suffix, save_session, suffix_path,
};
use crate::session::{PromptRef, PromptSource};
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
    let mut s = Session::new("openai", "gpt-4", 128000, "");
    s.add_message(MessageRole::User, "hello");
    save_session(&s).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8]).unwrap();
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
    let s = Session::new("openai", "gpt-4", 128000, "");
    save_session(&s).unwrap();

    delete_session(&s.id).unwrap();
    let found = find_sessions_by_prefix(&s.id[..8]).unwrap();
    assert!(found.is_empty());
    drop(env);
}

#[test]
fn save_session_preserves_messages() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000, "");
    s.add_message(MessageRole::User, "question");
    s.add_message(MessageRole::Assistant, "answer");
    save_session(&s).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].messages.len(), 2);
    assert_eq!(found[0].messages[0].content, "question");
    assert_eq!(found[0].messages[1].content, "answer");
    drop(env);
}

#[test]
fn save_session_round_trips_prompt_provenance() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000, "");
    s.prompt = Some(PromptRef {
        name: "code".into(),
        source: PromptSource::BuiltIn,
    });
    save_session(&s).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].prompt,
        Some(PromptRef {
            name: "code".into(),
            source: PromptSource::BuiltIn,
        })
    );
    drop(env);
}

#[test]
fn old_session_file_without_prompt_field_loads_as_none() {
    let env = setup_test_env();
    // Simulates a session file written before `prompt` existed: build one via
    // `Session::new`, serialize it, then drop the `prompt` key entirely
    // before writing it to disk, as an old binary would have.
    let s = Session::new("anthropic", "claude", 200000, "");
    let mut value = serde_json::to_value(&s).unwrap();
    value.as_object_mut().unwrap().remove("prompt");
    let json = serde_json::to_string(&value).unwrap();
    let path = crate::session::storage::data_dir()
        .join("sessions")
        .join(format!("{}.json", s.id));
    std::fs::write(&path, json).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].prompt, None);
    drop(env);
}

#[cfg(any(feature = "subagents", feature = "acp"))]
#[test]
fn save_session_preserves_tool_messages() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000, "");
    s.add_message(MessageRole::User, "question");
    let call_id = s.add_tool_call("read", &serde_json::json!({ "path": "src/main.rs" }));
    s.add_tool_result(call_id, "read", "file contents");
    s.add_subagent_tool_call(
        Some(call_id),
        "task",
        &serde_json::json!({ "prompts": ["find x"] }),
    );
    s.add_message(MessageRole::Assistant, "answer");
    save_session(&s).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8]).unwrap();
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
    let mut s = Session::new("anthropic", "claude", 200000, "");
    let head = "H".repeat(TOOL_RESULT_HEAD_CHARS);
    let omitted = "M"
        .repeat(TOOL_RESULT_SAVE_THRESHOLD - TOOL_RESULT_HEAD_CHARS - TOOL_RESULT_TAIL_CHARS + 1);
    let tail = "T".repeat(TOOL_RESULT_TAIL_CHARS);
    let output = format!("{head}{omitted}{tail}");

    let returned = s.add_tool_result(0, "bash/unsafe", &output);

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

    let mut s = Session::new("anthropic", "claude", 200000, "");
    let output = "x".repeat(TOOL_RESULT_SAVE_THRESHOLD + 1);
    s.add_tool_result(0, "bash", &output);

    let content = s.messages[0].content.as_str();
    assert!(content.contains(&output));
    assert!(content.contains("failed to save long tool output separately"));
    let _ = std::fs::remove_file(path);
    drop(lock);
}

#[test]
fn tool_call_and_result_link_by_id() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000, "");
    let call_id = s.add_tool_call("read", &serde_json::json!({ "path": "src/main.rs" }));
    s.add_tool_result(call_id, "read", "file contents");

    match s.messages[0].tool.clone().expect("call record") {
        ToolRecord::Call { id, name, args } => {
            assert_eq!(id, call_id);
            assert_eq!(name, "read");
            assert_eq!(args, serde_json::json!({ "path": "src/main.rs" }));
        }
        other => panic!("expected ToolRecord::Call, got {other:?}"),
    }
    match s.messages[1].tool.clone().expect("result record") {
        ToolRecord::Result {
            call_id: linked,
            name,
            truncated,
            full_output_path,
        } => {
            assert_eq!(linked, call_id);
            assert_eq!(name, "read");
            assert!(!truncated);
            assert!(full_output_path.is_none());
        }
        other => panic!("expected ToolRecord::Result, got {other:?}"),
    }
    drop(env);
}

#[test]
fn oversized_tool_result_has_truncated_record_with_overflow_path() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000, "");
    let call_id = s.add_tool_call("bash", &serde_json::json!({ "command": "ls" }));
    let output = "x".repeat(TOOL_RESULT_SAVE_THRESHOLD + 1);
    s.add_tool_result(call_id, "bash", &output);

    match s.messages[1].tool.clone().expect("result record") {
        ToolRecord::Result {
            call_id: linked,
            truncated,
            full_output_path,
            ..
        } => {
            assert_eq!(linked, call_id);
            assert!(truncated);
            let path = full_output_path.expect("overflow path recorded");
            assert!(Path::new(path.as_str()).starts_with(&env.dir));
            assert_eq!(std::fs::read_to_string(path.as_str()).unwrap(), output);
        }
        other => panic!("expected ToolRecord::Result, got {other:?}"),
    }
    drop(env);
}

#[test]
fn tool_record_round_trips_through_json() {
    let call = ToolRecord::Call {
        id: 7,
        name: "read".into(),
        args: serde_json::json!({ "path": "a.rs" }),
    };
    let call_json = serde_json::to_value(&call).unwrap();
    assert_eq!(call_json["id"], 7);
    assert!(call_json.get("call_id").is_none());
    match serde_json::from_value::<ToolRecord>(call_json).unwrap() {
        ToolRecord::Call { id, name, args } => {
            assert_eq!(id, 7);
            assert_eq!(name, "read");
            assert_eq!(args, serde_json::json!({ "path": "a.rs" }));
        }
        other => panic!("expected ToolRecord::Call round-trip, got {other:?}"),
    }

    let result = ToolRecord::Result {
        call_id: 7,
        name: "read".into(),
        truncated: true,
        full_output_path: Some("/tmp/out.txt".into()),
    };
    let result_json = serde_json::to_value(&result).unwrap();
    assert_eq!(result_json["call_id"], 7);
    assert!(result_json.get("id").is_none());
    match serde_json::from_value::<ToolRecord>(result_json).unwrap() {
        ToolRecord::Result {
            call_id,
            truncated,
            full_output_path,
            ..
        } => {
            assert_eq!(call_id, 7);
            assert!(truncated);
            assert_eq!(full_output_path.as_deref(), Some("/tmp/out.txt"));
        }
        other => panic!("expected ToolRecord::Result round-trip, got {other:?}"),
    }
}

#[cfg(any(feature = "subagents", feature = "acp"))]
#[test]
fn subagent_tool_call_links_to_the_enclosing_task_call() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000, "");
    let call_id = s.add_tool_call("task", &serde_json::json!({ "prompts": ["find x"] }));
    s.add_subagent_tool_call(
        Some(call_id),
        "grep",
        &serde_json::json!({ "pattern": "fn x" }),
    );
    s.add_tool_result(call_id, "task", "found it");

    assert_eq!(s.messages[1].role, MessageRole::SubagentToolCall);
    match s.messages[1].tool.clone().expect("subagent call record") {
        ToolRecord::SubagentCall {
            parent_call_id,
            name,
            args,
        } => {
            assert_eq!(parent_call_id, call_id);
            assert_eq!(name, "grep");
            assert_eq!(args, serde_json::json!({ "pattern": "fn x" }));
        }
        other => panic!("expected ToolRecord::SubagentCall, got {other:?}"),
    }
    drop(env);
}

/// The linkage is never faked: with no enclosing call id to point at, the
/// message keeps its display summary and simply carries no structured
/// record, rather than claiming a parent it does not have.
#[cfg(any(feature = "subagents", feature = "acp"))]
#[test]
fn subagent_tool_call_without_a_parent_records_no_structured_record() {
    let env = setup_test_env();
    let mut s = Session::new("anthropic", "claude", 200000, "");
    s.add_subagent_tool_call(None, "grep", &serde_json::json!({ "pattern": "fn x" }));

    assert_eq!(s.messages[0].role, MessageRole::SubagentToolCall);
    assert!(s.messages[0].tool.is_none());
    assert!(s.messages[0].content.contains("grep"));
    drop(env);
}

/// `ToolRecord` is `#[serde(untagged)]`, so a subagent record must stay
/// structurally distinguishable from a plain `Call`: it carries
/// `parent_call_id` and deliberately no `id`, otherwise serde would resolve
/// its JSON to the `Call` variant listed first.
#[cfg(any(feature = "subagents", feature = "acp"))]
#[test]
fn subagent_tool_record_round_trips_without_colliding_with_call() {
    let subagent = ToolRecord::SubagentCall {
        parent_call_id: 3,
        name: "grep".into(),
        args: serde_json::json!({ "pattern": "fn x" }),
    };
    let json = serde_json::to_value(&subagent).unwrap();
    assert_eq!(json["parent_call_id"], 3);
    assert!(json.get("id").is_none());
    assert!(json.get("call_id").is_none());
    match serde_json::from_value::<ToolRecord>(json).unwrap() {
        ToolRecord::SubagentCall {
            parent_call_id,
            name,
            args,
        } => {
            assert_eq!(parent_call_id, 3);
            assert_eq!(name, "grep");
            assert_eq!(args, serde_json::json!({ "pattern": "fn x" }));
        }
        other => panic!("expected ToolRecord::SubagentCall round-trip, got {other:?}"),
    }

    // The reverse direction: a main-agent call must not be swallowed by the
    // subagent variant either.
    let call_json = serde_json::to_value(ToolRecord::Call {
        id: 3,
        name: "task".into(),
        args: serde_json::json!({ "prompts": ["find x"] }),
    })
    .unwrap();
    assert!(matches!(
        serde_json::from_value::<ToolRecord>(call_json).unwrap(),
        ToolRecord::Call { .. }
    ));
}

#[test]
fn old_session_file_without_tool_records_loads_with_tool_none() {
    let json = r#"{
        "id": "legacy-session",
        "name": "old session",
        "messages": [
            { "role": "user", "content": "hi", "estimated_tokens": 1 },
            { "role": "tool_call", "content": "read: src/main.rs", "estimated_tokens": 3 }
        ],
        "compactions": [],
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "total_cost": 0.0,
        "total_estimated_tokens": 4,
        "context_window": 128000,
        "model": "gpt-4",
        "provider": "openai",
        "working_dir": "/tmp"
    }"#;

    let session: Session = serde_json::from_str(json).expect("old session file loads");
    assert_eq!(session.messages.len(), 2);
    assert!(session.messages[0].tool.is_none());
    assert!(session.messages[1].tool.is_none());
    assert_eq!(session.next_tool_call_id, 0);
}

#[test]
fn find_all_sessions_returns_saved_sessions_newest_first() {
    let env = setup_test_env();
    let mut older = Session::new("openai", "gpt-4", 128000, "");
    older.updated_at = "2026-01-01T00:00:00Z".into();
    older.add_message(MessageRole::User, "older");
    older.updated_at = "2026-01-01T00:00:00Z".into();
    save_session(&older).unwrap();

    let mut newer = Session::new("anthropic", "claude", 200000, "");
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
    let mut s = Session::new("openai", "gpt-4", 128000, "");
    s.total_input_tokens = 100;
    s.total_output_tokens = 50;
    s.total_cost = 0.003;
    s.input_token_cost = 0.00001;
    s.output_token_cost = 0.00003;
    save_session(&s).unwrap();

    let found = find_sessions_by_prefix(&s.id[..8]).unwrap();
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
    let s = Session::new("openai", "gpt-4", 128000, "");
    save_session(&s).unwrap();
    let found = find_sessions_by_prefix(&s.id[..8]).unwrap();
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
