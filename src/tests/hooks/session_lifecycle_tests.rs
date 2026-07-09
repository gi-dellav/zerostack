use std::collections::HashMap;
use std::sync::Arc;

use crate::extras::hooks::dispatcher::HookDispatcher;
use crate::extras::hooks::settings::{HookGroup, HookHandler, HooksConfig};
use crate::extras::hooks::{HookCtx, dispatch_session_end_with, dispatch_session_start_with};

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

fn dispatcher_with(event: &str, handlers: Vec<HookHandler>) -> Arc<HookDispatcher> {
    let mut config: HooksConfig = HashMap::new();
    config.insert(
        event.to_string(),
        vec![HookGroup {
            matcher: None,
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
async fn session_start_runs_the_matching_hook_with_source() {
    let marker = std::env::temp_dir().join(format!(
        "zerostack-hooks-session-start-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let cmd = format!("cat > {}", marker.display());
    let dispatcher = dispatcher_with("SessionStart", vec![handler(&cmd)]);

    dispatch_session_start_with(&dispatcher, &ctx(), "resume").await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let contents = std::fs::read_to_string(&marker).unwrap();
    assert!(contents.contains("\"source\":\"resume\""));
    assert!(contents.contains("\"hook_event_name\":\"SessionStart\""));
}

#[tokio::test]
async fn session_end_runs_the_matching_hook_with_reason() {
    let marker = std::env::temp_dir().join(format!(
        "zerostack-hooks-session-end-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let cmd = format!("cat > {}", marker.display());
    let dispatcher = dispatcher_with("SessionEnd", vec![handler(&cmd)]);

    dispatch_session_end_with(&dispatcher, &ctx(), "clear").await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let contents = std::fs::read_to_string(&marker).unwrap();
    assert!(contents.contains("\"reason\":\"clear\""));
}

#[tokio::test]
async fn no_matching_hook_does_not_panic_or_block() {
    let dispatcher = Arc::new(HookDispatcher::from_config(&HashMap::new()).unwrap());
    dispatch_session_start_with(&dispatcher, &ctx(), "startup").await;
    dispatch_session_end_with(&dispatcher, &ctx(), "exit").await;
}
