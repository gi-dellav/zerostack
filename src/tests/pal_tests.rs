//! PAL workflow tests: line classification/parsing (pure) plus headless
//! execution through `Engine::run_string`.
//!
//! Execution tests use the same scripted `AnyAgent::Mock` harness as
//! `engine_tests.rs`: each agent message in the script consumes one scripted
//! turn, `/` and `!` lines run without touching the model.

#![allow(clippy::await_holding_lock)]

use std::collections::HashMap;

use crate::cli::Cli;
use crate::config::Config;
use crate::engine::Engine;
use crate::engine::pal::{
    PalLineKind, classify_line, is_executable, is_nested_pal, parse_content, resolve_script_path,
};
use crate::engine::sink::{EventSink, StringSink};
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::tests::fake_model::{self, FakeModel};

fn isolate_data_dirs() {
    let dir = std::env::temp_dir().join(format!(
        "zerostack-pal-tests-{}-{}",
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
    Session::new("anthropic", "claude-sonnet-4-5", 200_000, "pal-test")
}

fn test_context() -> crate::context::ContextFiles {
    crate::context::load_with_prompts_dirs(true, &[])
}

fn engine_with_turns(turns: Vec<Vec<&str>>) -> (Engine, FakeModel) {
    isolate_data_dirs();
    let model = fake_model::text_turns(turns);
    let agent = AnyAgent::Mock(rig::agent::AgentBuilder::new(model.clone()).build());
    let engine = Engine::with_agent(
        test_cli(),
        Config::default(),
        test_session(),
        test_context(),
        test_client(),
        None,
        Sandbox::new(false, "bwrap"),
        agent,
    );
    (engine, model)
}

// ── classification ─────────────────────────────────────────────────────────

#[test]
fn pal_skips_empty_lines() {
    assert_eq!(classify_line(""), PalLineKind::Empty);
    assert_eq!(classify_line("   "), PalLineKind::Empty);
    assert_eq!(classify_line("\t "), PalLineKind::Empty);
    assert!(!is_executable(PalLineKind::Empty));
}

#[test]
fn pal_skips_comments() {
    assert_eq!(classify_line("# hello"), PalLineKind::Comment);
    assert_eq!(classify_line("  # indented"), PalLineKind::Comment);
    // A comment wins over any other prefix.
    assert_eq!(classify_line("#/help"), PalLineKind::Comment);
    assert_eq!(classify_line("#!echo"), PalLineKind::Comment);
    assert!(!is_executable(PalLineKind::Comment));
}

#[test]
fn pal_slash_goes_to_engine() {
    assert_eq!(classify_line("/help"), PalLineKind::Slash);
    assert_eq!(classify_line("  /model foo"), PalLineKind::Slash);
    assert!(is_executable(PalLineKind::Slash));
}

#[test]
fn pal_bang_goes_to_engine() {
    assert_eq!(classify_line("!echo hi"), PalLineKind::Shell);
    assert_eq!(classify_line("  !ls"), PalLineKind::Shell);
    assert!(is_executable(PalLineKind::Shell));
}

#[test]
fn pal_other_lines_are_messages() {
    assert_eq!(classify_line("fix the bug"), PalLineKind::Message);
    assert_eq!(classify_line(".ask hi"), PalLineKind::Message);
    assert!(is_executable(PalLineKind::Message));
}

#[test]
fn pal_parse_skips_comments_and_blanks_with_linenos() {
    let entries = parse_content("# top\n\n  \n/hello\n!echo hi\nfix it\n");
    let texts: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, vec!["/hello", "!echo hi", "fix it"]);
    assert_eq!(entries[0].lineno, 4);
    assert_eq!(entries[0].kind, PalLineKind::Slash);
    assert_eq!(entries[2].lineno, 6);
    assert_eq!(entries[2].kind, PalLineKind::Message);
}

#[test]
fn pal_nested_detection() {
    assert!(is_nested_pal("/pal foo.pal"));
    assert!(is_nested_pal("/pal"));
    assert!(is_nested_pal("  /pal x"));
    assert!(!is_nested_pal("/help"));
    assert!(!is_nested_pal("run /pal later"));
}

#[test]
fn pal_resolve_script_path_absolute_and_relative() {
    let abs = if cfg!(windows) {
        std::path::PathBuf::from("C:\\tmp\\x.pal")
    } else {
        std::path::PathBuf::from("/tmp/x.pal")
    };
    assert_eq!(resolve_script_path(abs.to_str().unwrap()), abs);
    let rel = resolve_script_path("sub/dir.pal");
    assert!(rel.ends_with("sub/dir.pal"));
    assert!(rel.is_absolute());
}

// ── headless execution (Engine::run_string as the core) ────────────────────

#[tokio::test]
async fn pal_content_runs_messages_slash_and_shell() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, model) = engine_with_turns(vec![vec!["got it"]]);
    let mut sink = StringSink::new();

    let (done, total) = engine
        .run_pal_content("# c\n\nfirst question\n/help\n!echo hi\n", &mut sink)
        .await;
    assert_eq!((done, total), (3, 3));

    // Only the plain message reached the model; `/help` and `!echo` ran
    // through run_string without an agent turn.
    assert_eq!(model.requests().len(), 1);
    let messages = &engine.session().messages;
    // message turn (2) + shell turn (2); `/help` mutates nothing.
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].content.as_str(), "first question");
    assert_eq!(messages[1].content.as_str(), "got it");
    assert_eq!(messages[2].content.as_str(), "!echo hi");
    assert_eq!(messages[3].content.as_str(), "hi");

    let transcript = sink.transcript();
    assert!(transcript.contains("got it"), "got: {transcript}");
    assert!(transcript.contains("hi"), "got: {transcript}");
}

