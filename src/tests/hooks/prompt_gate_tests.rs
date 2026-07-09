use std::collections::HashMap;

use crate::extras::hooks::dispatcher::HookDispatcher;
use crate::extras::hooks::settings::{HookGroup, HookHandler, HooksConfig};
use crate::extras::hooks::{HookCtx, PromptGate, gate_user_prompt};

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
        "UserPromptSubmit".to_string(),
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
async fn no_hooks_configured_proceeds_unchanged() {
    let dispatcher = HookDispatcher::from_config(&HashMap::new()).unwrap();
    let gate = gate_user_prompt(&dispatcher, &ctx(), "hello".to_string()).await;
    assert!(matches!(gate, PromptGate::Proceed(p) if p == "hello"));
}

#[tokio::test]
async fn exit_zero_no_decision_proceeds_unchanged() {
    let dispatcher = dispatcher_with(vec![handler("true")]);
    let gate = gate_user_prompt(&dispatcher, &ctx(), "hello".to_string()).await;
    assert!(matches!(gate, PromptGate::Proceed(p) if p == "hello"));
}

#[tokio::test]
async fn decision_block_blocks_the_prompt() {
    let dispatcher = dispatcher_with(vec![handler(
        r#"echo '{"decision":"block","reason":"not now"}'"#,
    )]);
    let gate = gate_user_prompt(&dispatcher, &ctx(), "hello".to_string()).await;
    assert!(matches!(gate, PromptGate::Blocked(reason) if reason == "not now"));
}

#[tokio::test]
async fn additional_context_is_prepended_to_the_prompt() {
    let dispatcher = dispatcher_with(vec![handler(
        r#"echo '{"additionalContext":"extra info"}'"#,
    )]);
    let gate = gate_user_prompt(&dispatcher, &ctx(), "hello".to_string()).await;
    match gate {
        PromptGate::Proceed(p) => assert_eq!(p, "extra info\n\nhello"),
        PromptGate::Blocked(_) => panic!("expected Proceed"),
    }
}
