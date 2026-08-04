use std::path::PathBuf;

use crate::agent::tools::bash::mask_hint_for_exit;
use crate::sandbox::mask_hint;

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
        Some("note: ~/.ssh is masked by sandbox; add to sandbox-expose to allow".to_string())
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
        Some("note: ~/.aws is masked by sandbox; add to sandbox-expose to allow".to_string())
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
            "note: ~/.ssh is masked by sandbox; add to sandbox-expose to allow\n\
note: ~/.aws is masked by sandbox; add to sandbox-expose to allow"
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

#[test]
fn test_exit_zero_never_hints_even_when_masked_path_is_mentioned() {
    // Same command/stderr that would hint on a failure must stay silent on
    // success: this exercises the wiring predicate's exit-code gate, not just
    // `mask_hint` itself, since a bug that dropped the gate would still pass
    // every `mask_hint`-only assertion above.
    let command = "cat ~/.ssh/id_ed25519";
    let stderr = "";
    assert_eq!(
        mask_hint_for_exit(0, command, stderr, &[ssh_root()], &home()),
        None,
        "exit code 0 must never produce a hint"
    );
    assert!(
        mask_hint_for_exit(1, command, stderr, &[ssh_root()], &home()).is_some(),
        "sanity: the same inputs must hint on a non-zero exit"
    );
}
