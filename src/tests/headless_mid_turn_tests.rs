//! Headless (`-p`) mid-turn compaction: when a completion's real prompt
//! crosses `mid_turn_compact_threshold * context_window` and it asked for
//! tools, `run_print_with_ceiling` lets those tools finish and stops before
//! the next request. `dispatch_print` then compacts and resumes; the
//! end-to-end path (including the summary request and the respawn) is covered
//! by the stub-server repro in the issue, since driving `dispatch_print` needs
//! the full CLI plumbing (see `resumed_history_tests.rs`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rig::agent::AgentBuilder;
use rig::completion::Usage;
use rig::tool::Tool;
use serde::Deserialize;

use crate::agent::runner::{MidTurnCeiling, MidTurnStop, run_print, run_print_with_ceiling};
use crate::agent::tools::ToolError;
use crate::retry::RetryConfig;
use crate::tests::fake_model::{FakeModel, MockCompletionModel, MockStreamEvent};

#[derive(Debug, Deserialize)]
struct CountArgs {}

/// Counts its executions, so a test can prove a tool call did or did not run.
struct CountTool(Arc<AtomicUsize>);

impl Tool for CountTool {
    const NAME: &'static str = "count";

    type Error = ToolError;
    type Args = CountArgs;
    type Output = String;

    fn description(&self) -> String {
        "Counts calls.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn call(
        &self,
        _context: &mut rig::tool::ToolContext,
        _args: CountArgs,
    ) -> Result<String, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("counted".to_string())
    }
}

const WINDOW: u64 = 32_768;

fn ceiling(awaiting_relief: bool) -> Option<MidTurnCeiling> {
    Some(MidTurnCeiling {
        context_window: WINDOW,
        threshold: 0.5,
        anthropic_native: false,
        awaiting_relief,
        grace_calls: 0,
    })
}

fn usage(input_tokens: u64) -> Usage {
    Usage::new().input_tokens(input_tokens).output_tokens(10)
}

/// Turn 1: a tool call whose completion reports `first_prompt` input tokens.
/// Turn 2: final text whose completion reports `second_prompt`.
fn tool_then_text(first_prompt: u64, second_prompt: u64) -> FakeModel {
    MockCompletionModel::from_stream_turns(vec![
        vec![
            MockStreamEvent::tool_call("call-1", "count", serde_json::json!({})),
            MockStreamEvent::final_response(usage(first_prompt)),
        ],
        vec![
            MockStreamEvent::text("done".to_string()),
            MockStreamEvent::final_response(usage(second_prompt)),
        ],
    ])
}

/// Two tool-call turns reporting `p1` / `p2` prompt tokens, then final text.
fn tool_tool_text(p1: u64, p2: u64) -> FakeModel {
    MockCompletionModel::from_stream_turns(vec![
        vec![
            MockStreamEvent::tool_call("call-1", "count", serde_json::json!({})),
            MockStreamEvent::final_response(usage(p1)),
        ],
        vec![
            MockStreamEvent::tool_call("call-2", "count", serde_json::json!({})),
            MockStreamEvent::final_response(usage(p2)),
        ],
        vec![
            MockStreamEvent::text("done".to_string()),
            MockStreamEvent::final_response(usage(1_000)),
        ],
    ])
}

async fn run(
    model: FakeModel,
    ceiling: Option<MidTurnCeiling>,
) -> (crate::agent::runner::PrintOutcome, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = AgentBuilder::new(model)
        .tool(CountTool(calls.clone()))
        .default_max_turns(4)
        .build();
    let outcome = run_print_with_ceiling(
        &agent,
        "go",
        false,
        &RetryConfig::default(),
        Vec::new(),
        #[cfg(feature = "hooks")]
        None,
        ceiling,
    )
    .await
    .expect("run_print_with_ceiling");
    (outcome, calls.load(Ordering::SeqCst))
}

#[tokio::test]
async fn over_threshold_runs_the_completions_tools_then_stops() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    // 30_000 / 32_768 = 92% > 50%.
    let (outcome, tool_runs) = run(tool_then_text(30_000, 1_000), ceiling(false)).await;
    assert_eq!(
        outcome.mid_turn_stop,
        Some(MidTurnStop::Compact {
            prompt_tokens: 30_000
        })
    );
    assert_eq!(tool_runs, 1, "the crossing completion's work is kept");
    assert_eq!(outcome.tool_interactions.len(), 1);
    assert!(
        !outcome.response.contains("done"),
        "stopped before the next request"
    );
    assert_eq!(
        outcome.usage.input_tokens,
        Some(30_000),
        "usage comes from the per-call sum"
    );
}

#[tokio::test]
async fn over_threshold_right_after_compaction_is_exhausted() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (outcome, tool_runs) = run(tool_then_text(30_000, 1_000), ceiling(true)).await;
    assert_eq!(
        outcome.mid_turn_stop,
        Some(MidTurnStop::Exhausted {
            prompt_tokens: 30_000
        })
    );
    assert_eq!(tool_runs, 1);
}

#[tokio::test]
async fn over_threshold_on_the_final_answer_does_not_stop() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    // The crossing completion asks for no tools: the run is ending anyway.
    let (outcome, tool_runs) = run(tool_then_text(1_000, 30_000), ceiling(false)).await;
    assert_eq!(outcome.mid_turn_stop, None);
    assert_eq!(tool_runs, 1);
    assert!(outcome.response.contains("done"));
}

#[tokio::test]
async fn a_call_under_threshold_clears_awaiting_relief() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    // First call is under (relief worked), so a later crossing compacts again
    // instead of counting as "still over right after a compaction".
    let (outcome, tool_runs) = run(tool_tool_text(1_000, 30_000), ceiling(true)).await;
    assert_eq!(
        outcome.mid_turn_stop,
        Some(MidTurnStop::Compact {
            prompt_tokens: 30_000
        })
    );
    assert_eq!(tool_runs, 2);
    assert_eq!(outcome.tool_interactions.len(), 2);
}

#[tokio::test]
async fn under_threshold_run_is_unchanged() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let (outcome, tool_runs) = run(tool_then_text(1_000, 2_000), ceiling(false)).await;
    assert_eq!(outcome.mid_turn_stop, None);
    assert_eq!(tool_runs, 1);
    assert!(outcome.response.contains("done"));
}

#[tokio::test]
async fn a_grace_call_runs_over_threshold_then_the_next_crossing_compacts() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    // After a compaction that could not help, the caller grants one call: it
    // runs over the ceiling, and the following crossing compacts again
    // instead of reporting exhaustion.
    let grace = Some(MidTurnCeiling {
        grace_calls: 1,
        ..ceiling(true).unwrap()
    });
    let (outcome, tool_runs) = run(tool_tool_text(30_000, 30_000), grace).await;
    assert_eq!(
        outcome.mid_turn_stop,
        Some(MidTurnStop::Compact {
            prompt_tokens: 30_000
        })
    );
    assert_eq!(tool_runs, 2);
}

#[tokio::test]
async fn no_ceiling_keeps_historical_behavior() {
    let _guard = crate::tests::fake_model::run_print_guard::acquire();
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = AgentBuilder::new(tool_then_text(30_000, 30_000))
        .tool(CountTool(calls.clone()))
        .default_max_turns(4)
        .build();
    let outcome = run_print(
        &agent,
        "go",
        false,
        &RetryConfig::default(),
        Vec::new(),
        #[cfg(feature = "hooks")]
        None,
    )
    .await
    .expect("run_print");
    assert_eq!(outcome.mid_turn_stop, None);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(outcome.response.contains("done"));
}
