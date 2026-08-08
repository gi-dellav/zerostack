use std::collections::HashMap;
use std::path::PathBuf;

use crate::sandbox::essential_env_from;
use crate::tests::sandbox_support::{args_of, bwrap_sandbox, pair_at, scratch_dir, triple_at};

fn dir(name: &str) -> PathBuf {
    scratch_dir("agent-cutoff", name)
}

#[test]
fn test_essential_env_excludes_ssh_agent_vars_even_when_set() {
    // Through the seam, not `set_var`: libtest runs these on a thread pool
    // where sibling tests read the process environment (`SSH_AUTH_SOCK`,
    // `XDG_CONFIG_HOME`, `PATH`), and mutating it under them is the data race
    // `set_var` is unsafe for, on top of clobbering whatever the developer had
    // set.
    let env: HashMap<&str, &str> = HashMap::from([
        ("SSH_AUTH_SOCK", "/tmp/zerostack-test-agent.sock"),
        ("SSH_AGENT_PID", "12345"),
        ("PATH", "/usr/bin:/bin"),
    ]);
    let vars = essential_env_from(|name| env.get(name).map(|value| value.to_string()));

    assert!(
        vars.contains(&("PATH", "/usr/bin:/bin".to_string())),
        "sanity: the seam forwards what is on the list, or the assertions below are vacuous: {vars:?}"
    );
    assert!(
        !vars.iter().any(|(k, _)| *k == "SSH_AUTH_SOCK"),
        "SSH_AUTH_SOCK must never reach the sandbox environment: {vars:?}"
    );
    assert!(
        !vars.iter().any(|(k, _)| *k == "SSH_AGENT_PID"),
        "SSH_AGENT_PID must never reach the sandbox environment: {vars:?}"
    );
}

/// Creates a stand-in for a live agent socket: `wrap_command` only masks a
/// socket path that exists on the host, so the file has to be real.
fn touch_socket(dir: &std::path::Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let sock = dir.join(name);
    std::fs::write(&sock, b"").unwrap();
    sock
}

#[test]
fn test_ssh_auth_sock_seam_emits_dev_null_bind_after_root_bind() {
    let sock_dir = dir("sock");
    let sock = touch_socket(&sock_dir, "agent.sock");
    let cache_dir = dir("sock-cache");
    let sandbox =
        bwrap_sandbox(Vec::new(), cache_dir.clone()).with_ssh_auth_sock(Some(sock.clone()));

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let sock_str = sock.to_string_lossy();
    let root_bind = pair_at(&args, "--ro-bind", "/").expect("`/` should be ro-bound");
    let agent_bind = triple_at(&args, "--ro-bind-try", "/dev/null", &sock_str)
        .unwrap_or_else(|| panic!("expected /dev/null bound over the agent socket: {args:?}"));

    assert!(
        root_bind < agent_bind,
        "the agent socket mask must come after the `/` ro-bind: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&sock_dir);
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_stale_ssh_auth_sock_emits_no_dev_null_bind() {
    // A dead agent leaves SSH_AUTH_SOCK pointing at a path that no longer
    // exists (routine in long-lived tmux sessions). Binding over it would make
    // bwrap create the destination on the read-only `/` bind, which fails with
    // EROFS and aborts every command in the session.
    let sock = dir("stale").join("agent.sock");
    let _ = std::fs::remove_dir_all(dir("stale"));
    let cache_dir = dir("stale-cache");
    let sandbox =
        bwrap_sandbox(Vec::new(), cache_dir.clone()).with_ssh_auth_sock(Some(sock.clone()));

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    assert!(
        triple_at(&args, "--ro-bind-try", "/dev/null", &sock.to_string_lossy()).is_none(),
        "a socket path missing on the host must not be bound over: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_empty_ssh_auth_sock_emits_no_dev_null_bind() {
    // `SSH_AUTH_SOCK=` is a common way to disable forwarding; it names no path
    // at all, so there is nothing to mask.
    let cache_dir = dir("empty-cache");
    let sandbox =
        bwrap_sandbox(Vec::new(), cache_dir.clone()).with_ssh_auth_sock(Some(PathBuf::new()));

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    assert!(
        !args.iter().any(|a| a == "/dev/null"),
        "an empty SSH_AUTH_SOCK names no socket, so no bind should be emitted: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_agent_socket_mask_survives_exposing_the_directory_holding_it() {
    // `sandbox-expose ~/.gnupg` re-binds the host directory on top of the
    // mask, which brings the gpg-agent SSH socket inside it back: the agent
    // cutoff has no re-enable switch, so the socket bind must come after every
    // expose bind that could re-open it.
    let gnupg = dir("gnupg");
    let sock = touch_socket(&gnupg, "S.gpg-agent.ssh");
    let cache_dir = dir("gnupg-cache");
    let sandbox = bwrap_sandbox(vec![gnupg.clone()], cache_dir.clone())
        .with_expose(vec![gnupg.clone()])
        .with_ssh_auth_sock(Some(sock.clone()));

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let gnupg_str = gnupg.to_string_lossy();
    let expose = triple_at(&args, "--ro-bind-try", &gnupg_str, &gnupg_str)
        .unwrap_or_else(|| panic!("expected the exposed directory to be re-bound: {args:?}"));
    let agent_bind = triple_at(&args, "--ro-bind-try", "/dev/null", &sock.to_string_lossy())
        .unwrap_or_else(|| panic!("expected /dev/null bound over the agent socket: {args:?}"));

    assert!(
        expose < agent_bind,
        "the agent socket mask must be the last word, after the expose bind: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&gnupg);
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_no_host_ssh_auth_sock_emits_no_dev_null_bind() {
    let cache_dir = dir("none-cache");
    let sandbox = bwrap_sandbox(Vec::new(), cache_dir.clone()).with_ssh_auth_sock(None);

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    assert!(
        !args.iter().any(|a| a == "/dev/null"),
        "no host SSH_AUTH_SOCK, so no /dev/null bind should be emitted: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&cache_dir);
}
