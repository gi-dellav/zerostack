use std::collections::HashMap;

use crate::extras::hooks::dispatcher::HookDispatcher;
use crate::extras::hooks::settings::{HookGroup, HookHandler, HooksConfig};
use crate::extras::hooks::{HookCtx, hooks_test_dry_run_with};

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

fn dispatcher_with(handlers: Vec<HookHandler>) -> HookDispatcher {
    let mut config: HooksConfig = HashMap::new();
    config.insert(
        "PreToolUse".to_string(),
        vec![HookGroup {
            matcher: None,
            hooks: handlers,
        }],
    );
    HookDispatcher::from_config(&config).unwrap()
}

fn ctx() -> HookCtx {
    HookCtx {
        session_id: "sess".into(),
        session_path: "".into(),
        cwd: "/repo".into(),
        permission_mode: "standard".into(),
    }
}

#[tokio::test]
async fn report_shows_defer_when_no_hook_matches() {
    let dispatcher = HookDispatcher::from_config(&HashMap::new()).unwrap();
    let report = hooks_test_dry_run_with(&dispatcher, &ctx(), "bash", serde_json::json!({})).await;
    assert!(report.contains("Defer"));
}

#[tokio::test]
async fn report_shows_deny_and_reason_on_block() {
    let dispatcher = dispatcher_with(vec![handler("echo 'no pushes allowed' 1>&2; exit 2")]);
    let report = hooks_test_dry_run_with(
        &dispatcher,
        &ctx(),
        "bash",
        serde_json::json!({"command": "git push"}),
    )
    .await;
    assert!(report.contains("Deny"));
    assert!(report.contains("no pushes allowed"));
}

#[tokio::test]
async fn report_shows_updated_input_when_present() {
    let dispatcher = dispatcher_with(vec![handler(
        r#"echo '{"updatedInput":{"command":"rewritten"}}'"#,
    )]);
    let report = hooks_test_dry_run_with(
        &dispatcher,
        &ctx(),
        "bash",
        serde_json::json!({"command": "orig"}),
    )
    .await;
    assert!(report.contains("rewritten"));
}
