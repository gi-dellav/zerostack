use std::collections::HashMap;

use crate::extras::hooks::dispatcher::HookDispatcher;
use crate::extras::hooks::settings::{HookGroup, HookHandler, HooksConfig};
use crate::extras::hooks::{HookCtx, StopGate, gate_stop};

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
        "Stop".to_string(),
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
async fn no_hooks_configured_releases() {
    let dispatcher = HookDispatcher::from_config(&HashMap::new()).unwrap();
    let gate = gate_stop(&dispatcher, &ctx(), false, None, None).await;
    assert!(matches!(gate, StopGate::Release));
}

#[tokio::test]
async fn exit_zero_no_decision_releases() {
    let dispatcher = dispatcher_with(vec![handler("true")]);
    let gate = gate_stop(&dispatcher, &ctx(), false, None, None).await;
    assert!(matches!(gate, StopGate::Release));
}

#[tokio::test]
async fn decision_block_forces_continuation_with_reason() {
    let dispatcher = dispatcher_with(vec![handler(
        r#"echo '{"decision":"block","reason":"tests still failing"}'"#,
    )]);
    let gate = gate_stop(&dispatcher, &ctx(), false, None, None).await;
    match gate {
        StopGate::Continue { reason } => assert_eq!(reason, "tests still failing"),
        StopGate::Release => panic!("expected Continue"),
    }
}

#[tokio::test]
async fn exit_two_also_forces_continuation() {
    let dispatcher = dispatcher_with(vec![handler("echo 'not done yet' 1>&2; exit 2")]);
    let gate = gate_stop(&dispatcher, &ctx(), false, None, None).await;
    match gate {
        StopGate::Continue { reason } => assert_eq!(reason.trim(), "not done yet"),
        StopGate::Release => panic!("expected Continue"),
    }
}
