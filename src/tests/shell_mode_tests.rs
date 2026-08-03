use crate::sandbox::Sandbox;
use tokio::time::{Duration, sleep, timeout};

#[tokio::test]
async fn test_shell_mode_runs_command() {
    let sandbox = Sandbox::new(false, "bwrap");
    let mut cmd = sandbox.wrap_command("echo hello").unwrap();
    let output = cmd.output().await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "hello");
}

#[tokio::test]
async fn test_shell_mode_strips_bang_prefix() {
    let sandbox = Sandbox::new(false, "bwrap");
    // The command after stripping '!'
    let cmd_str = "echo shell_mode_works";
    let mut cmd = sandbox.wrap_command(cmd_str).unwrap();
    let output = cmd.output().await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "shell_mode_works");
}

#[tokio::test]
async fn test_shell_mode_failing_command() {
    let sandbox = Sandbox::new(false, "bwrap");
    let mut cmd = sandbox.wrap_command("exit 42").unwrap();
    let output = cmd.output().await.unwrap();
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(42));
}

#[tokio::test]
async fn test_shell_mode_stderr_included() {
    let sandbox = Sandbox::new(false, "bwrap");
    let mut cmd = sandbox.wrap_command("echo stderr_output >&2").unwrap();
    let output = cmd.output().await.unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.trim(), "stderr_output");
}

#[tokio::test]
async fn test_output_command_returns_output_when_descendant_holds_pipe() {
    let sandbox = Sandbox::new(false, "bwrap");
    let output = timeout(
        Duration::from_secs(1),
        sandbox.output_command("printf ok; sleep 2 &"),
    )
    .await
    .unwrap()
    .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, "ok");
}

#[tokio::test]
async fn test_shell_mode_timeout_kills_descendants() {
    let marker =
        std::env::temp_dir().join(format!("zerostack-abort-descendant-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);

    let sandbox = Sandbox::new(false, "bwrap");
    let command = format!(
        "sh -c 'sleep 2; printf leaked > {}' & wait",
        marker.display()
    );

    let result = timeout(Duration::from_millis(100), sandbox.output_command(&command)).await;
    assert!(result.is_err());

    sleep(Duration::from_millis(2300)).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn test_shell_mode_explicit_kill_active_kills_running_descendants() {
    let marker = std::env::temp_dir().join(format!(
        "zerostack-abort-active-descendant-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);

    let sandbox = Sandbox::new(false, "bwrap");
    let command = format!(
        "sh -c 'sleep 2; printf leaked > {}' & wait",
        marker.display()
    );
    let handle = tokio::spawn({
        let sandbox = sandbox.clone();
        async move { sandbox.output_command(&command).await }
    });

    zerostack_test_wait_until(|| sandbox.active_group_count() == 1).await;
    sandbox.kill_active();
    let _ = timeout(Duration::from_secs(1), handle).await;

    sleep(Duration::from_millis(2300)).await;
    assert!(!marker.exists());
    assert_eq!(sandbox.active_group_count(), 0);
}

async fn zerostack_test_wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !predicate() {
        assert!(std::time::Instant::now() < deadline);
        sleep(Duration::from_millis(10)).await;
    }
}
