use std::path::PathBuf;

use crate::agent::tools::bash::mask_hint_for_exit;
use crate::sandbox::{Sandbox, mask_hint};

fn home() -> PathBuf {
    PathBuf::from("/home/tester")
}

fn ssh_root() -> PathBuf {
    home().join(".ssh")
}

fn aws_root() -> PathBuf {
    home().join(".aws")
}

#[test]
fn test_masked_file_read_failure_hints() {
    let hint = mask_hint(
        "cat ~/.ssh/id_ed25519",
        "cat: /home/tester/.ssh/id_ed25519: No such file or directory",
        &[ssh_root()],
        &home(),
    );
    assert_eq!(
        hint,
        Some(
            "note: ~/.ssh is masked by the sandbox; ask the user whether it should be exposed"
                .to_string()
        )
    );
}

#[test]
fn test_absolute_path_only_in_stderr_hints() {
    // The command string uses a shell variable, so only the shell-expanded
    // absolute path in stderr names the masked root.
    let hint = mask_hint(
        "cat $CRED_FILE",
        "cat: /home/tester/.aws/credentials: No such file or directory",
        &[aws_root()],
        &home(),
    );
    assert_eq!(
        hint,
        Some(
            "note: ~/.aws is masked by the sandbox; ask the user whether it should be exposed"
                .to_string()
        )
    );
}

#[test]
fn test_multiple_hits_name_each_root_once() {
    let hint = mask_hint(
        "cat ~/.ssh/id_ed25519 ~/.ssh/id_rsa ~/.aws/credentials",
        "",
        &[ssh_root(), aws_root()],
        &home(),
    );
    assert_eq!(
        hint,
        Some(
            "note: ~/.ssh is masked by the sandbox; ask the user whether it should be exposed\n\
note: ~/.aws is masked by the sandbox; ask the user whether it should be exposed"
                .to_string()
        )
    );
}

#[test]
fn test_unrelated_failure_never_hints() {
    let hint = mask_hint(
        "cat missing.txt",
        "cat: missing.txt: No such file or directory",
        &[ssh_root(), aws_root()],
        &home(),
    );
    assert_eq!(hint, None);
}

/// A sandbox that really masks `root`, so `mask_hint_for_exit` is exercised
/// the way `bash.rs` calls it: the mask list comes out of the sandbox rather
/// than from the test, which is what puts the whole hint path (the exit-code
/// gate, the mask list, the substring match) under one call.
fn masking_sandbox(root: &std::path::Path) -> Sandbox {
    Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_mask_roots(vec![root.to_path_buf()])
}

#[test]
fn test_exit_zero_never_hints_even_when_masked_path_is_mentioned() {
    // The same command that hints on a failure must stay silent on success.
    // The gate lives in the function under test and the tool calls it
    // unconditionally, so this is the branch production takes, and the cost it
    // guards (the sandbox's `exists()` walk over the mask list) is on the
    // other side of it.
    let root = std::env::temp_dir().join(format!("zerostack-hint-{}/.ssh", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let sandbox = masking_sandbox(&root);
    let command = format!("cat {}/id_ed25519", root.display());

    assert_eq!(
        mask_hint_for_exit(0, &command, "", &sandbox),
        None,
        "exit code 0 must never produce a hint"
    );
    let hint = mask_hint_for_exit(1, &command, "", &sandbox)
        .expect("a failed command naming a masked root must hint");
    assert!(
        hint.contains(&root.display().to_string()) && hint.contains("ask the user"),
        "the hint should name the masked root and route the decision to the user: {hint}"
    );

    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn test_unsandboxed_command_never_hints() {
    // Nothing was masked, so nothing can be blamed on the mask: the mask list
    // the function pulls from the sandbox is empty when the backend is not
    // going to run.
    let root =
        std::env::temp_dir().join(format!("zerostack-hint-bare-{}/.ssh", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(false)
        .with_mask_roots(vec![root.clone()]);
    let command = format!("cat {}/id_ed25519", root.display());

    assert_eq!(
        mask_hint_for_exit(1, &command, "", &sandbox),
        None,
        "a command that ran unsandboxed read the real file, so a mask hint would be a lie"
    );

    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}
