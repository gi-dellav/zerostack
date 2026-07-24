use std::collections::HashSet;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::sandbox::{ProcessGroupGuard, configure_child_lifetime, kill_process_group};

/// Result of running a hook subprocess to completion or timeout.
pub(crate) struct HookOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

/// Pure: builds the invocation for a hook command. When `args` is present,
/// uses the exec form: `command` is the program itself, run directly with
/// `args` as its argv, bypassing the shell entirely (no metacharacter
/// expansion). When absent, falls back to the shell form (`sh -c command` on
/// Unix, `powershell -Command command` on Windows).
pub(crate) fn build_shell_invocation(
    command: &str,
    args: Option<&[String]>,
) -> (String, Vec<String>) {
    match args {
        Some(args) => (command.to_string(), args.to_vec()),
        None => {
            let (shell, flag) = if cfg!(windows) {
                ("powershell", "-Command")
            } else {
                ("sh", "-c")
            };
            (
                shell.to_string(),
                vec![flag.to_string(), command.to_string()],
            )
        }
    }
}

/// Spawns the hook as a subprocess in its own process group, writes
/// `stdin_json` to its stdin then closes it, waits up to `timeout`, and on
/// timeout kills the whole process group. `async: true` handling (run in the
/// background, ignore the decision) is the caller's responsibility.
/// `project_dir` is exposed to the hook as `$ZEROSTACK_PROJECT_DIR`. `args`
/// selects the exec form (see [`build_shell_invocation`]) when present.
pub(crate) async fn run_hook(
    command: &str,
    args: Option<&[String]>,
    stdin_json: &[u8],
    timeout: std::time::Duration,
    project_dir: &str,
) -> HookOutput {
    let (program, args) = build_shell_invocation(command, args);
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd.env("ZEROSTACK_PROJECT_DIR", project_dir);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    configure_child_lifetime(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return HookOutput {
                exit_code: None,
                stdout: Vec::new(),
                stderr: format!("failed to spawn hook: {e}").into_bytes(),
                timed_out: false,
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(stdin_json).await {
            tracing::warn!("hooks: failed to write hook stdin: {e}");
        }
        drop(stdin);
    }

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let active_groups: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut guard = ProcessGroupGuard::new(child.id(), active_groups.clone());

    let run = async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(pipe) = stdout_pipe.as_mut()
            && let Err(e) = pipe.read_to_end(&mut stdout).await
        {
            tracing::warn!("hooks: failed to read hook stdout (output may be truncated): {e}");
        }
        if let Some(pipe) = stderr_pipe.as_mut()
            && let Err(e) = pipe.read_to_end(&mut stderr).await
        {
            tracing::warn!("hooks: failed to read hook stderr (output may be truncated): {e}");
        }
        let status = child.wait().await;
        (status, stdout, stderr)
    };

    match tokio::time::timeout(timeout, run).await {
        Ok((status, stdout, stderr)) => {
            guard.disarm();
            let exit_code = status.ok().and_then(|s| s.code());
            HookOutput {
                exit_code,
                stdout,
                stderr,
                timed_out: false,
            }
        }
        Err(_) => {
            if let Some(pid) = child.id() {
                kill_process_group(pid);
            }
            guard.disarm();
            HookOutput {
                exit_code: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
                timed_out: true,
            }
        }
    }
}
