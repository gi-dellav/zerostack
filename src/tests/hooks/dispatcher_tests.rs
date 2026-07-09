use std::collections::HashMap;

use crate::extras::hooks::dispatcher::HookDispatcher;
use crate::extras::hooks::envelope::EventFields;
use crate::extras::hooks::settings::{HookGroup, HookHandler, HooksConfig};
use crate::extras::hooks::{Decision, HookCtx, Verdict};

fn handler(command: &str) -> HookHandler {
    HookHandler {
        kind: "command".to_string(),
        command: Some(command.to_string()),
        args: None,
        timeout: Some(5),
        is_async: false,
        condition: None,
        once: false,
    }
}

fn handler_with_condition(command: &str, condition: &str) -> HookHandler {
    HookHandler {
        condition: Some(condition.to_string()),
        ..handler(command)
    }
}

fn handler_once(command: &str) -> HookHandler {
    HookHandler {
        once: true,
        ..handler(command)
    }
}

fn ctx() -> HookCtx {
    HookCtx {
        session_id: "sess-1".into(),
        session_path: "/tmp/sess.json".into(),
        cwd: "/repo".into(),
        permission_mode: "default".into(),
    }
}

fn config_with(event: &str, matcher: Option<&str>, handlers: Vec<HookHandler>) -> HooksConfig {
    let mut config: HooksConfig = HashMap::new();
    config.insert(
        event.to_string(),
        vec![HookGroup {
            matcher: matcher.map(str::to_string),
            hooks: handlers,
        }],
    );
    config
}

#[test]
fn invalid_regex_matcher_fails_at_load_time() {
    let config = config_with("PreToolUse", Some("(unclosed"), vec![handler("true")]);
    assert!(HookDispatcher::from_config(&config).is_err());
}

#[test]
fn wildcard_matcher_matches_every_tool() {
    let config = config_with("PreToolUse", None, vec![handler("true")]);
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    assert!(!dispatcher.handlers_for("PreToolUse", "bash").is_empty());
    assert!(
        !dispatcher
            .handlers_for("PreToolUse", "anything_else")
            .is_empty()
    );
}

#[test]
fn name_list_matcher_matches_after_normalization() {
    // "Edit|Write" is CC-style names; the model calls zerostack's "write" tool.
    let config = config_with("PreToolUse", Some("Edit|Write"), vec![handler("true")]);
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    assert!(!dispatcher.handlers_for("PreToolUse", "write").is_empty());
    assert!(dispatcher.handlers_for("PreToolUse", "bash").is_empty());
}

#[test]
fn is_empty_true_when_no_events_configured() {
    let dispatcher = HookDispatcher::from_config(&HashMap::new()).unwrap();
    assert!(dispatcher.is_empty());
}

#[test]
fn is_empty_false_when_a_handler_is_configured() {
    let config = config_with("PreToolUse", None, vec![handler("true")]);
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    assert!(!dispatcher.is_empty());
}

#[test]
fn summary_is_empty_when_no_events_configured() {
    let dispatcher = HookDispatcher::from_config(&HashMap::new()).unwrap();
    assert!(dispatcher.summary().is_empty());
}

#[test]
fn summary_lists_events_with_handler_counts_sorted_by_event_name() {
    let mut config: HooksConfig = HashMap::new();
    config.insert(
        "Stop".to_string(),
        vec![HookGroup {
            matcher: None,
            hooks: vec![handler("true")],
        }],
    );
    config.insert(
        "PreToolUse".to_string(),
        vec![HookGroup {
            matcher: None,
            hooks: vec![handler("true"), handler("false")],
        }],
    );
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    assert_eq!(
        dispatcher.summary(),
        vec![("PreToolUse".to_string(), 2), ("Stop".to_string(), 1),]
    );
}

#[test]
fn identical_commands_are_deduplicated() {
    let config = config_with(
        "PreToolUse",
        None,
        vec![handler("echo dup"), handler("echo dup")],
    );
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    assert_eq!(dispatcher.handlers_for("PreToolUse", "bash").len(), 1);
}

