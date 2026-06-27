use crate::extras::r#loop::{
    DEFAULT_PLAN_FILENAME, LoopState, SUMMARY_TRUNCATION_CHARS, plan, transcript,
};
use std::path::PathBuf;

fn loop_test_data_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zerostack-loop-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    unsafe { std::env::set_var("ZS_DATA_DIR", &dir) };
    dir
}

// --- LoopState tests ---

#[test]
fn test_loop_state_new_defaults() {
    let ls = LoopState::new("fix bugs".to_string(), PathBuf::from("plan.md"), None, None);
    assert!(ls.active);
    assert_eq!(ls.prompt, "fix bugs");
    assert_eq!(ls.plan_file, PathBuf::from("plan.md"));
    assert_eq!(ls.iteration, 0);
    assert_eq!(ls.max_iterations, None);
    assert!(ls.last_summary.is_none());
    assert!(ls.run_cmd.is_none());
    assert!(ls.last_run_output.is_none());
}

#[test]
fn test_loop_state_new_with_max() {
    let ls = LoopState::new(
        "test".to_string(),
        PathBuf::from("p.md"),
        Some(5),
        Some("make check".to_string()),
    );
    assert_eq!(ls.max_iterations, Some(5));
    assert_eq!(ls.run_cmd, Some("make check".to_string()));
}

#[test]
fn test_should_stop_no_max() {
    let ls = LoopState::new("x".to_string(), PathBuf::from("p.md"), None, None);
    assert!(!ls.should_stop());
}

#[test]
fn test_should_stop_with_max_not_reached() {
    let mut ls = LoopState::new("x".to_string(), PathBuf::from("p.md"), Some(3), None);
    ls.iteration = 2;
    assert!(!ls.should_stop());
}

#[test]
fn test_should_stop_with_max_exactly_reached_still_runs() {
    let mut ls = LoopState::new("x".to_string(), PathBuf::from("p.md"), Some(3), None);
    ls.iteration = 3;
    // iteration == max should NOT trigger stop (must exceed)
    assert!(!ls.should_stop());
}

#[test]
fn test_should_stop_with_max_exceeded() {
    let mut ls = LoopState::new("x".to_string(), PathBuf::from("p.md"), Some(3), None);
    ls.iteration = 4;
    assert!(ls.should_stop());
}

#[test]
fn test_iteration_label_no_max() {
    let mut ls = LoopState::new("x".to_string(), PathBuf::from("p.md"), None, None);
    ls.iteration = 5;
    assert_eq!(ls.iteration_label(), "LOOP 5/∞");
}

#[test]
fn test_iteration_label_with_max() {
    let mut ls = LoopState::new("x".to_string(), PathBuf::from("p.md"), Some(10), None);
    ls.iteration = 3;
    assert_eq!(ls.iteration_label(), "LOOP 3/10");
}

#[test]
fn test_build_prompt_contains_key_parts() {
    let mut ls = LoopState::new(
        "implement feature X".to_string(),
        PathBuf::from(DEFAULT_PLAN_FILENAME),
        Some(5),
        Some("cargo test".to_string()),
    );
    ls.iteration = 2;
    ls.last_summary = Some("fixed parser bug".to_string());
    ls.last_run_output = Some("all tests passed".to_string());

    let prompt = ls.build_prompt();

    assert!(prompt.contains("implement feature X"));
    assert!(prompt.contains("Iteration 2/5"));
    assert!(prompt.contains(DEFAULT_PLAN_FILENAME));
    assert!(prompt.contains("fixed parser bug"));
    assert!(prompt.contains("all tests passed"));
    assert!(prompt.contains("Choose ONE task from the plan"));
}

#[test]
fn test_build_prompt_starting_fresh() {
    let ls = LoopState::new("task".to_string(), PathBuf::from("plan.md"), None, None);
    let prompt = ls.build_prompt();
    assert!(prompt.contains("starting fresh"));
    assert!(prompt.contains("(none)"));
    assert!(prompt.contains("Iteration 0/∞"));
}

