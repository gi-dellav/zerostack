//! E2e proof that a headless (`-p`) run persists the tool calls its
//! subagents make, each attributed to the `task` call that spawned it.
//!
//! Exercises the same `run_print` boundary as `headless_tool_record_tests.rs`
//! (see that file's header for why the fake `CompletionModel` carrier is
//! driven directly rather than through the full `dispatch_print` CLI
//! plumbing), with one addition: a stand-in `task` tool that emits
//! `AgentEvent::SubagentToolCall` on the process-global subagent channel
//! exactly as `runner::run_subagent` does. The real `TaskTool` cannot be
//! driven here — it builds its subagent from the global `SubagentConfig`'s
//! `AnyClient`, which has no fake-model variant — and `run_subagent` itself
//! is not what this section changes: the gap being closed is that headless
//! runs never listened on that channel at all.

use rig::agent::AgentBuilder;
use rig::tool::Tool;
use serde::Deserialize;

use crate::agent::runner::{PrintOutcome, run_print};
use crate::agent::tools::ToolError;
use crate::event::AgentEvent;
use crate::retry::RetryConfig;
use crate::session::{MessageRole, Session, ToolRecord};
use crate::tests::fake_model::{FakeModel, MockCompletionModel, MockStreamEvent};

#[derive(Debug, Deserialize)]
struct TaskArgs {
    prompts: Vec<String>,
}

/// Stand-in for `TaskTool`: emits one `SubagentToolCall` event per prompt on
/// the channel `run_print` wires up, then returns a summary, mirroring a real
/// `task` call whose subagents each ran one tool.
struct FakeTaskTool;

impl Tool for FakeTaskTool {
    const NAME: &'static str = "task";

    type Error = ToolError;
    type Args = TaskArgs;
    type Output = String;

    fn description(&self) -> String {
        "Investigates the codebase via a subagent.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "prompts": { "type": "array", "items": { "type": "string" } } },
            "required": ["prompts"]
        })
    }

    async fn call(&self, args: TaskArgs) -> Result<String, ToolError> {
        let tx = crate::extras::subagents::clone_subagent_event_tx()
            .expect("run_print must publish a subagent event sender for the turn");
        for prompt in &args.prompts {
            tx.send(AgentEvent::SubagentToolCall {
                name: "grep".into(),
                args: serde_json::json!({ "pattern": prompt }),
            })
            .await
            .expect("subagent event channel must stay open for the whole turn");
        }
        Ok(format!("investigated {} prompt(s)", args.prompts.len()))
    }
}

/// Longer than `format_tool_call_summary`'s 200-char display truncation
/// (`src/ui/utils.rs`), so a passing assertion on the structured `args` field
/// proves it carries the complete value, not the truncated display summary
/// that lands in `content`.
fn long_prompt(tag: &str) -> String {
    format!("{tag}-{}", "x".repeat(500))
}

fn task_call_event(id: &str, prompts: &[String]) -> MockStreamEvent {
    MockStreamEvent::tool_call(id, "task", serde_json::json!({ "prompts": prompts }))
}

/// Mirrors `dispatch_print`'s recording sequence (`src/startup.rs`) exactly:
/// the user message, then per tool interaction its call, the subagent calls
/// it spawned, and its result, then the assistant message.
fn record_turn_into_session(
    session: &mut Session,
    prompt: &str,
    result: anyhow::Result<PrintOutcome>,
) -> anyhow::Result<()> {
    let outcome = result?;
    session.add_message(MessageRole::User, prompt);
    for interaction in &outcome.tool_interactions {
        let call_id = session.add_tool_call(&interaction.name, &interaction.args);
        for subagent_call in &interaction.subagent_calls {
            session.add_subagent_tool_call(Some(call_id), &subagent_call.name, &subagent_call.args);
        }
        session.add_tool_result(call_id, &interaction.name, &interaction.output);
    }
    session.add_message(MessageRole::Assistant, &outcome.response);
    Ok(())
}

fn call_id_of(session: &Session, index: usize) -> u64 {
    match &session.messages[index].tool {
        Some(ToolRecord::Call { id, .. }) => *id,
        other => panic!("expected a Call record at message {index}, got {other:?}"),
    }
}

fn subagent_record(session: &Session, index: usize) -> (u64, String, serde_json::Value) {
    match &session.messages[index].tool {
        Some(ToolRecord::SubagentCall {
            parent_call_id,
            name,
            args,
        }) => (*parent_call_id, name.to_string(), args.clone()),
        other => panic!("expected a SubagentCall record at message {index}, got {other:?}"),
    }
}

async fn run_task_turn(model: FakeModel, max_turns: usize) -> anyhow::Result<PrintOutcome> {
    let agent = AgentBuilder::new(model)
        .tool(FakeTaskTool)
        .default_max_turns(max_turns)
        .build();

    run_print(
        &agent,
        "investigate",
        false,
        &RetryConfig::default(),
        Vec::new(),
        #[cfg(feature = "hooks")]
        None,
    )
    .await
}