#[tokio::test]
async fn pal_content_continues_through_errors() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![vec!["ok"]]);
    let mut sink = StringSink::new();

    // `/nope` errors (unknown command) but the script carries on.
    let (done, total) = engine
        .run_pal_content("/nope-not-a-command\nsecond\n", &mut sink)
        .await;
    assert_eq!((done, total), (2, 2));
    let transcript = sink.transcript();
    assert!(
        transcript.contains("unknown command"),
        "error reported: {transcript}"
    );
    assert!(transcript.contains("ok"), "later step ran: {transcript}");
    assert_eq!(
        engine.session().messages.last().map(|m| m.role),
        Some(MessageRole::Assistant)
    );
}

#[tokio::test]
async fn pal_content_rejects_nested_pal() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, model) = engine_with_turns(vec![vec!["ok"]]);
    let mut sink = StringSink::new();

    let (done, total) = engine
        .run_pal_content("hello\n/pal other.pal\n", &mut sink)
        .await;
    assert_eq!((done, total), (1, 2));
    assert_eq!(model.requests().len(), 1);
    assert!(
        sink.transcript().contains("nested /pal"),
        "got: {}",
        sink.transcript()
    );
}

#[tokio::test]
async fn pal_file_missing_is_reported() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);
    let mut sink = StringSink::new();

    let path = std::env::temp_dir().join(format!(
        "zerostack-pal-missing-{}-{}.pal",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    ));
    let _ = std::fs::remove_file(&path);
    let (done, total) = engine.run_pal_file(&path, &mut sink).await;
    assert_eq!((done, total), (0, 0));
    assert!(
        sink.transcript().contains("cannot read"),
        "got: {}",
        sink.transcript()
    );
    assert!(engine.session().messages.is_empty());
}

#[tokio::test]
async fn pal_slash_dispatch_runs_script() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, model) = engine_with_turns(vec![vec!["reply"]]);
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "zerostack-pal-script-{}-{}.pal",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    ));
    std::fs::write(&path, "# demo\nask me\n!echo yo\n").unwrap();

    let out = engine
        .run_string(&format!("/pal {}", path.display()))
        .await
        .expect("run_string");
    assert!(
        out.text.contains("1/2") || out.text.contains("2/2"),
        "got: {}",
        out.text
    );
    assert_eq!(model.requests().len(), 1);
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn pal_slash_without_arg_shows_usage() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (mut engine, _model) = engine_with_turns(vec![]);
    let out = engine.run_string("/pal").await.expect("run_string");
    assert!(out.text.contains("usage"), "got: {}", out.text);
}

// StringSink keeps the pal transcript contract: one entry per visual line.
#[test]
fn pal_sink_write_line_contract() {
    let mut sink = StringSink::new();
    sink.write_line("a\nb");
    assert_eq!(sink.lines(), &["a".to_string(), "b".to_string()]);
}

// ── TUI arming (pure planning, no renderer needed) ──────────────────────────

#[test]
fn pal_plan_content_queues_steps() {
    let steps =
        crate::ui::slash::pal::plan_pal_content("# c\n\nhi\n/help\n!echo x\n").expect("plan");
    assert_eq!(steps, vec!["hi", "/help", "!echo x"]);
}

#[test]
fn pal_plan_content_rejects_empty_and_nested() {
    assert!(
        crate::ui::slash::pal::plan_pal_content("# only\n\n")
            .unwrap_err()
            .contains("no executable steps")
    );
    assert!(
        crate::ui::slash::pal::plan_pal_content("hi\n/pal other.pal\n")
            .unwrap_err()
            .contains("nested /pal")
    );
}