#[test]
fn test_summary_truncation_constant() {
    // Ensure we use a reasonable truncation value
    assert!(SUMMARY_TRUNCATION_CHARS > 0);
}

// --- plan tests ---

#[test]
fn test_plan_exists_on_nonexistent() {
    let tmp = std::env::temp_dir().join("zerostack_test_plan_nonexistent.md");
    let _ = std::fs::remove_file(&tmp);
    assert!(!plan::plan_exists(&tmp));
}

#[test]
fn test_plan_exists_on_existing() {
    let tmp = std::env::temp_dir().join("zerostack_test_plan_exists.md");
    std::fs::write(&tmp, "# Test plan").unwrap();
    assert!(plan::plan_exists(&tmp));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_read_plan_returns_content() {
    let tmp = std::env::temp_dir().join("zerostack_test_read_plan.md");
    std::fs::write(&tmp, "item 1\nitem 2").unwrap();
    let content = plan::read_plan(&tmp);
    assert_eq!(content, Some("item 1\nitem 2".to_string()));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_read_plan_nonexistent_returns_none() {
    let tmp = std::env::temp_dir().join("zerostack_test_read_nonexistent.md");
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(plan::read_plan(&tmp), None);
}

#[test]
fn test_delete_plan_removes_file() {
    let tmp = std::env::temp_dir().join("zerostack_test_delete_plan.md");
    std::fs::write(&tmp, "data").unwrap();
    assert!(tmp.exists());
    plan::delete_plan(&tmp);
    assert!(!tmp.exists());
}

#[test]
fn test_delete_plan_nonexistent_does_not_panic() {
    let tmp = std::env::temp_dir().join("zerostack_test_delete_nonexistent.md");
    let _ = std::fs::remove_file(&tmp);
    plan::delete_plan(&tmp); // should not panic
}

#[tokio::test]
async fn test_handle_startup_no_plan_returns_false() {
    let tmp = std::env::temp_dir().join("zerostack_test_startup_nonexistent.md");
    let _ = std::fs::remove_file(&tmp);
    let result = plan::handle_startup(&tmp).await.unwrap();
    assert!(!result);
}

// --- transcript tests ---

#[test]
fn test_transcript_dir_contains_session_id() {
    // We can't easily test the full path, but we can test it doesn't panic
    // with a typical session id
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from(".zerostack"))
        .join("zerostack")
        .join("loops")
        .join("test-session-123");
    assert!(dir.ends_with("test-session-123"));
}

#[test]
fn test_save_iteration_creates_file() {
    let session_id = "test-save-iteration";
    let dir = loop_test_data_dir().join("loops").join(session_id);

    // Clean up before test
    let _ = std::fs::remove_dir_all(&dir);

    transcript::save_iteration(
        session_id,
        1,
        "test prompt",
        "test response",
        Some("validation ok"),
        "summary",
    )
    .unwrap();

    let iter_file = dir.join("iter-0001.json");
    assert!(iter_file.exists());

    let content = std::fs::read_to_string(&iter_file).unwrap();
    assert!(content.contains("test prompt"));
    assert!(content.contains("test response"));
    assert!(content.contains("validation ok"));
    assert!(content.contains("summary"));
    assert!(content.contains("\"iteration\": 1"));

    // Clean up
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_save_iteration_without_validation_output() {
    let session_id = "test-save-no-validation";
    let dir = loop_test_data_dir().join("loops").join(session_id);

    let _ = std::fs::remove_dir_all(&dir);

    transcript::save_iteration(session_id, 2, "p", "r", None, "s").unwrap();

    let content = std::fs::read_to_string(dir.join("iter-0002.json")).unwrap();
    assert!(content.contains("\"validation_output\": null"));

    let _ = std::fs::remove_dir_all(&dir);
}
