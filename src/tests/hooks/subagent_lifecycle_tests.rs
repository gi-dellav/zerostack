use std::collections::HashMap;
use std::sync::Arc;

use crate::extras::hooks::dispatcher::HookDispatcher;
use crate::extras::hooks::settings::{HookGroup, HookHandler, HooksConfig};
use crate::extras::hooks::{HookCtx, SubagentStopGate, gate_subagent_start, gate_subagent_stop};

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

fn dispatcher_with(
    event: &str,
    matcher: Option<&str>,
    handlers: Vec<HookHandler>,
) -> Arc<HookDispatcher> {
    let mut config: HooksConfig = HashMap::new();
    config.insert(
        event.to_string(),
        vec![HookGroup {
            matcher: matcher.map(str::to_string),
            hooks: handlers,
        }],
    );
    Arc::new(HookDispatcher::from_config(&config).unwrap())
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
async fn subagent_start_with_no_hooks_returns_no_context() {
    let dispatcher = Arc::new(HookDispatcher::from_config(&HashMap::new()).unwrap());
    let extra = gate_subagent_start(&dispatcher, &ctx(), "explore").await;
    assert!(extra.is_none());
}

#[tokio::test]
async fn subagent_start_matches_cc_style_agent_type_name() {
    let dispatcher = dispatcher_with(
        "SubagentStart",
        Some("Explore"),
        vec![handler(
            r#"echo '{"additionalContext":"extra background"}'"#,
        )],
    );
    let extra = gate_subagent_start(&dispatcher, &ctx(), "explore").await;
    assert_eq!(extra.as_deref(), Some("extra background"));
}

#[tokio::test]
async fn subagent_start_no_decision_returns_no_context() {
    let dispatcher = dispatcher_with("SubagentStart", None, vec![handler("true")]);
    let extra = gate_subagent_start(&dispatcher, &ctx(), "explore").await;
    assert!(extra.is_none());
}

#[tokio::test]
async fn subagent_stop_no_hooks_releases() {
    let dispatcher = Arc::new(HookDispatcher::from_config(&HashMap::new()).unwrap());
    let gate = gate_subagent_stop(&dispatcher, &ctx(), "explore", false).await;
    assert!(matches!(gate, SubagentStopGate::Release));
}

#[tokio::test]
async fn subagent_stop_block_forces_continuation_with_reason() {
    let dispatcher = dispatcher_with(
        "SubagentStop",
        None,
        vec![handler(
            r#"echo '{"decision":"block","reason":"keep digging"}'"#,
        )],
    );
    let gate = gate_subagent_stop(&dispatcher, &ctx(), "explore", false).await;
    match gate {
        SubagentStopGate::Continue { reason } => assert_eq!(reason, "keep digging"),
        SubagentStopGate::Release => panic!("expected Continue"),
    }
}
