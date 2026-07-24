use crate::extras::export::{parse_jsonl_import, session_to_html, session_to_jsonl};
use crate::session::{MessageRole, Session};

fn sample_session() -> Session {
    let mut session = Session::new("openrouter", "test-model", 128_000, "demo session");
    session.add_message(MessageRole::User, "hello there");
    session.add_message(MessageRole::Assistant, "hi! **how** can I help?");
    session.add_message(MessageRole::ToolCall, "bash: ls -la");
    session.add_message(MessageRole::ToolResult, "bash:\ntotal 0");
    session
}

#[test]
fn jsonl_round_trip_preserves_messages() {
    let session = sample_session();
    let jsonl = session_to_jsonl(&session);
    let messages = parse_jsonl_import(&jsonl).unwrap();
    assert_eq!(messages.len(), session.messages.len());
    for (imported, original) in messages.iter().zip(session.messages.iter()) {
        assert_eq!(imported.role, original.role);
        assert_eq!(imported.content, original.content);
    }
}

#[test]
fn jsonl_first_line_is_session_metadata() {
    let session = sample_session();
    let jsonl = session_to_jsonl(&session);
    let header: serde_json::Value = serde_json::from_str(jsonl.lines().next().unwrap()).unwrap();
    assert_eq!(header["type"], "session");
    assert_eq!(header["name"], "demo session");
    assert_eq!(header["model"], "test-model");
}

#[test]
fn jsonl_import_accepts_bare_lines_without_metadata() {
    let jsonl =
        "{\"role\":\"user\",\"content\":\"hi\"}\n{\"role\":\"assistant\",\"content\":\"yo\"}\n";
    let messages = parse_jsonl_import(jsonl).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, MessageRole::User);
    assert_eq!(messages[0].content, "hi");
    assert_eq!(messages[1].role, MessageRole::Assistant);
}

#[test]
fn jsonl_import_errors_with_line_number_on_bad_json() {
    let jsonl = "{\"role\":\"user\",\"content\":\"ok\"}\nnot json\n";
    let err = parse_jsonl_import(jsonl).unwrap_err();
    assert!(
        err.to_string().contains("line 2"),
        "error should name the line: {err}"
    );
}

#[test]
fn jsonl_import_errors_when_no_messages() {
    let err = parse_jsonl_import("{\"type\":\"session\"}\n").unwrap_err();
    assert!(err.to_string().contains("no messages"));
}

#[test]
fn html_escapes_verbatim_content() {
    let mut session = Session::new("p", "m", 128_000, "x");
    session.add_message(MessageRole::User, "show me <script>alert(1)</script> & go");
    let html = session_to_html(&session);
    assert!(html.contains("&lt;script&gt;"), "user HTML must be escaped");
    assert!(html.contains("&amp;"), "ampersand must be escaped");
    assert!(!html.contains("<script>alert"));
}

#[test]
fn html_renders_assistant_markdown() {
    let session = sample_session();
    let html = session_to_html(&session);
    assert!(
        html.contains("<strong>how</strong>"),
        "assistant markdown should render: {html}"
    );
}

#[test]
fn html_contains_session_metadata() {
    let session = sample_session();
    let html = session_to_html(&session);
    assert!(html.contains("demo session"));
    assert!(html.contains("openrouter / test-model"));
    assert!(html.contains("<!DOCTYPE html>"));
}

#[test]
fn html_title_falls_back_to_session_id() {
    let session = Session::new("p", "m", 128_000, "");
    let html = session_to_html(&session);
    assert!(
        html.contains("zerostack session"),
        "unnamed sessions get an id-based title: {html}"
    );
}
