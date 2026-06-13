use crate::session::{MessageRole, Session, SessionMessage};

#[test]
fn estimate_tokens_empty() {
    // Empty string returns min of 1
    assert_eq!(Session::estimate_tokens(""), 1);
}

#[test]
fn estimate_tokens_short() {
    // 3 chars → 3/4 = 0, but min 1
    assert_eq!(Session::estimate_tokens("abc"), 1);
}

#[test]
fn estimate_tokens_exact_divisible() {
    assert_eq!(Session::estimate_tokens("abcd"), 1);
}

#[test]
fn estimate_tokens_rounds_down() {
    assert_eq!(Session::estimate_tokens("abcde"), 1);
}

#[test]
fn estimate_tokens_long() {
    assert_eq!(Session::estimate_tokens(&"x".repeat(100)), 25);
}

#[test]
fn new_session_has_id() {
    let s = Session::new("openai", "gpt-4", 128000);
    assert!(!s.id.is_empty());
}

#[test]
fn new_session_sets_provider_and_model() {
    let s = Session::new("anthropic", "claude-sonnet", 200000);
    assert_eq!(s.provider.as_str(), "anthropic");
    assert_eq!(s.model.as_str(), "claude-sonnet");
}

#[test]
fn new_session_sets_context_window() {
    let s = Session::new("openai", "gpt-4", 128000);
    assert_eq!(s.context_window, 128000);
}

#[test]
fn new_session_sets_working_dir() {
    let s = Session::new("openai", "gpt-4", 128000);
    assert!(!s.working_dir.is_empty());
}

#[test]
fn new_session_has_timestamps() {
    let s = Session::new("openai", "gpt-4", 128000);
    assert!(!s.created_at.is_empty());
    assert!(!s.updated_at.is_empty());
}

#[test]
fn new_session_starts_empty() {
    let s = Session::new("openai", "gpt-4", 128000);
    assert!(s.messages.is_empty());
    assert!(s.compactions.is_empty());
    assert_eq!(s.total_estimated_tokens, 0);
    assert_eq!(s.total_input_tokens, 0);
    assert_eq!(s.total_output_tokens, 0);
    assert_eq!(s.total_cost, 0.0);
}

#[test]
fn add_message_appends() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.add_message(MessageRole::User, "hello");
    assert_eq!(s.messages.len(), 1);
    assert_eq!(s.messages[0].role, MessageRole::User);
    assert_eq!(s.messages[0].content, "hello");
}

#[test]
fn add_message_increments_estimated_tokens() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    let before = s.total_estimated_tokens;
    s.add_message(MessageRole::Assistant, "hello world, this is a test");
    assert!(s.total_estimated_tokens > before);
}

#[test]
fn add_message_updates_updated_at() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    let before = s.updated_at.clone();
    // Brief sleep to ensure timestamp changes
    std::thread::sleep(std::time::Duration::from_millis(1));
    s.add_message(MessageRole::User, "hi");
    assert!(s.updated_at != before);
}

#[test]
fn needs_compaction_when_over_threshold() {
    let mut s = Session::new("openai", "gpt-4", 1000);
    s.add_message(MessageRole::User, &"x".repeat(900 * 4)); // ~900 tokens
    // With context_window=1000, reserve=200, threshold is 800
    // We have ~900 tokens, so should need compaction
    assert!(s.needs_compaction(200));
}

#[test]
fn needs_compaction_when_under_threshold() {
    let mut s = Session::new("openai", "gpt-4", 1000);
    s.add_message(MessageRole::User, "short");
    // Very few tokens, should not need compaction
    assert!(!s.needs_compaction(200));
}

#[test]
fn needs_compaction_zero_context_window() {
    let s = Session::new("openai", "gpt-4", 0);
    assert!(!s.needs_compaction(200));
}

#[test]
fn update_context_window_changes_value() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.update_context_window(256000);
    assert_eq!(s.context_window, 256000);
}

#[test]
fn compacted_context_returns_none_without_compactions() {
    let s = Session::new("openai", "gpt-4", 128000);
    let (summary, index) = s.compacted_context();
    assert!(summary.is_none());
    assert_eq!(index, 0);
}

#[test]
fn compress_adds_compaction_entry() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.add_message(MessageRole::User, "msg1");
    s.add_message(MessageRole::Assistant, "msg2");
    s.add_message(MessageRole::User, "msg3");
    s.add_message(MessageRole::Assistant, "msg4");

    let _before_count = s.messages.len();
    s.compress("summary text".to_string(), 2, 50);
    assert!(s.compactions.len() == 1);
    assert_eq!(s.compactions[0].summary, "summary text");
}

#[test]
fn compress_inserts_summary_as_system_message() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.add_message(MessageRole::User, "msg1");
    s.add_message(MessageRole::Assistant, "msg2");
    s.add_message(MessageRole::User, "msg3");

    s.compress("compressed summary".to_string(), 2, 30);
    // First message should now be the summary as System
    assert_eq!(s.messages[0].role, MessageRole::System);
    assert_eq!(s.messages[0].content, "compressed summary");
}

#[test]
fn compress_drains_messages_before_first_kept_index() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.add_message(MessageRole::User, "msg1");
    s.add_message(MessageRole::Assistant, "msg2");
    s.add_message(MessageRole::User, "msg3");
    s.add_message(MessageRole::Assistant, "msg4");

    s.compress("summary".to_string(), 2, 30);
    // Messages before index 2 (0,1) should be removed, replaced by summary
    // After compression: summary + msg3 + msg4 (plus summary takes index 0)
    assert_eq!(s.messages.len(), 3);
    assert_eq!(s.messages[0].role, MessageRole::System);
    assert_eq!(s.messages[1].content, "msg3");
    assert_eq!(s.messages[2].content, "msg4");
}

#[test]
fn compacted_context_returns_summary_after_compress() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.add_message(MessageRole::User, "msg1");
    s.add_message(MessageRole::Assistant, "msg2");
    s.compress("the summary".to_string(), 1, 20);

    let (summary, index) = s.compacted_context();
    assert_eq!(summary, Some("the summary"));
    assert_eq!(index, 1);
}
