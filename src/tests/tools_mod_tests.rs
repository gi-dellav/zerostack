use crate::agent::tools::{
    deny_repeated_reads, is_skip_dir, set_deny_repeated_reads, track_read, untrack_read_path,
};
use std::sync::{Mutex, MutexGuard};

/// Serializes the tests that touch the global deny-repeated-reads flag and
/// read tracker: they share process-wide state and would otherwise race
/// under parallel test execution.
static TRACK_TEST_LOCK: Mutex<()> = Mutex::new(());

fn track_test_lock() -> MutexGuard<'static, ()> {
    TRACK_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn skip_node_modules() {
    assert!(is_skip_dir("node_modules"));
}

#[test]
fn skip_target() {
    assert!(is_skip_dir("target"));
}

#[test]
fn skip_case_sensitive() {
    assert!(!is_skip_dir("Node_Modules"));
    assert!(!is_skip_dir("TARGET"));
}

#[test]
fn skip_other_dirs() {
    assert!(!is_skip_dir("src"));
    assert!(!is_skip_dir(""));
    assert!(!is_skip_dir("node_modules_extra"));
}

#[test]
fn track_read_returns_none_when_deny_disabled() {
    let _guard = track_test_lock();
    set_deny_repeated_reads(false);
    // clean up tracker from previous tests
    untrack_read_path("test_path");

    let result = track_read("test_path", 0, 10);
    assert!(result.is_none());
}

#[test]
fn track_read_first_call_returns_none() {
    let _guard = track_test_lock();
    set_deny_repeated_reads(true);
    untrack_read_path("test_path");

    let result = track_read("test_path", 1, 100);
    assert!(result.is_none());
}

#[test]
fn track_read_duplicate_returns_blocking_message() {
    let _guard = track_test_lock();
    set_deny_repeated_reads(true);
    untrack_read_path("dup_path");

    // First call
    let first = track_read("dup_path", 5, 50);
    assert!(first.is_none());

    // Second identical call
    let second = track_read("dup_path", 5, 50);
    assert!(second.is_some());
    let msg = second.unwrap();
    assert!(msg.contains("already read"));
    assert!(msg.contains("dup_path"));
}

#[test]
fn track_read_different_offset_not_duplicate() {
    let _guard = track_test_lock();
    set_deny_repeated_reads(true);
    untrack_read_path("diff_path");

    let first = track_read("diff_path", 0, 100);
    assert!(first.is_none());

    let second = track_read("diff_path", 10, 100);
    assert!(second.is_none());
}

#[test]
fn track_read_different_limit_not_duplicate() {
    let _guard = track_test_lock();
    set_deny_repeated_reads(true);
    untrack_read_path("diff_path2");

    let first = track_read("diff_path2", 0, 100);
    assert!(first.is_none());

    let second = track_read("diff_path2", 0, 200);
    assert!(second.is_none());
}

#[test]
fn untrack_removes_matching_path() {
    let _guard = track_test_lock();
    set_deny_repeated_reads(true);

    track_read("remove_me", 0, 10);
    untrack_read_path("remove_me");

    // After untracking, first call should be fine again
    let result = track_read("remove_me", 0, 10);
    assert!(result.is_none());
}

#[test]
fn untrack_does_not_affect_other_paths() {
    let _guard = track_test_lock();
    set_deny_repeated_reads(true);

    track_read("keep_me", 0, 10);
    track_read("unrelated", 0, 10);

    untrack_read_path("unrelated");

    // keep_me should still be tracked
    let result = track_read("keep_me", 0, 10);
    assert!(result.is_some());
}

#[test]
fn deny_repeated_reads_default() {
    let _guard = track_test_lock();
    // Default should be true (after test setup)
    // Reset to default
    set_deny_repeated_reads(true);
    assert!(deny_repeated_reads());
}

#[test]
fn set_deny_repeated_reads_toggle() {
    let _guard = track_test_lock();
    set_deny_repeated_reads(false);
    assert!(!deny_repeated_reads());

    set_deny_repeated_reads(true);
    assert!(deny_repeated_reads());
}

#[test]
fn find_files_descriptors_say_regex_not_glob() {
    use rig::tool::Tool;

    let tool = crate::agent::tools::FindFilesTool::new(None, None, 100);
    let desc = tool.description();
    assert!(
        desc.contains("regex"),
        "find_files description should say regex, got: {desc}"
    );
    assert!(
        !desc.to_lowercase().contains("glob"),
        "find_files description must not say glob, got: {desc}"
    );
    let params = tool.parameters();
    let pattern_desc = params
        .pointer("/properties/pattern/description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        pattern_desc.contains("Regex"),
        "find_files pattern param should say Regex, got: {pattern_desc}"
    );
}

#[test]
fn system_and_explore_prompts_describe_find_files_as_regex() {
    #[cfg(feature = "subagents")]
    let prompts = [
        crate::agent::prompt::SYSTEM_PROMPT,
        crate::extras::subagents::prompt::EXPLORE_PROMPT,
    ];
    #[cfg(not(feature = "subagents"))]
    let prompts = [crate::agent::prompt::SYSTEM_PROMPT];
    for prompt in prompts {
        let line = prompt
            .lines()
            .find(|l| l.contains("**find_files**"))
            .unwrap_or("");
        assert!(
            line.contains("regex"),
            "prompt find_files line should say regex, got: {line}"
        );
        assert!(
            !line.to_lowercase().contains("glob pattern"),
            "prompt find_files line must not say glob pattern, got: {line}"
        );
    }
}
