use crate::session::{MessageRole, Session};

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
fn estimate_tokens_cjk_not_undercounted_like_chars_div4() {
    let text = "今天天氣很好真開心"; // 9 chars
    let est = Session::estimate_tokens(text);
    assert_eq!(est, 8); // 9 * 9 / 10 = 8
    assert!(est > (text.chars().count() as u64 / 4));
}

#[test]
fn estimate_tokens_mixed_cjk_and_latin() {
    let text = "請幫我 refactor this function 好嗎";
    let wide = text
        .chars()
        .filter(|c| {
            let o = *c as u32;
            (0x2E80..=0x9FFF).contains(&o)
        })
        .count() as u64;
    let est = Session::estimate_tokens(text);
    assert!(est >= wide * 9 / 10);
}

#[test]
fn estimate_tokens_pure_ascii_matches_old_formula() {
    let text = "the quick brown fox jumps over the lazy dog";
    assert_eq!(Session::estimate_tokens(text), text.len() as u64 / 4);
}

#[test]
fn effective_context_falls_back_without_calibration() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.add_message(MessageRole::User, "hello world this is a test message");
    assert_eq!(s.effective_context_tokens(), s.total_estimated_tokens);
}

#[test]
fn effective_context_uses_calibration_anchor_plus_delta() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.add_message(MessageRole::User, "first user message");
    s.add_message(MessageRole::Assistant, "assistant reply");
    s.set_calibration(5000, 200); // anchor = 5200, covers 2 messages
    assert_eq!(s.calibrated_msg_count, 2);

    s.add_message(MessageRole::User, "a follow up question");
    let delta = Session::estimate_tokens("a follow up question");
    assert_eq!(s.effective_context_tokens(), 5200 + delta);
}

#[test]
fn calibration_ignores_zero_usage() {
    let mut s = Session::new("openai", "gpt-4", 128000);
    s.add_message(MessageRole::User, "msg");
    s.set_calibration(0, 0);
    assert_eq!(s.calibrated_tokens, 0);
    assert_eq!(s.effective_context_tokens(), s.total_estimated_tokens);
}

#[test]
fn real_input_tokens_native_route_adds_cache_fields() {
    // The Anthropic-native route reports input_tokens excluding cached/
    // cache-creation tokens, so the real prompt size is the sum of all three.
    // A cache hit (input ~0, cached large) must NOT collapse the measured context.
    assert_eq!(Session::real_input_tokens(true, 10, 7000, 0), 7010);
    assert_eq!(Session::real_input_tokens(true, 0, 7000, 0), 7000);
    assert_eq!(Session::real_input_tokens(true, 50, 0, 6000), 6050);
}

#[test]
fn real_input_tokens_non_native_uses_input_only() {
    // OpenAI/Gemini/OpenRouter fold the cached subset into input_tokens and
    // report no cache-creation; adding the cache fields would double-count.
    assert_eq!(Session::real_input_tokens(false, 7000, 5600, 0), 7000);
}

// Helper: a session with `n` ASCII messages of `len` chars each, so every
// message has a predictable estimated_tokens == len/4.
fn session_with_messages(n: usize, len: usize) -> Session {
    let mut s = Session::new("openai", "gpt-4", 128000);
    for _ in 0..n {
        s.add_message(MessageRole::User, &"x".repeat(len));
    }
    s
}

#[test]
fn compaction_cut_keeps_recent_within_budget() {
    // 4 messages × 10 tokens = 40 total. keep_recent=15 reaches back across
    // the last two (20 tokens), so the first two are summarized.
    let s = session_with_messages(4, 40);
    assert_eq!(s.messages[0].estimated_tokens, 10);
    assert_eq!(Session::select_compaction_cut(&s.messages, 15), 2);
}

#[test]
fn compaction_cut_oversized_keep_recent_summarizes_nothing() {
    // Regression: keep_recent (100) larger than the whole history (40) must
    // keep the recent messages, NOT summarize everything (cut == 0, which the
    // caller treats as "entire context is recent").
    let s = session_with_messages(4, 40);
    assert_eq!(Session::select_compaction_cut(&s.messages, 100), 0);
}

#[test]
fn compaction_cut_zero_keep_recent_summarizes_all() {
    let s = session_with_messages(4, 40);
    assert_eq!(Session::select_compaction_cut(&s.messages, 0), 4);
}

#[test]
fn compaction_cut_single_message_is_kept() {
    let s = session_with_messages(1, 40); // 1 msg, 10 tokens
    assert_eq!(Session::select_compaction_cut(&s.messages, 5), 0);
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

#[test]
fn detect_git_branch_in_repo_returns_nonempty() {
    let b = Session::detect_git_branch(env!("CARGO_MANIFEST_DIR"));
    assert!(b.is_some(), "repo root should resolve a branch or commit");
    assert!(!b.unwrap().is_empty());
}

#[test]
fn detect_git_branch_outside_repo_is_none() {
    let p = std::env::temp_dir().join("zerostack-definitely-not-a-repo-xyz123");
    assert!(Session::detect_git_branch(p.to_str().unwrap()).is_none());
}

#[test]
fn parse_porcelain_counts_changes_and_sync() {
    let out = "\
# branch.oid abc123
# branch.head main
# branch.upstream origin/main
# branch.ab +2 -1
1 M. N... 100644 100644 100644 aaa bbb staged.rs
1 .M N... 100644 100644 100644 aaa bbb modified.rs
1 .D N... 100644 100644 000000 aaa bbb deleted.rs
? untracked.rs
";
    let g = crate::session::Session::parse_porcelain(out);
    assert_eq!(g.staged, 1);
    assert_eq!(g.modified, 1);
    assert_eq!(g.deleted, 1);
    assert_eq!(g.untracked, 1);
    assert_eq!(g.ahead, 2);
    assert_eq!(g.behind, 1);
    assert!(g.is_dirty());
}
