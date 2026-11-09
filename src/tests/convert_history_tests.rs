//! Unit tests for `convert_history`'s assembly of a resumed session's prior
//! turns into the `rig::completion::Message` history handed to the model.

use rig::completion::Message;

use crate::agent::runner::convert_history;
use crate::session::{MessageRole, Session};

fn sample_session() -> Session {
    Session::new("anthropic", "claude-test", 200_000, "")
}

#[test]
fn uncompacted_session_gives_full_tail_in_order_no_summary() {
    let mut session = sample_session();
    session.add_message(MessageRole::User, "hello");
    session.add_message(MessageRole::Assistant, "hi there");
    session.add_message(MessageRole::User, "how are you");

    let history = convert_history(&session);

    assert_eq!(
        history,
        vec![
            Message::user("hello"),
            Message::assistant("hi there"),
            Message::user("how are you"),
        ]
    );
}

#[test]
fn compacted_session_gives_summary_as_assistant_then_kept_tail() {
    let mut session = sample_session();
    session.add_message(MessageRole::User, "old question");
    session.add_message(MessageRole::Assistant, "old answer");
    session.add_message(MessageRole::User, "kept question");
    session.add_message(MessageRole::Assistant, "kept answer");

    // Summarize the first two messages, keeping the last two.
    session.compress("did some prior work".to_string(), 2, 100);

    let history = convert_history(&session);

    assert_eq!(
        history,
        vec![
            Message::assistant(
                "[Recap of my prior work in this conversation]\ndid some prior work"
            ),
            Message::user("kept question"),
            Message::assistant("kept answer"),
        ]
    );
}

#[test]
fn empty_session_gives_empty_vec() {
    let session = sample_session();

    let history = convert_history(&session);

    assert_eq!(history, Vec::<Message>::new());
}

/// An attachment stays part of the conversation: the history of a later turn
/// carries the image again, in the same user message as the text.
#[test]
#[cfg(feature = "multimodal")]
fn user_message_with_media_keeps_the_attachment_in_the_history() {
    use rig::completion::message::UserContent;

    let path =
        std::env::temp_dir().join(format!("zerostack_test_history_{}.png", std::process::id()));
    std::fs::write(&path, b"fake png").unwrap();
    let media = crate::extras::multimodal::attach(&path).unwrap();

    let mut session = sample_session();
    session.add_user_message("look at this", vec![media]);

    let history = convert_history(&session);
    let _ = std::fs::remove_file(&path);

    assert_eq!(history.len(), 1);
    let Message::User { content } = &history[0] else {
        panic!("expected a user message, got {:?}", history[0]);
    };
    assert_eq!(content.len(), 2);
    assert!(matches!(content[0], UserContent::Text(_)));
    assert!(matches!(content[1], UserContent::Image(_)));
}