#[tokio::test]
async fn headless_subagent_calls_recorded_with_full_args_and_parent_call_id() {
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    let prompts = vec![long_prompt("first"), long_prompt("second")];
    let model = MockCompletionModel::from_stream_turns(vec![
        vec![
            task_call_event("call-1", &prompts),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("done".to_string()),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);

    let outcome = run_task_turn(model, 2)
        .await
        .expect("run_print should succeed against the fake model");

    assert_eq!(outcome.tool_interactions.len(), 1);
    let interaction = &outcome.tool_interactions[0];
    assert_eq!(interaction.name, "task");
    assert_eq!(
        interaction
            .subagent_calls
            .iter()
            .map(|c| (c.name.as_str(), c.args.clone()))
            .collect::<Vec<_>>(),
        prompts
            .iter()
            .map(|p| ("grep", serde_json::json!({ "pattern": p })))
            .collect::<Vec<_>>(),
        "every subagent tool call of the enclosing task call must be collected, \
         in order and with complete args"
    );

    let mut session = Session::new("anthropic", "claude-test", 200_000, "");
    record_turn_into_session(&mut session, "investigate", Ok(outcome))
        .expect("recording a successful turn must not fail");

    assert_eq!(
        session.messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        vec![
            MessageRole::User,
            MessageRole::ToolCall,
            MessageRole::SubagentToolCall,
            MessageRole::SubagentToolCall,
            MessageRole::ToolResult,
            MessageRole::Assistant,
        ],
        "subagent calls belong between their task call and its result, the \
         order they happened in"
    );

    let task_call_id = call_id_of(&session, 1);
    for (offset, prompt) in prompts.iter().enumerate() {
        let (parent_call_id, name, args) = subagent_record(&session, 2 + offset);
        assert_eq!(parent_call_id, task_call_id);
        assert_eq!(name, "grep");
        assert_eq!(args, serde_json::json!({ "pattern": prompt }));
        // The display summary in `content` is truncated at 200 chars
        // (`format_tool_call_summary`); the structured field must not be.
        assert!(
            session.messages[2 + offset].content.len() < prompt.len(),
            "content should be the truncated display summary, not the full args"
        );
    }
}

/// A subagent that makes more tool calls than the event channel can hold must
/// not stall the turn: `run_print` has to keep draining while the `task` call
/// is still running, not only once its result arrives. Without that, the
/// sender blocks on a full channel with nobody reading it, the `task` tool
/// never returns, and the run hangs — so this test's failure mode is the
/// timeout, and it is the reason the drain is a `select!` arm rather than a
/// sweep at the end of the call.
#[tokio::test]
async fn subagent_calls_beyond_the_channel_capacity_do_not_stall_the_turn() {
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    // Comfortably over the channel's capacity.
    let prompts: Vec<String> = (0..200).map(|i| format!("prompt {i}")).collect();
    let model = MockCompletionModel::from_stream_turns(vec![
        vec![
            task_call_event("call-1", &prompts),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("done".to_string()),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), run_task_turn(model, 2))
        .await
        .expect("the turn must not stall on a full subagent event channel")
        .expect("run_print should succeed against the fake model");

    assert_eq!(outcome.tool_interactions.len(), 1);
    assert_eq!(
        outcome.tool_interactions[0]
            .subagent_calls
            .iter()
            .map(|c| c.args["pattern"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        prompts,
        "every subagent call must be collected, in order, however many there are"
    );
}

/// The parent link is per call, not per turn: with two `task` calls in one
/// turn, each subagent record points at the call it ran under.
#[tokio::test]
async fn subagent_calls_attribute_to_their_own_task_call() {
    let _run_print_guard = crate::tests::fake_model::run_print_guard::acquire();

    let first = vec!["first prompt".to_string()];
    let second = vec!["second prompt".to_string()];
    let model = MockCompletionModel::from_stream_turns(vec![
        vec![
            task_call_event("call-1", &first),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            task_call_event("call-2", &second),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("done".to_string()),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ]);

    let outcome = run_task_turn(model, 3)
        .await
        .expect("run_print should succeed against the fake model");

    assert_eq!(outcome.tool_interactions.len(), 2);
    for (interaction, prompts) in outcome
        .tool_interactions
        .iter()
        .zip([&first, &second].into_iter())
    {
        assert_eq!(interaction.subagent_calls.len(), 1);
        assert_eq!(
            interaction.subagent_calls[0].args,
            serde_json::json!({ "pattern": prompts[0] }),
            "a task call must collect only the subagent calls it spawned"
        );
    }

    let mut session = Session::new("anthropic", "claude-test", 200_000, "");
    record_turn_into_session(&mut session, "investigate", Ok(outcome))
        .expect("recording a successful turn must not fail");

    // User, (call, subagent call, result) x 2, assistant.
    assert_eq!(session.messages.len(), 8);
    let first_call_id = call_id_of(&session, 1);
    let second_call_id = call_id_of(&session, 4);
    assert_ne!(first_call_id, second_call_id);
    assert_eq!(subagent_record(&session, 2).0, first_call_id);
    assert_eq!(subagent_record(&session, 5).0, second_call_id);
}
