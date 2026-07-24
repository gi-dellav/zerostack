//! e2e proof that a resumed session's prior messages reach the model.
//!
//! Exercises the `-p --continue` code path at the boundary `run_print` and
//! the fake model carrier share: a `Session` seeded with prior turns is
//! converted via `convert_history` (the same conversion `dispatch_print`
//! uses) and threaded through `run_print`, then the carrier's captured
//! request history is asserted to match. Section 4 extends this same test
//! with persistence and hooks-continuation assertions; keep additions here
//! easy to layer on top rather than rewriting the scaffolding.

use rig::agent::AgentBuilder;

use crate::agent::runner::{convert_history, run_print};
use crate::retry::RetryConfig;
use crate::session::{MessageRole, Session};
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

    // Mirrors `dispatch_print`'s own `convert_history(&self.session)` call:
    // this is the `-p --continue` code path being exercised end to end.
    let (_response, _usage) = run_print(
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
