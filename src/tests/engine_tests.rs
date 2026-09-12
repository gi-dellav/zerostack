//! Tests for the headless [`Engine`](crate::engine::Engine) (`run_string`).
//!
//! The engine is driven with a scripted `AnyAgent::Mock` (rig's
//! `MockCompletionModel`, see `fake_model.rs`), so a full
//! user-message → agent-response round trip runs with no network and no
//! terminal. `ZS_DATA_DIR`/`ZS_CONFIG_DIR` are isolated per test process so
//! session persistence never touches the developer's real data dir.

#![allow(clippy::await_holding_lock)]

use std::collections::HashMap;

use crate::cli::Cli;
use crate::config::Config;
use crate::engine::{Engine, RunKind};
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::tests::fake_model::{self, FakeModel};

fn isolate_data_dirs() {
    let dir = std::env::temp_dir().join(format!(
        "zerostack-engine-tests-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&dir).unwrap();
    unsafe { std::env::set_var("ZS_DATA_DIR", &dir) };
    unsafe { std::env::set_var("ZS_CONFIG_DIR", &dir) };
}

fn test_cli() -> Cli {
    Cli {
        api_key: Some("test-key".to_string()),
        no_session: true,
        no_color: true,
        ..Default::default()
    }
}

fn test_client() -> AnyClient {
    crate::provider::create_client("anthropic", Some("test-key"), &HashMap::new(), None)
        .expect("create test client")
}

fn test_session() -> Session {
    Session::new("anthropic", "claude-sonnet-4-5", 200_000, "engine-test")
}

fn test_context() -> crate::context::ContextFiles {
    crate::context::load_with_prompts_dirs(true, &[])
}

fn engine_with_turns(turns: Vec<Vec<&str>>) -> (Engine, FakeModel) {
    isolate_data_dirs();
    let model = fake_model::text_turns(turns);
    let agent = AnyAgent::Mock(rig::agent::AgentBuilder::new(model.clone()).build());
    let engine = Engine::new(
        test_cli(),
        Config::default(),
        test_session(),
        test_context(),
        test_client(),
        None,
        Sandbox::new(false, "bwrap"),
    )
    .with_agent(agent);
    (engine, model)
}

#[tokio::test]
async fn run_string_empty_is_ignored() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);
    for input in ["", "   ", "\n\t "] {
        let out = engine.run_string(input).await.expect("run_string");
        assert_eq!(out.kind, RunKind::Ignored);
        assert!(out.text.is_empty());
    }
    assert!(engine.session().messages.is_empty());
}

#[tokio::test]
async fn run_string_message_runs_agent_and_updates_session() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, model) = engine_with_turns(vec![vec!["hi there"]]);

    let out = engine.run_string("hello").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Agent);
    assert_eq!(out.text, "hi there");

    let messages = &engine.session().messages;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[0].content.as_str(), "hello");
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[1].content.as_str(), "hi there");

    // History handed to the model excluded the trailing prompt (rig
    // invariant); the first turn carries no prior history.
    assert_eq!(model.requests().len(), 1);
    assert!(fake_model::history_at(&model, 0).is_empty());
}

#[tokio::test]
async fn run_string_second_turn_sees_first_turn_history() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, model) = engine_with_turns(vec![vec!["one"], vec!["two"]]);

    engine.run_string("first").await.expect("turn 1");
    engine.run_string("second").await.expect("turn 2");

    assert_eq!(model.requests().len(), 2);
    // Turn 2's history is the two committed messages of turn 1 rendered
    // through `convert_history` (User + Assistant).
    let history = fake_model::history_at(&model, 1);
    assert_eq!(history.len(), 2);
}

#[tokio::test]
async fn run_string_agent_error_rolls_back_user_message() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    isolate_data_dirs();
    let model = fake_model::FakeModel::from_stream_turns(vec![vec![
        crate::tests::fake_model::MockStreamEvent::error("stream broke"),
    ]]);
    let agent = AnyAgent::Mock(rig::agent::AgentBuilder::new(model).build());
    let mut engine = Engine::new(
        test_cli(),
        Config::default(),
        test_session(),
        test_context(),
        test_client(),
        None,
        Sandbox::new(false, "bwrap"),
    )
    .with_agent(agent);

    let out = engine.run_string("hello").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Agent);
    assert!(
        out.text.contains("stream broke"),
        "error surfaces in output: {}",
        out.text
    );
    assert!(
        engine.session().messages.is_empty(),
        "failed turn leaves no optimistic user message"
    );
}

#[tokio::test]
async fn run_string_bang_runs_shell_and_records_turn() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("!echo hi").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Agent);
    assert_eq!(out.text, "hi");

    let messages = &engine.session().messages;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[0].content.as_str(), "!echo hi");
    assert_eq!(messages[1].role, MessageRole::Assistant);
    assert_eq!(messages[1].content.as_str(), "hi");
}

