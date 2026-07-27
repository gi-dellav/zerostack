use crate::extras::advisor::format_conversation;
use crate::session::{MessageRole, SessionMessage};

fn msg(role: MessageRole, content: &str) -> SessionMessage {
    SessionMessage {
        role,
        content: content.into(),
        estimated_tokens: 0,
        tool: None,
    }
}

#[test]
fn format_conversation_empty() {
    assert_eq!(format_conversation(&[], 1), "");
}

#[test]
fn format_conversation_single_message() {
    let msgs = [msg(MessageRole::User, "hello")];
    assert_eq!(format_conversation(&msgs, 1024), "[User]: hello");
}

#[test]
fn format_conversation_two_messages() {
    let msgs = [
        msg(MessageRole::User, "hello"),
        msg(MessageRole::Assistant, "hi there"),
    ];
    assert_eq!(
        format_conversation(&msgs, 1024),
        "[User]: hello\n\n[Assistant]: hi there"
    );
}

#[test]
fn format_conversation_nothing_fits_returns_omission_only() {
    let msgs = [
        msg(MessageRole::User, "aaaaaaaaaa"),
        msg(MessageRole::Assistant, "bbbbbbbbbb"),
        msg(MessageRole::User, "cccccccccc"),
        msg(MessageRole::Assistant, "dddddddddd"),
    ];
    let result = format_conversation(&msgs, 0);
    assert!(result.contains("[... conversation omitted ...]"));
    // Nothing fits in head or tail, so result is just the marker (plus \n\n wrapping)
    assert!(!result.contains("[User]:"));
    assert!(!result.contains("[Assistant]:"));
}

#[test]
fn format_conversation_head_with_tail_gap() {
    // 7 messages, limit=0: head empty, tail empty, just omission marker
    let short = "aa";
    let msgs = [
        msg(MessageRole::User, short),
        msg(MessageRole::Assistant, short),
        msg(
            MessageRole::User,
            "a much longer message that takes more space",
        ),
        msg(
            MessageRole::Assistant,
            "another fairly long message here too",
        ),
        msg(
            MessageRole::User,
            "a third long message that pushes the limit",
        ),
        msg(MessageRole::Assistant, short),
        msg(MessageRole::User, short),
    ];
    let result = format_conversation(&msgs, 0);
    assert!(result.contains("[... conversation omitted ...]"));
    // With limit=0 nothing fits; head and tail are both empty
}

#[test]
fn format_conversation_small_limit_head_tail_with_gap() {
    // per_side=5 bytes: first short msg (~10B) fails head; last short msg (~10B) fails tail
    // but with just 2 short messages close together, one might slip into tail
    let msgs = [
        msg(MessageRole::User, "x"),
        msg(MessageRole::Assistant, "y"),
        msg(MessageRole::User, "z"),
    ];
    let result = format_conversation(&msgs, 0);
    // per_side=0: nothing fits; expect omission marker only
    assert!(result.contains("[... conversation omitted ...]"));
}

#[test]
fn format_conversation_no_gap() {
    let msgs = [
        msg(MessageRole::User, "hello"),
        msg(MessageRole::Assistant, "world"),
    ];
    let result = format_conversation(&msgs, 1024);
    assert!(!result.contains("[... conversation omitted ...]"));
}

#[test]
fn format_conversation_no_duplicate_when_head_covers_all() {
    let msgs = [
        msg(MessageRole::User, "short"),
        msg(MessageRole::Assistant, "msg"),
    ];
    let result = format_conversation(&msgs, 1024);
    assert_eq!(result.matches("[User]:").count(), 1);
    assert_eq!(result.matches("[Assistant]:").count(), 1);
}

#[test]
fn omission_marker_not_on_same_line_as_tail() {
    let msgs = [
        msg(MessageRole::User, "first"),
        msg(MessageRole::Assistant, "second"),
        msg(MessageRole::User, "third"),
        msg(MessageRole::Assistant, "fourth"),
        msg(MessageRole::User, "fifth"),
    ];
    let result = format_conversation(&msgs, 0);
    let marker_pos = result.find("[... conversation omitted ...]").unwrap();
    let after_marker = &result[marker_pos + "[... conversation omitted ...]".len()..];
    assert!(
        after_marker.starts_with("\n\n"),
        "marker must be separated from next message by \\n\\n, got: {after_marker:?}"
    );
}
