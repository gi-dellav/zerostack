//! Regression proof that a parallel tool-call batch (all `ToolCall`s
//! streamed before any of their `ToolResult`s, which is how rig 0.40 drives
//! providers' parallel tool use) is recorded with each result paired to its
//! own call. Before pairing was keyed by rig's `internal_call_id`, both
//! `run_print` and `spawn_agent` tracked the batch in a single
//! most-recent-call slot, so with N>1 calls in flight the first result was
//! recorded under the *last* call's name/args and later results under an
//! empty name — corrupting the session-JSON evidence channel.
//!
//! Uses the same fake-model + real-registered-tool setup as
//! `headless_tool_record_tests.rs` (see that file's header), with two
//! distinct tools so a mispairing changes observable names, not just args.
//!
//! The TUI keeps its own copy of that pairing in `AgentRunState`, driven by
//! the same events; the second half of this file covers that bookkeeping and
//! the unmatched-result fallback directly, since neither is reachable from the
//! fake model (rig only streams an unmatched result when a hook stack skips an
//! invalid tool call, and zerostack installs no rig hooks).

use rig::agent::AgentBuilder;
use rig::tool::Tool;
use serde::Deserialize;

use crate::agent::runner::run_print;
use crate::agent::tools::ToolError;
use crate::event::AgentEvent;
use crate::retry::RetryConfig;
use crate::session::{Session, ToolRecord};
use crate::tests::fake_model::{FakeModel, MockCompletionModel, MockStreamEvent};
use crate::ui::event_handler::resolve_tool_result_call_id;
use crate::ui::state::AgentRunState;

#[derive(Debug, Deserialize)]
struct TextArgs {
    text: String,
}

/// A tool event as `spawn_agent` emitted it, kept in arrival order so a test
/// can assert on the batch shape as well as on the pairing.
#[derive(Debug)]
enum ToolEvent {
    Call {
        id: compact_str::CompactString,
        name: String,
        args: serde_json::Value,
    },
    Result {
        id: compact_str::CompactString,
        name: String,
        output: String,
    },
}

struct EchoTool;

impl Tool for EchoTool {
    const NAME: &'static str = "echo";

    type Error = ToolError;
    type Args = TextArgs;
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

    async fn call(&self, args: TextArgs) -> Result<String, ToolError> {
        Ok(format!("echoed: {}", args.text))
    }
}

struct ReverseTool;

impl Tool for ReverseTool {
    const NAME: &'static str = "reverse";

    type Error = ToolError;
    type Args = TextArgs;
    type Output = String;

    fn description(&self) -> String {
        "Reverses the given text.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        })
    }

    async fn call(&self, args: TextArgs) -> Result<String, ToolError> {
        Ok(args.text.chars().rev().collect())
    }
}