#[tokio::test]
async fn dispatch_returns_continue_without_running_anything_when_no_handler_matches() {
    let marker = std::env::temp_dir().join(format!(
        "zerostack-hooks-dispatch-nomatch-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let cmd = format!("touch {}", marker.display());
    let config = config_with("PreToolUse", Some("Bash"), vec![handler(&cmd)]);
    let dispatcher = HookDispatcher::from_config(&config).unwrap();

    let decision = dispatcher
        .dispatch_pre_tool_use(&ctx(), "write", serde_json::json!({}))
        .await;

    assert_eq!(decision.verdict, Verdict::Defer);
    assert!(!marker.exists());
}

#[tokio::test]
async fn dispatch_pre_tool_use_defers_when_hook_exits_zero_with_no_decision() {
    let config = config_with("PreToolUse", None, vec![handler("true")]);
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    let decision = dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({"command": "ls"}))
        .await;
    assert_eq!(decision.verdict, Verdict::Defer);
    assert!(decision.updated_input.is_none());
}

#[tokio::test]
async fn dispatch_pre_tool_use_a_lone_allow_verdict_merges_as_allow() {
    // Regression: Verdict's declared/derived Ord is Allow < Defer < Ask <
    // Deny (least to most severe), so a merge seeded from a hardcoded
    // Defer sentinel would never let a lone Allow verdict win (Allow is not
    // > Defer) and would silently report Defer instead.
    let config = config_with(
        "PreToolUse",
        None,
        vec![handler(r#"echo '{"permissionDecision":"allow"}'"#)],
    );
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    let decision = dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({}))
        .await;
    assert_eq!(decision.verdict, Verdict::Allow);
}

#[tokio::test]
async fn dispatch_pre_tool_use_denies_on_exit_code_two() {
    let config = config_with(
        "PreToolUse",
        None,
        vec![handler("echo 'no way' 1>&2; exit 2")],
    );
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    let decision = dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({}))
        .await;
    assert_eq!(decision.verdict, Verdict::Deny);
    assert_eq!(decision.reason.as_deref().map(str::trim), Some("no way"));
}

#[tokio::test]
async fn dispatch_pre_tool_use_merges_most_severe_verdict() {
    let config = config_with(
        "PreToolUse",
        None,
        vec![
            handler(r#"echo '{"permissionDecision":"allow"}'"#),
            handler("exit 2"),
        ],
    );
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    let decision = dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({}))
        .await;
    assert_eq!(decision.verdict, Verdict::Deny);
}

#[tokio::test]
async fn dispatch_pre_tool_use_folds_updated_input_in_declared_order() {
    let config = config_with(
        "PreToolUse",
        None,
        vec![
            handler(r#"echo '{"updatedInput":{"command":"first"}}'"#),
            handler(r#"echo '{"updatedInput":{"command":"second"}}'"#),
        ],
    );
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    let decision = dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({"command": "orig"}))
        .await;
    assert_eq!(
        decision.updated_input,
        Some(serde_json::json!({"command": "second"}))
    );
}

#[tokio::test]
async fn dispatch_generic_returns_continue_when_no_handler_matches() {
    let dispatcher = HookDispatcher::from_config(&HashMap::new()).unwrap();
    let decision = dispatcher
        .dispatch(
            "Stop",
            None,
            &ctx(),
            EventFields::Stop {
                stop_hook_active: false,
                loop_iteration: None,
                loop_active: None,
            },
        )
        .await;
    assert_eq!(decision, Decision::Continue);
}

#[tokio::test]
async fn dispatch_generic_blocks_on_decision_block_json() {
    let config = config_with(
        "Stop",
        None,
        vec![handler(
            r#"echo '{"decision":"block","reason":"tests still failing"}'"#,
        )],
    );
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    let decision = dispatcher
        .dispatch(
            "Stop",
            None,
            &ctx(),
            EventFields::Stop {
                stop_hook_active: false,
                loop_iteration: None,
                loop_active: None,
            },
        )
        .await;
    assert_eq!(
        decision,
        Decision::Block {
            reason: "tests still failing".to_string()
        }
    );
}

#[tokio::test]
async fn dispatch_post_tool_use_failure_runs_but_cannot_change_outcome() {
    let marker = std::env::temp_dir().join(format!(
        "zerostack-hooks-posttooluse-failure-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let cmd = format!("touch {}", marker.display());
    let config = config_with("PostToolUseFailure", None, vec![handler(&cmd)]);
    let dispatcher = HookDispatcher::from_config(&config).unwrap();

    dispatcher
        .dispatch_post_tool_use_failure(&ctx(), "bash", serde_json::json!({}), "boom")
        .await;

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(marker.exists());
}

#[tokio::test]
async fn if_condition_true_runs_the_handler() {
    let config = config_with(
        "PreToolUse",
        None,
        vec![handler_with_condition(
            r#"echo '{"permissionDecision":"deny"}'"#,
            "true",
        )],
    );
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    let decision = dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({}))
        .await;
    assert_eq!(decision.verdict, Verdict::Deny);
}

#[tokio::test]
async fn if_condition_false_skips_the_handler() {
    let config = config_with(
        "PreToolUse",
        None,
        vec![handler_with_condition(
            r#"echo '{"permissionDecision":"deny"}'"#,
            "false",
        )],
    );
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    let decision = dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({}))
        .await;
    assert_eq!(decision.verdict, Verdict::Defer);
}

#[tokio::test]
async fn if_condition_broken_command_fails_closed_and_runs_anyway() {
    // A condition that hangs past its timeout counts as "cannot be
    // evaluated" per the fail-closed requirement: the handler still runs.
    let handler = HookHandler {
        timeout: Some(1),
        ..handler_with_condition(r#"echo '{"permissionDecision":"deny"}'"#, "sleep 30")
    };
    let config = config_with("PreToolUse", None, vec![handler]);
    let dispatcher = HookDispatcher::from_config(&config).unwrap();
    let decision = dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({}))
        .await;
    assert_eq!(decision.verdict, Verdict::Deny);
}

#[tokio::test]
async fn once_handler_runs_on_first_dispatch_and_is_skipped_on_second() {
    let marker = std::env::temp_dir().join(format!("zerostack-hooks-once-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let cmd = format!("printf x >> {}", marker.display());
    let config = config_with("PreToolUse", None, vec![handler_once(&cmd)]);
    let dispatcher = HookDispatcher::from_config(&config).unwrap();

    dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({}))
        .await;
    dispatcher
        .dispatch_pre_tool_use(&ctx(), "bash", serde_json::json!({}))
        .await;

    let contents = std::fs::read_to_string(&marker).unwrap_or_default();
    assert_eq!(contents, "x", "handler with once:true must not run twice");
}