#[tokio::test]
async fn run_string_bang_empty_is_command_error() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("!  ").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("empty command"), "got: {}", out.text);
    assert!(engine.session().messages.is_empty());
}

#[tokio::test]
async fn run_string_help_lists_commands() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("/help").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("/add"), "got: {}", out.text);
    assert!(out.text.contains("/compress"), "got: {}", out.text);
    // No session mutation for read-only commands.
    assert!(engine.session().messages.is_empty());
}

#[tokio::test]
async fn run_string_unknown_command_errors() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine
        .run_string("/nope-not-a-command")
        .await
        .expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("unknown command"), "got: {}", out.text);
}

#[tokio::test]
async fn run_string_sessions_lists_without_mutation() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("/sessions").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(
        out.text.contains("no saved sessions"),
        "isolated data dir has none: {}",
        out.text
    );
    assert!(engine.session().messages.is_empty());
}

#[tokio::test]
async fn run_string_rename_persists_in_session() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("/rename demo").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("demo"), "got: {}", out.text);
    assert_eq!(engine.session().name.as_str(), "demo");
}

#[tokio::test]
async fn run_string_undo_redo_round_trip() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![vec!["reply"]]);

    engine.run_string("hello").await.expect("turn");
    assert_eq!(engine.session().messages.len(), 2);

    let out = engine.run_string("/undo").await.expect("undo");
    assert_eq!(out.kind, RunKind::Command);
    assert!(engine.session().messages.is_empty(), "got: {}", out.text);

    let out = engine.run_string("/redo").await.expect("redo");
    assert_eq!(out.kind, RunKind::Command);
    assert_eq!(engine.session().messages.len(), 2);
}

#[tokio::test]
async fn run_string_clear_empties_session() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![vec!["reply"]]);

    engine.run_string("hello").await.expect("turn");
    engine.run_string("/clear").await.expect("clear");
    assert!(engine.session().messages.is_empty());
}

#[tokio::test]
async fn run_string_retry_reruns_last_message() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, model) = engine_with_turns(vec![vec!["one"], vec!["two"]]);

    engine.run_string("hello").await.expect("turn 1");
    let out = engine.run_string("/retry").await.expect("retry");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("two"), "got: {}", out.text);
    assert_eq!(model.requests().len(), 2);
}

#[tokio::test]
async fn run_string_mode_switches_security_mode() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    use std::sync::{Arc, Mutex};

    use crate::permission::checker::PermissionChecker;
    use crate::permission::{PermissionConfigs, SecurityMode};

    isolate_data_dirs();
    let checker = PermissionChecker::new(
        &PermissionConfigs::default(),
        SecurityMode::Standard,
        None,
        None,
    );
    let perm: crate::permission::checker::PermCheck = Arc::new(Mutex::new(checker));
    let model = fake_model::text_turns(Vec::<Vec<&str>>::new());
    let agent = AnyAgent::Mock(rig::agent::AgentBuilder::new(model).build());
    let mut engine = Engine::new(
        test_cli(),
        Config::default(),
        test_session(),
        test_context(),
        test_client(),
        Some(perm.clone()),
        Sandbox::new(false, "bwrap"),
    )
    .with_agent(agent);

    let out = engine
        .run_string("/mode readonly")
        .await
        .expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert_eq!(
        perm.lock().unwrap().mode(),
        SecurityMode::ReadOnly,
        "got: {}",
        out.text
    );
}

#[tokio::test]
async fn run_string_reasoning_toggles() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("/reasoning").await.expect("toggle");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("off"), "got: {}", out.text);

    let out = engine.run_string("/reasoning").await.expect("toggle");
    assert!(out.text.contains("on"), "got: {}", out.text);
}

#[tokio::test]
async fn run_string_rewind_reports_headless_limitation() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("/rewind").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("TUI"), "got: {}", out.text);
}

#[tokio::test]
async fn run_string_quit_does_not_exit_process() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("/quit").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("quit"), "got: {}", out.text);
}

#[tokio::test]
async fn run_string_dot_unknown_prompt_errors() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine
        .run_string(".nope-not-a-prompt")
        .await
        .expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("unknown prompt"), "got: {}", out.text);
}

#[tokio::test]
async fn run_string_queue_is_always_empty_headless() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("/queue").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("empty"), "got: {}", out.text);
}

#[tokio::test]
async fn run_string_btw_usage_without_message() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);

    let out = engine.run_string("/btw").await.expect("run_string");
    assert_eq!(out.kind, RunKind::Command);
    assert!(out.text.contains("usage"), "got: {}", out.text);
    // `/btw` never touches the session.
    assert!(engine.session().messages.is_empty());
}
