use crate::extras::hooks::subprocess::{build_shell_invocation, run_hook};
use tokio::time::Duration;

#[test]
fn build_shell_invocation_wraps_command_in_sh_c() {
    let (program, args) = build_shell_invocation("echo hi", None);
    assert_eq!(program, "sh");
    assert_eq!(args, vec!["-c".to_string(), "echo hi".to_string()]);
}

#[test]
fn build_shell_invocation_uses_exec_form_when_args_present() {
    let extra = vec!["hello".to_string(), "world".to_string()];
    let (program, args) = build_shell_invocation("echo", Some(&extra));
    assert_eq!(program, "echo");
    assert_eq!(args, vec!["hello".to_string(), "world".to_string()]);
}

#[test]
fn build_shell_invocation_uses_shell_form_when_args_absent() {
    let (program, args) = build_shell_invocation("echo hi", None);
    assert_eq!(program, "sh");
    assert_eq!(args, vec!["-c".to_string(), "echo hi".to_string()]);
}

#[tokio::test]
async fn run_hook_echoes_stdin_and_completes() {
    let output = run_hook("cat", None, b"hello", Duration::from_secs(2), "/repo").await;
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(output.stdout, b"hello");
    assert!(!output.timed_out);
}

#[tokio::test]
async fn run_hook_reports_nonzero_exit_code() {
    let output = run_hook("exit 7", None, b"", Duration::from_secs(2), "/repo").await;
    assert_eq!(output.exit_code, Some(7));
    assert!(!output.timed_out);
}

#[tokio::test]
async fn run_hook_times_out_and_kills_group() {
    let marker = std::env::temp_dir().join(format!(
        "zerostack-hook-subprocess-timeout-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker);
    let command = format!(
        "sh -c 'sleep 2; printf leaked > {}' & wait",
        marker.display()
    );

    let output = run_hook(&command, None, b"", Duration::from_millis(100), "/repo").await;
    assert!(output.timed_out);

    tokio::time::sleep(Duration::from_millis(2300)).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn run_hook_exposes_zerostack_project_dir_env_var() {
    let output = run_hook(
        "echo \"$ZEROSTACK_PROJECT_DIR\"",
        None,
        b"",
        Duration::from_secs(2),
        "/repo/project",
    )
    .await;
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "/repo/project"
    );
}

#[tokio::test]
async fn run_hook_exec_form_bypasses_the_shell() {
    // In exec form the arg is passed literally to the program, with no shell
    // metacharacter expansion (a shell would expand "$HOME" or "*").
    let args = vec!["$HOME literally".to_string()];
    let output = run_hook("echo", Some(&args), b"", Duration::from_secs(2), "/repo").await;
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "$HOME literally"
    );
}
