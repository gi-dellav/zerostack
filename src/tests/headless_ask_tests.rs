//! e2e proof that a headless (`-p`, non-interactive) run whose permission
//! checker resolves `Ask` fails closed instead of hanging.
//!
//! Before this section's fix, `dispatch_print`/`dispatch_loop` handed the
//! live `ask_tx` sender to the agent even in non-interactive mode, so a tool
//! whose permission check resolves `Ask` would send an `AskRequest` down a
//! channel nobody drains and then block forever awaiting a reply that never
//! comes (`src/agent/tools/mod.rs::handle_ask_inner`). This test scripts that
//! exact shape at the `run_print` boundary: a `WriteTool` wired with a
//! `Guarded`-mode checker (which asks for an external-path write) and a live
//! ask-channel sender whose receiver is kept alive but never polled, mirroring
//! the pre-fix headless dispatch. Section 6.2 makes the production fix
//! (non-interactive dispatch hands the agent `None` for the ask sender); this
//! test exercises that fixed behavior by passing `None`, exactly as production
//! now does for non-interactive runs.
//!
//! Rig's multi-turn agent loop treats a tool's `Err` as ordinary tool-result
//! content fed back to the model for the next turn (not a terminating
//! error), so proving "denied, not hung" at this boundary means driving the
//! agent through a second turn and inspecting what the model was actually
//! shown, rather than asserting `run_print` itself returns `Err`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rig::agent::AgentBuilder;
use rig::completion::Message;
use rig::message::{ToolResultContent, UserContent};

use crate::agent::runner::run_print;
use crate::agent::tools::WriteTool;
use crate::permission::checker::PermissionChecker;
use crate::permission::{PermissionConfigs, SecurityMode};
use crate::retry::RetryConfig;
use crate::tests::fake_model::{MockCompletionModel, MockStreamEvent};

fn guarded_checker() -> PermissionChecker {
    PermissionChecker::new(
        &PermissionConfigs::default(),
        SecurityMode::Guarded,
        Some(std::path::PathBuf::from("/home/user/project")),
        Some(vec![
            "guarded".to_string(),
            "standard".to_string(),
            "yolo".to_string(),
        ]),
    )
}

// The model calls `write` on turn 0, then (given the tool's denial fed back
// as ordinary tool-result content) produces a plain text reply on turn 1.
// `default_max_turns(2)` is needed because rig's implicit per-agent budget is
// one model call total; without it, driving a second turn hits
// `PromptError::MaxTurnsError` regardless of how the tool call resolved,
// which would prove nothing about the ask-channel fix.
fn write_tool_call_model() -> MockCompletionModel {
    MockCompletionModel::from_stream_turns(vec![
        vec![
            MockStreamEvent::tool_call(
                "call-1",
                "write",
                serde_json::json!({"path": "/etc/passwd", "content": "x"}),
            ),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("acknowledged".to_string()),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ])
}

// 6.1/6.3: before the 6.2 fix, `dispatch_print`/`dispatch_loop` handed the
// live `ask_tx` sender straight to the agent regardless of interactivity
// (`build_permission_checker` populates it for any run that has a
// permission checker at all), so a `-p` run whose checker resolves `Ask`
// would hang forever awaiting a reply nobody sends. The fix makes headless
// dispatch hand the agent `None` for the ask sender; this test passes `None`,
// exactly as production now does for a non-interactive (`-p`) run.
#[tokio::test]
async fn headless_ask_denies_instead_of_hanging() {
    // Under `--features hooks`, `run_print` consults the process-global Stop
    // dispatcher; serialize against the test that installs one. No-op otherwise.
    #[cfg(feature = "hooks")]
    let _dispatcher_guard = crate::tests::fake_model::dispatcher_guard::acquire();

    let model = write_tool_call_model();
    let permission = Some(Arc::new(Mutex::new(guarded_checker())));

    // Production hands non-interactive dispatch `None` for the ask channel
    // (see `dispatch_print`/`dispatch_loop`); with no sender, an `Ask` verdict
    // must fail closed instead of hanging on a channel nobody drains.
    let ask_tx: Option<crate::permission::ask::AskSender> = None;
    let write_tool = WriteTool::new(permission, ask_tx, None);

    let agent = AgentBuilder::new(model.clone())
        .tool(write_tool)
        .default_max_turns(2)
        .build();

    tokio::time::timeout(
        Duration::from_secs(2),
        run_print(
            &agent,
            "please write to /etc/passwd",
            false,
            &RetryConfig::default(),
            Vec::new(),
            #[cfg(feature = "hooks")]
            None,
        ),
    )
    .await
    .expect("run_print must complete within the timeout instead of hanging on an unserviced ask channel")
    .expect("run_print should complete once the tool's denial is fed back to the model as an ordinary turn");

    // The second call's chat history is what the model was actually shown
    // after the tool ran; its trailing message (the "prompt" for this turn,
    // per rig's `CompletionRequest` invariant) is the tool's own result, and
    // it must carry the non-interactive denial, proving the tool failed
    // closed rather than silently succeeding or hanging.
    let second_call_history: Vec<Message> = model.requests()[1]
        .chat_history
        .clone()
        .into_iter()
        .collect();
    let saw_denial = second_call_history.iter().any(|message| match message {
        Message::User { content } => content.iter().any(|item| match item {
            UserContent::ToolResult(tool_result) => {
                tool_result.content.iter().any(|part| match part {
                    ToolResultContent::Text(text) => text
                        .text
                        .contains("Permission denied (non-interactive mode)"),
                    _ => false,
                })
            }
            _ => false,
        }),
        _ => false,
    });
    assert!(
        saw_denial,
        "expected the tool result fed back to the model on the second turn to \
         contain the non-interactive denial message, history: {second_call_history:?}"
    );
}
