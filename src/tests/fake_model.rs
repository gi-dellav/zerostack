//! Shared test-only fake `CompletionModel` carrier for headless-dispatch e2e
//! tests (`run_print` over `Agent<FakeModel>`).
//!
//! Rig 0.40 ships exactly the scripted-streaming test double this change
//! needs (`rig::test_utils::MockCompletionModel`): a cloneable
//! `CompletionModel` with scripted streaming turns, a `GetTokenUsage`
//! response, and per-request capture (`requests()` returns the
//! `CompletionRequest`s it received, each carrying the `chat_history` the
//! caller sent). This module wraps it rather than hand-rolling the
//! `CompletionModel` impl, and adds only the thin, task-specific API this
//! change's tests need on top: scripting plain text turns, and reading back
//! the history a given turn received.
//!
//! Tool-call scripting is deliberately not wrapped here: `MockStreamEvent`
//! (also re-exported by rig) already supports it directly
//! (`MockStreamEvent::tool_call(..)`), so section 2's e2e can reach for that
//! itself when it needs it, without this module growing an API no current
//! caller exercises.

use rig::completion::Message;
pub use rig::test_utils::{MockCompletionModel, MockStreamEvent};

/// The fake `CompletionModel` carrier. Build with [`text_turns`] (or
/// [`text_chunks`] for the common single-turn case), pass into
/// `AgentBuilder::new(model).build()`, then inspect what it received via
/// [`history_at`].
pub type FakeModel = MockCompletionModel;

/// Script a fake model with one streaming turn per element of `turns`, each
/// yielding its plain-text chunks in order and ending in a default (zero)
/// usage final response. Turn N is consumed by the (N+1)th call the agent
/// makes to the model (e.g. turn 0 by the initial `stream_chat`, turn 1 by a
/// hooks `Stop`-continuation retry).
pub fn text_turns<I, J, S>(turns: I) -> FakeModel
where
    I: IntoIterator<Item = J>,
    J: IntoIterator<Item = S>,
    S: Into<String>,
{
    let scripted: Vec<Vec<MockStreamEvent>> = turns
        .into_iter()
        .map(|chunks| {
            chunks
                .into_iter()
                .map(|c| MockStreamEvent::text(c.into()))
                .chain(std::iter::once(
                    MockStreamEvent::final_response_with_default_usage(),
                ))
                .collect()
        })
        .collect();
    MockCompletionModel::from_stream_turns(scripted)
}

/// Script a fake model with a single streaming turn yielding `chunks` in
/// order.
pub fn text_chunks<I, S>(chunks: I) -> FakeModel
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    text_turns([chunks])
}

/// The chat history the model actually received on request `turn` (0-based),
/// with the trailing prompt message dropped: `CompletionRequest::chat_history`
/// always ends with the prompt (rig's documented invariant), so this returns
/// exactly the history the caller passed in alongside that prompt.
pub fn history_at(model: &FakeModel, turn: usize) -> Vec<Message> {
    let requests = model.requests();
    let request = requests.get(turn).unwrap_or_else(|| {
        panic!(
            "model was called only {} time(s); no request captured at turn {turn} \
             (did the expected continuation not fire?)",
            requests.len()
        )
    });
    let mut messages: Vec<Message> = request.chat_history.clone().into_iter().collect();
    messages.pop();
    messages
}

#[tokio::test]
async fn run_print_returns_scripted_text() {
    // Under `--features hooks`, `run_print` reaches the process-global Stop
    // dispatcher; serialize against the tests that install one so a leaked
    // hook can't force an unscripted continuation here. No-op otherwise.
    #[cfg(feature = "hooks")]
    let _dispatcher_guard = dispatcher_guard::acquire();

    let model = text_chunks(["hello, ", "world"]);
    let agent = rig::agent::AgentBuilder::new(model).build();

    let (response, _usage) = crate::agent::runner::run_print(
        &agent,
        "hi",
        false,
        &crate::retry::RetryConfig::default(),
        Vec::new(),
        #[cfg(feature = "hooks")]
        None,
    )
    .await
    .expect("run_print should succeed against the fake model");

    assert_eq!(response, "hello, world");
}

/// Serializes tests that touch the process-global hook dispatcher and clears
/// it when the guard drops. `run_print`'s `Stop` path reads a process-wide
/// dispatcher (`extras::hooks::DISPATCHER`), so a test that installs one via
/// `init_dispatcher` would otherwise leak that hook into any other `run_print`
/// test running concurrently in the same binary. Every `run_print` test holds
/// this guard for the duration of its call, so at most one such test runs at a
/// time and the dispatcher is always reset afterwards.
#[cfg(feature = "hooks")]
pub(crate) mod dispatcher_guard {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    /// Held for the lifetime of a dispatcher-touching test; resets the
    /// process-global dispatcher on drop.
    pub(crate) struct DispatcherGuard(#[allow(dead_code)] MutexGuard<'static, ()>);

    /// Acquire exclusive access to the process-global hook dispatcher.
    pub(crate) fn acquire() -> DispatcherGuard {
        let lock = LOCK.get_or_init(|| Mutex::new(()));
        DispatcherGuard(lock.lock().unwrap_or_else(|e| e.into_inner()))
    }

    impl Drop for DispatcherGuard {
        fn drop(&mut self) {
            crate::extras::hooks::reset_dispatcher();
        }
    }
}
