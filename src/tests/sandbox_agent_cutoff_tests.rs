use std::path::PathBuf;

use tokio::process::Command;

use crate::sandbox::{Sandbox, essential_env};

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "zerostack-agent-cutoff-{}-{}",
        name,
        std::process::id()
    ))
}

fn args_of(cmd: &Command) -> Vec<String> {
    cmd.as_std()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// Index of `flag` in an adjacent `flag value` pair, so `--tmpfs /tmp` never
/// answers a question asked about `--tmpfs <mask root>`.
fn pair_at(args: &[String], flag: &str, value: &str) -> Option<usize> {
    args.windows(2).position(|w| w[0] == flag && w[1] == value)
}

/// Index of `flag` in an adjacent `flag src dst` triple, matching the
/// `--ro-bind-try <src> <dst>` shape the agent-socket mask emits.
fn triple_at(args: &[String], flag: &str, src: &str, dst: &str) -> Option<usize> {
    args.windows(3)
        .position(|w| w[0] == flag && w[1] == src && w[2] == dst)
}

#[test]
fn test_essential_env_excludes_ssh_agent_vars_even_when_set() {
    unsafe { std::env::set_var("SSH_AUTH_SOCK", "/tmp/zerostack-test-agent.sock") };
    unsafe { std::env::set_var("SSH_AGENT_PID", "12345") };

    let vars = essential_env();

    unsafe { std::env::remove_var("SSH_AUTH_SOCK") };
    unsafe { std::env::remove_var("SSH_AGENT_PID") };

    assert!(
        !vars.iter().any(|(k, _)| *k == "SSH_AUTH_SOCK"),
        "SSH_AUTH_SOCK must never reach the sandbox environment: {vars:?}"
    );
    assert!(
        !vars.iter().any(|(k, _)| *k == "SSH_AGENT_PID"),
        "SSH_AGENT_PID must never reach the sandbox environment: {vars:?}"
    );
}

#[test]
fn test_ssh_auth_sock_seam_emits_dev_null_bind_after_root_bind() {
    let sock = scratch_dir("sock").join("agent.sock");
    let cache_dir = scratch_dir("sock-cache");
    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_cache_dir(cache_dir.clone())
        .with_mask_roots(Vec::new())
        .with_ssh_auth_sock(Some(sock.clone()));

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let sock_str = sock.to_string_lossy();
    let root_bind = pair_at(&args, "--ro-bind", "/").expect("`/` should be ro-bound");
    let agent_bind = triple_at(&args, "--ro-bind-try", "/dev/null", &sock_str)
        .unwrap_or_else(|| panic!("expected /dev/null bound over the agent socket: {args:?}"));

    assert!(
        root_bind < agent_bind,
        "the agent socket mask must come after the `/` ro-bind: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_no_host_ssh_auth_sock_emits_no_dev_null_bind() {
    let cache_dir = scratch_dir("none-cache");
    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_cache_dir(cache_dir.clone())
        .with_mask_roots(Vec::new())
        .with_ssh_auth_sock(None);

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    assert!(
        !args.iter().any(|a| a == "/dev/null"),
        "no host SSH_AUTH_SOCK, so no /dev/null bind should be emitted: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&cache_dir);
}
