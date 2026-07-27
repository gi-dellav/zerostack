//! E2e proof that a headless (`-p`) run persists structured tool call/result
//! records into the session, independent of `--pure-stdout`, and that a
//! turn ending in a mid-stream error persists nothing.
//!
//! Exercises the same `run_print` boundary as `resumed_history_tests.rs`
//! (see that file's header for why the fake `CompletionModel` carrier is
//! driven directly rather than through the full `dispatch_print` CLI
//! plumbing) via [`record_turn_into_session`], which mirrors
//! `dispatch_print`'s own recording sequence exactly: `?` on the
//! `run_print` result (so a mid-stream `Err` never reaches the
//! session-mutating code at all), then `add_message(User)`, one
//! `add_tool_call`/`add_tool_result` pair per returned `ToolInteraction`,
//! then `add_message(Assistant)`.

use rig::agent::AgentBuilder;
use rig::tool::Tool;
use serde::Deserialize;

use crate::agent::runner::{PrintOutcome, run_print};
use crate::agent::tools::ToolError;
use crate::retry::RetryConfig;
use crate::session::{MessageRole, Session, ToolRecord};
use crate::tests::fake_model::{FakeModel, MockCompletionModel, MockStreamEvent};

#[derive(Debug, Deserialize)]
struct EchoArgs {
    text: String,
}

/// Minimal test-only tool: echoes its `text` argument back with a fixed
/// prefix. Unlike `WriteTool` (used by `headless_ask_tests.rs`), it needs no
/// permission/ask-channel plumbing: these tests only need a real call/result
/// round trip through rig's own tool-execution machinery, since the mock
/// model can script a `ToolCall` event but not the `ToolResult` that follows
/// it — that comes from an actually registered `Tool` being invoked.
struct EchoTool;

impl Tool for EchoTool {
    const NAME: &'static str = "echo";

    type Error = ToolError;
    type Args = EchoArgs;
    type Output = String;

    fn description(&self) -> String {
        "Echoes the given text back.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        })
    }

    async fn call(&self, args: EchoArgs) -> Result<String, ToolError> {
        Ok(format!("echoed: {}", args.text))
    }
}

/// Longer than `format_tool_call_summary`'s 200-char display truncation
/// (`src/ui/utils.rs`), so a passing assertion on the structured `args`
/// field proves it carries the complete value, not the truncated display
/// summary that lands in `content`.
fn long_arg_text() -> String {
    "x".repeat(500)
}

fn echo_call_model() -> FakeModel {
    MockCompletionModel::from_stream_turns(vec![
        vec![
            MockStreamEvent::tool_call(
                "call-1",
                "echo",
                serde_json::json!({ "text": long_arg_text() }),
            ),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("done".to_string()),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ])
}

fn echo_then_error_model() -> FakeModel {
    MockCompletionModel::from_stream_turns(vec![
        vec![
            MockStreamEvent::tool_call("call-1", "echo", serde_json::json!({ "text": "x" })),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![MockStreamEvent::error("stream broke mid-turn")],
    ])
}

/// Mirrors `dispatch_print`'s recording sequence (`src/startup.rs`) exactly,
/// including the `?` that makes a mid-stream `Err` short-circuit before any
/// session mutation.
fn record_turn_into_session(
    session: &mut Session,
    prompt: &str,
    result: anyhow::Result<PrintOutcome>,
) -> anyhow::Result<()> {
    let outcome = result?;
    session.add_message(MessageRole::User, prompt);
    for interaction in &outcome.tool_interactions {
        let call_id = session.add_tool_call(&interaction.name, &interaction.args);
        session.add_tool_result(call_id, &interaction.name, &interaction.output);
    }
    session.add_message(MessageRole::Assistant, &outcome.response);
    Ok(())
}

#[tokio::test]
async fn headless_tool_call_recorded_with_full_args_independent_of_pure_stdout() {
    // `run_print` reaches process-global wiring (the hooks Stop dispatcher,
    // the subagent event sender); serialize against every other `run_print`
    // test so none of them clobber each other's.
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    let model = echo_call_model();
    let agent = AgentBuilder::new(model)
        .tool(EchoTool)
        .default_max_turns(2)
        .build();

    // `pure_stdout: false` — recording must not depend on stdout formatting
    // being enabled (spec scenario "Headless run without --pure-stdout still
    // records").
    let outcome = run_print(
        &agent,
        "please echo",
        false,
        &RetryConfig::default(),
        Vec::new(),
        #[cfg(feature = "hooks")]
        None,
    )
    .await
    .expect("run_print should succeed against the fake model");

    assert_eq!(outcome.tool_interactions.len(), 1);
    {
        let interaction = &outcome.tool_interactions[0];
        assert_eq!(interaction.name, "echo");
        assert_eq!(
            interaction.args,
            serde_json::json!({ "text": long_arg_text() })
        );
        assert_eq!(interaction.output, format!("echoed: {}", long_arg_text()));
    }

    let mut session = Session::new("anthropic", "claude-test", 200_000, "");
    record_turn_into_session(&mut session, "please echo", Ok(outcome))
        .expect("recording a successful turn must not fail");

    assert_eq!(session.messages.len(), 4);
    assert_eq!(session.messages[0].role, MessageRole::User);
    assert_eq!(session.messages[1].role, MessageRole::ToolCall);
    assert_eq!(session.messages[2].role, MessageRole::ToolResult);
    assert_eq!(session.messages[3].role, MessageRole::Assistant);

    match &session.messages[1].tool {
        Some(ToolRecord::Call { name, args, .. }) => {
            assert_eq!(name.as_str(), "echo");
            let expected_text = long_arg_text();
            assert_eq!(args, &serde_json::json!({ "text": expected_text }));
            // The display summary in `content` is truncated at 200 chars
            // (`format_tool_call_summary`); the structured field must not be.
            assert!(
                session.messages[1].content.len() < expected_text.len(),
                "content should be the truncated display summary, not the full args"
            );
        }
        other => panic!("expected a Call record, got {other:?}"),
    }

    match &session.messages[2].tool {
        Some(ToolRecord::Result {
            call_id,
            name,
            truncated,
            ..
        }) => {
            assert_eq!(*call_id, 0);
            assert_eq!(name.as_str(), "echo");
            assert!(!truncated);
        }
        other => panic!("expected a Result record, got {other:?}"),
    }
}

#[tokio::test]
async fn headless_mid_stream_error_after_tool_call_records_nothing() {
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    let model = echo_then_error_model();
    let agent = AgentBuilder::new(model)
        .tool(EchoTool)
        .default_max_turns(2)
        .build();

    let response_result = run_print(
        &agent,
        "please echo",
        false,
        &RetryConfig::default(),
        Vec::new(),
        #[cfg(feature = "hooks")]
        None,
    )
    .await;

    assert!(
        response_result.is_err(),
        "a mid-stream error after a completed tool call must surface as Err, \
         not a partial Ok that would let a truncated turn get persisted"
    );

    let mut session = Session::new("anthropic", "claude-test", 200_000, "");
    let record_result = record_turn_into_session(&mut session, "please echo", response_result);

    assert!(record_result.is_err());
    assert_eq!(
        session.messages.len(),
        0,
        "a turn that fails mid-stream must not add any messages, including \
         the tool call that already completed before the failure"
    );
}
