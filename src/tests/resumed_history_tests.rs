//! e2e proof that a resumed session's prior messages reach the model.
//!
//! Exercises the `-p --continue` code path at the boundary `run_print` and
//! the fake model carrier share: a `Session` seeded with prior turns is
//! converted via `convert_history` (the same conversion `dispatch_print`
//! uses) and threaded through `run_print`, then the carrier's captured
//! request history is asserted to match. A hooks-continuation variant (4.2)
//! extends this to prove the resumed history survives a `Stop`-forced retry.

use rig::agent::AgentBuilder;

use crate::agent::runner::{convert_history, run_print};
use crate::retry::RetryConfig;
use crate::session::{MessageRole, Session};
#[cfg(feature = "hooks")]
use crate::tests::fake_model::text_turns;
use crate::tests::fake_model::{history_at, text_chunks};

fn resumed_session() -> Session {
    let mut session = Session::new("anthropic", "claude-test", 200_000, "");
    session.add_message(MessageRole::User, "what's the plan");
    session.add_message(MessageRole::Assistant, "ship section 3");
    session
}

#[tokio::test]
async fn resumed_session_history_reaches_model_initial_turn() {
    let session = resumed_session();
    let expected_history = convert_history(&session);

    let model = text_chunks(["got it"]);
    let agent = AgentBuilder::new(model.clone()).build();

    // `run_print` reaches process-global wiring (the hooks Stop dispatcher,
    // the subagent event sender); serialize against every other `run_print`
    // test so none of them clobber each other's.
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    // Mirrors `dispatch_print`'s own `convert_history(&self.session)` call:
    // this is the `-p --continue` code path being exercised end to end.
    let _outcome = run_print(
        &agent,
        "continue",
        false,
        &RetryConfig::default(),
        expected_history.clone(),
        #[cfg(feature = "hooks")]
        None,
    )
    .await
    .expect("run_print should succeed against the fake model");

    let observed_history = history_at(&model, 0);
    assert_eq!(
        observed_history, expected_history,
        "run_print must forward the resumed session's prior messages to the \
         model as history on the initial stream_chat call"
    );
}

// 2.7: now that headless sessions can contain tool call/result messages
// (this section's addition), `-p --continue` must replay them through the
// same `convert_history` interactive resume already uses. `convert_history`
// itself is untouched (design.md decision 7, "replay alignment is accepted,
// not worked around"); this proves the existing `[ToolCall]:`/`[ToolResult]:`
// tagging actually reaches the model at the `run_print` boundary once a
// resumed session has tool messages to replay, not just in isolation.
#[tokio::test]
async fn resumed_session_tool_messages_replay_as_tagged_history_entries() {
    let mut session = resumed_session();
    let call_id = session.add_tool_call("read", &serde_json::json!({ "path": "src/main.rs" }));
    session.add_tool_result(call_id, "read", "file contents");
    let expected_history = convert_history(&session);

    let model = text_chunks(["got it"]);
    let agent = AgentBuilder::new(model.clone()).build();

    // `run_print` reaches process-global wiring (the hooks Stop dispatcher,
    // the subagent event sender); serialize against every other `run_print`
    // test so none of them clobber each other's.
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    let _outcome = run_print(
        &agent,
        "continue",
        false,
        &RetryConfig::default(),
        expected_history.clone(),
        #[cfg(feature = "hooks")]
        None,
    )
    .await
    .expect("run_print should succeed against the fake model");

    let observed_history = history_at(&model, 0);
    assert_eq!(
        observed_history, expected_history,
        "run_print must forward the resumed session's tool messages to the \
         model as history, exactly as it does for plain user/assistant turns"
    );

    let rendered = format!("{observed_history:?}");
    assert!(
        rendered.contains("[ToolCall]:"),
        "resumed history must carry a [ToolCall] entry for the replayed tool \
         call, rendered: {rendered}"
    );
    assert!(
        rendered.contains("[ToolResult]:"),
        "resumed history must carry a [ToolResult] entry for the replayed \
         tool result, rendered: {rendered}"
    );
}

// 4.2: a `Stop` hook forcing a continuation must still see the resumed
// history. `run_print`'s `Stop`-continuation path (`src/agent/runner.rs`,
// ~line 692) calls the production `dispatch_stop`, which reads the
// process-global hook dispatcher; this test installs one so the real wiring
// (not a test-only substitute) is exercised end to end.
#[cfg(feature = "hooks")]
#[tokio::test]
async fn resumed_session_history_survives_stop_hook_continuation() {
    use std::collections::HashMap;

    use crate::extras::hooks::dispatcher::HookDispatcher;
    use crate::extras::hooks::init_dispatcher;
    use crate::extras::hooks::settings::{HookGroup, HookHandler, HooksConfig};

    // Installs a process-global Stop hook below; the guard serializes against
    // other `run_print` tests and clears the dispatcher when it drops, so the
    // hook can't leak into them.
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    let session = resumed_session();
    let expected_history = convert_history(&session);

    // Script two turns: the first ends with the `Stop` hook below forcing a
    // continuation (guarding the `retry_history` seed wired in 3.2); the
    // second is the real final answer released after the hook stands down.
    let model = text_turns([["partial answer"], ["final answer"]]);
    let agent = AgentBuilder::new(model.clone()).build();

    // Blocks exactly once: the envelope's `stop_hook_active` is `false` on
    // the first `Stop` dispatch, so the hook forces a continuation; on the
    // retry it is `true`, the condition misses, and the hook emits no
    // decision, releasing the final response.
    let handler = HookHandler {
        kind: "command".to_string(),
        command: Some(
            r#"if grep -q '"stop_hook_active":false'; then echo '{"decision":"block","reason":"resume once more"}'; fi"#
                .to_string(),
        ),
        args: None,
        timeout: Some(5),
        is_async: false,
        condition: None,
        once: false,
    };
    let mut config: HooksConfig = HashMap::new();
    config.insert(
        "Stop".to_string(),
        vec![HookGroup {
            matcher: None,
            hooks: vec![handler],
        }],
    );
    init_dispatcher(HookDispatcher::from_config(&config).unwrap());

    let outcome = run_print(
        &agent,
        "continue",
        false,
        &RetryConfig::default(),
        expected_history.clone(),
        None,
    )
    .await
    .expect("run_print should succeed against the fake model");

    assert_eq!(
        outcome.response, "partial answerfinal answer",
        "the Stop-forced continuation must run to completion and the returned \
         response must keep both turns' text (the first turn was already \
         streamed to stdout), not just the second turn's"
    );

    // Guards the `retry_history` seed (runner.rs ~601, consumed by the
    // `Stop`-continuation retry ~727): the resumed session's prior messages
    // must still be present in the history the SECOND request received.
    let second_request_history = history_at(&model, 1);
    assert!(
        second_request_history.starts_with(&expected_history),
        "the Stop-hook continuation retry must still carry the resumed \
         session's prior messages as history, not just the first turn's \
         tool interactions"
    );
}