/// One model turn issuing two tool calls in a single batch, then a plain-text
/// closing turn.
fn two_call_model() -> FakeModel {
    MockCompletionModel::from_stream_turns(vec![
        vec![
            MockStreamEvent::tool_call("call-1", "echo", serde_json::json!({ "text": "alpha" })),
            MockStreamEvent::tool_call("call-2", "reverse", serde_json::json!({ "text": "beta" })),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("done".to_string()),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ])
}

#[tokio::test]
async fn run_print_pairs_each_result_with_its_own_call_in_a_parallel_batch() {
    // `run_print` reaches process-global wiring (the hooks Stop dispatcher,
    // the subagent event sender); serialize against every other `run_print`
    // test so none of them clobber each other's.
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    let model = two_call_model();
    let agent = AgentBuilder::new(model)
        .tool(EchoTool)
        .tool(ReverseTool)
        .default_max_turns(2)
        .build();

    let outcome = run_print(
        &agent,
        "please echo and reverse",
        false,
        &RetryConfig::default(),
        Vec::new(),
        #[cfg(feature = "hooks")]
        None,
    )
    .await
    .expect("run_print should succeed against the fake model");

    assert_eq!(outcome.tool_interactions.len(), 2);

    let echo = outcome
        .tool_interactions
        .iter()
        .find(|i| i.name == "echo")
        .expect(
            "an interaction named 'echo' must be recorded (an empty or \
                 mispaired name means single-slot tracking regressed)",
        );
    assert_eq!(echo.args, serde_json::json!({ "text": "alpha" }));
    assert_eq!(echo.output, "echoed: alpha");

    let reverse = outcome
        .tool_interactions
        .iter()
        .find(|i| i.name == "reverse")
        .expect("an interaction named 'reverse' must be recorded");
    assert_eq!(reverse.args, serde_json::json!({ "text": "beta" }));
    assert_eq!(reverse.output, "ateb");
}

#[tokio::test]
async fn spawn_agent_events_pair_each_result_with_its_own_call_by_id() {
    // `spawn_agent` sets the same process-global subagent event sender as
    // `run_print`; hold the shared guard for the same reason.
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    let model = two_call_model();
    let agent = AgentBuilder::new(model)
        .tool(EchoTool)
        .tool(ReverseTool)
        .default_max_turns(2)
        .build();

    let mut runner = crate::agent::runner::spawn_agent(
        agent,
        "please echo and reverse".to_string(),
        Vec::new(),
        RetryConfig::default(),
        #[cfg(feature = "hooks")]
        None,
    );

    // One ordered log rather than a call vector and a result vector: the
    // arrival order is itself under test (see the batch-shape assertion
    // below), and two vectors discard it.
    let mut log: Vec<ToolEvent> = Vec::new();
    while let Some(event) = runner.event_rx.recv().await {
        match event {
            AgentEvent::ToolCall {
                call_id,
                name,
                args,
            } => log.push(ToolEvent::Call {
                id: call_id,
                name: name.to_string(),
                args,
            }),
            AgentEvent::ToolResult {
                call_id,
                name,
                output,
            } => log.push(ToolEvent::Result {
                id: call_id,
                name: name.to_string(),
                output: output.to_string(),
            }),
            AgentEvent::Done { .. } | AgentEvent::Error(_) => break,
            _ => {}
        }
    }

    let first_result = log
        .iter()
        .position(|e| matches!(e, ToolEvent::Result { .. }))
        .expect("the fake model's two calls must produce results");
    let calls_before_first_result = log[..first_result]
        .iter()
        .filter(|e| matches!(e, ToolEvent::Call { .. }))
        .count();
    assert_eq!(
        calls_before_first_result, 2,
        "this test's premise is the parallel-batch shape: both ToolCall events \
         must arrive before the first ToolResult. If rig switched to strict \
         call/result interleaving, the pairing assertions below would pass \
         without guarding anything"
    );

    let calls: Vec<(compact_str::CompactString, String, serde_json::Value)> = log
        .iter()
        .filter_map(|e| match e {
            ToolEvent::Call { id, name, args } => Some((id.clone(), name.clone(), args.clone())),
            ToolEvent::Result { .. } => None,
        })
        .collect();
    let results: Vec<(compact_str::CompactString, String, String)> = log
        .iter()
        .filter_map(|e| match e {
            ToolEvent::Result { id, name, output } => {
                Some((id.clone(), name.clone(), output.clone()))
            }
            ToolEvent::Call { .. } => None,
        })
        .collect();

    assert_eq!(calls.len(), 2);
    assert_eq!(results.len(), 2);
    assert_ne!(calls[0].0, calls[1].0, "event ids must be unique per call");

    for (result_id, result_name, output) in &results {
        let (_, call_name, args) = calls
            .iter()
            .find(|(call_id, _, _)| call_id == result_id)
            .expect("every ToolResult event must reference a ToolCall event's id");
        assert_eq!(
            result_name, call_name,
            "a ToolResult must carry the name of the call it answers"
        );
        let expected = match call_name.as_str() {
            "echo" => format!("echoed: {}", args["text"].as_str().unwrap()),
            "reverse" => args["text"].as_str().unwrap().chars().rev().collect(),
            other => panic!("unexpected tool name {other}"),
        };
        assert_eq!(output, &expected);
    }
}

#[test]
fn agent_run_state_pairs_each_result_of_a_two_call_batch() {
    let mut run = AgentRunState::default();
    run.push_pending_tool_call("call-1".into(), 7);
    run.push_pending_tool_call("call-2".into(), 8);

    // Results may come back in either order; each must find its own call.
    assert_eq!(run.take_pending_tool_call("call-2"), Some(8));
    assert_eq!(run.take_pending_tool_call("call-1"), Some(7));
    assert_eq!(run.take_pending_tool_call("call-1"), None);
    assert!(run.pending_tool_calls.is_empty());
}

#[cfg(any(feature = "subagents", feature = "acp"))]
#[test]
fn newest_pending_tool_call_falls_back_to_the_next_newest() {
    let mut run = AgentRunState::default();
    assert_eq!(run.newest_pending_tool_call(), None);

    run.push_pending_tool_call("call-1".into(), 7);
    run.push_pending_tool_call("call-2".into(), 8);
    assert_eq!(run.newest_pending_tool_call(), Some(8));

    // Once the newest call's result lands it is no longer a candidate parent
    // for a subagent call, but its still-running sibling is.
    run.take_pending_tool_call("call-2");
    assert_eq!(run.newest_pending_tool_call(), Some(7));

    run.take_pending_tool_call("call-1");
    assert_eq!(run.newest_pending_tool_call(), None);
}

#[test]
fn pushing_a_duplicate_id_replaces_the_stale_entry() {
    let mut run = AgentRunState::default();
    run.push_pending_tool_call("call-1".into(), 7);
    run.push_pending_tool_call("call-2".into(), 8);
    run.push_pending_tool_call("call-1".into(), 9);

    // One entry per id, and the live call wins — including for recency.
    assert_eq!(run.pending_tool_calls.len(), 2);
    #[cfg(any(feature = "subagents", feature = "acp"))]
    assert_eq!(run.newest_pending_tool_call(), Some(9));
    assert_eq!(run.take_pending_tool_call("call-1"), Some(9));
    assert_eq!(run.take_pending_tool_call("call-2"), Some(8));
}

#[test]
fn clearing_drops_calls_stranded_by_a_teardown() {
    let mut run = AgentRunState::default();
    run.push_pending_tool_call("call-1".into(), 7);
    run.push_pending_tool_call("call-2".into(), 8);

    run.clear_pending_tool_calls();

    assert!(run.pending_tool_calls.is_empty());
    assert_eq!(run.take_pending_tool_call("call-1"), None);
    // The decisive one: a stranded entry would otherwise parent the respawned
    // turn's first subagent call to the aborted turn's call.
    #[cfg(any(feature = "subagents", feature = "acp"))]
    assert_eq!(run.newest_pending_tool_call(), None);
}

#[test]
fn an_unmatched_tool_result_gets_its_own_call_instead_of_call_zero() {
    let mut session = Session::new("anthropic", "claude-test", 200_000, "");
    let mut run = AgentRunState::default();

    // Session call ids start at 0, so the first real call owns id 0 — the
    // value an unmatched result must never be linked to.
    let real_call = session.add_tool_call("read", &serde_json::json!({ "path": "a.txt" }));
    assert_eq!(real_call, 0);
    run.push_pending_tool_call("call-1".into(), real_call);

    let orphan = resolve_tool_result_call_id(&mut run, &mut session, "call-ghost", "grep");
    assert_ne!(orphan, real_call);
    session.add_tool_result(orphan, "grep", "no matches");

    // The real call's record is untouched...
    let calls: Vec<&ToolRecord> = session
        .messages
        .iter()
        .filter_map(|m| m.tool.as_ref())
        .filter(|r| matches!(r, ToolRecord::Call { .. }))
        .collect();
    assert!(matches!(
        calls.first(),
        Some(ToolRecord::Call { id, name, args })
            if *id == real_call && name == "read" && args["path"] == "a.txt"
    ));
    // ...and the orphan result answers a synthesized call of its own, with no
    // args because none were ever streamed.
    assert!(matches!(
        calls.get(1),
        Some(ToolRecord::Call { id, name, args })
            if *id == orphan && name == "grep" && args.is_null()
    ));

    // The pending real call is still in flight and still findable.
    assert_eq!(run.take_pending_tool_call("call-1"), Some(real_call));
}
