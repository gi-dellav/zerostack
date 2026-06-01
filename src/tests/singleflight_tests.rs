//! Tests for the single-flight submission policy that prevents orphaning a
//! running main agent run.

use crate::ui::{SubmitAction, allowed_while_running, classify_submission};

#[test]
fn idle_runs_immediately() {
    assert_eq!(classify_submission(false, "hello"), SubmitAction::Run);
    assert_eq!(classify_submission(false, "/help"), SubmitAction::Run);
}

#[test]
fn running_plain_text_is_queued() {
    // The whole point: typing while busy must NOT spawn a second run.
    assert_eq!(classify_submission(true, "ok"), SubmitAction::Queue);
    assert_eq!(
        classify_submission(true, "compile this"),
        SubmitAction::Queue
    );
    assert_eq!(
        classify_submission(true, "  leading space"),
        SubmitAction::Queue
    );
}

#[test]
fn running_commands_are_rejected_not_queued() {
    // Commands not on the `allowed_while_running` whitelist are rejected while a
    // run is active (they can't be cleanly queued/replayed).
    assert_eq!(
        classify_submission(true, "/model gpt"),
        SubmitAction::RejectWhileRunning
    );
    assert_eq!(
        classify_submission(true, ".prompt"),
        SubmitAction::RejectWhileRunning
    );
    assert_eq!(
        classify_submission(true, "!ls"),
        SubmitAction::RejectWhileRunning
    );
    assert_eq!(
        classify_submission(true, "   /compress"),
        SubmitAction::RejectWhileRunning
    );
}

#[test]
fn whitelisted_commands_pass_through_while_running() {
    // /queue manages the queue and must work while a run is active, so it
    // classifies as Run (falls through to its handler) instead of being gated.
    assert!(allowed_while_running("/queue"));
    assert!(allowed_while_running("/queue ls"));
    assert!(allowed_while_running("/queue clear"));
    assert!(allowed_while_running("  /queue pop"));
    assert!(!allowed_while_running("/model gpt"));
    assert!(!allowed_while_running("hello"));

    assert_eq!(classify_submission(true, "/queue"), SubmitAction::Run);
    assert_eq!(classify_submission(true, "/queue ls"), SubmitAction::Run);
    assert_eq!(classify_submission(true, "/queue clear"), SubmitAction::Run);
}

#[test]
fn running_empty_is_ignored() {
    assert_eq!(classify_submission(true, ""), SubmitAction::Ignore);
    assert_eq!(classify_submission(true, "   "), SubmitAction::Ignore);
}
