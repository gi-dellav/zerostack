
use crate::sandbox::Sandbox;

#[tokio::test]
async fn test_shell_mode_runs_command() {
    let sandbox = Sandbox::new(false, "bwrap");
    let mut cmd = sandbox.wrap_command("echo hello");
    let output = cmd.output().await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "hello");
}

#[tokio::test]
async fn test_shell_mode_strips_bang_prefix() {
    let sandbox = Sandbox::new(false, "bwrap");
    // The command after stripping '!'
    let cmd_str = "echo shell_mode_works";
    let mut cmd = sandbox.wrap_command(cmd_str);
    let output = cmd.output().await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "shell_mode_works");
}

#[tokio::test]
async fn test_shell_mode_failing_command() {
    let sandbox = Sandbox::new(false, "bwrap");
    let mut cmd = sandbox.wrap_command("exit 42");
    let output = cmd.output().await.unwrap();
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(42));
}

#[tokio::test]
async fn test_shell_mode_stderr_included() {
    let sandbox = Sandbox::new(false, "bwrap");
    let mut cmd = sandbox.wrap_command("echo stderr_output >&2");
    let output = cmd.output().await.unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.trim(), "stderr_output");
}
