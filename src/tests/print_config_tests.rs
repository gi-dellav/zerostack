use crate::print::{format_sandbox_expose_display, format_sandbox_network_display};
use crate::sandbox::{Sandbox, partition_expose};

/// The `sandbox-network` row as `--print-config` builds it: from a sandbox
/// carrying the resolved settings, with the backend probe pinned so the
/// answer does not change with whether the developer's host has bwrap.
fn network_row(enabled: bool, backend: &str, network: bool, backend_available: bool) -> String {
    format_sandbox_network_display(
        Sandbox::new(enabled, backend)
            .with_network(network)
            .with_backend_available(backend_available)
            .network_effect(),
    )
}

/// The `sandbox-network` row reports what is in effect, the same way the
/// `sandbox-expose` row does: `sandbox-network = false` is a bwrap policy, so
/// with the sandbox off, another backend selected, or bwrap not installed it
/// does nothing, and a bare `false` would read as a promise the session is
/// not keeping.
#[test]
fn sandbox_network_display_open_network_is_plain_true() {
    assert_eq!(network_row(true, "bwrap", true, true), "true");
    assert_eq!(network_row(false, "bwrap", true, true), "true");
    // An open network is open however the rest of the session is configured,
    // so the row never annotates `true`.
    assert_eq!(network_row(true, "bwrap", true, false), "true");
    assert_eq!(network_row(true, "zerobox", true, true), "true");
}

#[test]
fn sandbox_network_display_effective_when_sandboxed_under_bwrap() {
    assert_eq!(network_row(true, "bwrap", false, true), "false");
}

#[test]
fn sandbox_network_display_flags_a_disabled_sandbox() {
    assert_eq!(
        network_row(false, "bwrap", false, true),
        "false (no effect: sandbox is off)"
    );
}

#[test]
fn sandbox_network_display_flags_the_zerobox_backend() {
    assert_eq!(
        network_row(true, "zerobox", false, true),
        "false (no effect: bwrap backend only)"
    );
}

/// The variant the row used to be missing, and the one that mattered most: on
/// a host with no bwrap the sandbox falls back to running commands bare, with
/// the network intact, and a row printing a plain `false` told the user their
/// commands were offline while every one of them was online. The reason the
/// row is rendered from a sandbox rather than from resolved booleans is that
/// this question is a probe, not a setting.
#[test]
fn sandbox_network_display_flags_a_missing_backend() {
    assert_eq!(
        network_row(true, "bwrap", false, false),
        "false (no effect: bwrap is not installed)"
    );
}

/// The sandbox being off wins over the backend also being missing: with
/// nothing wrapping the command, which binary is on `PATH` is not the thing
/// the user needs to fix first.
#[test]
fn sandbox_network_display_prefers_the_disabled_sandbox_reason() {
    assert_eq!(
        network_row(false, "bwrap", false, false),
        "false (no effect: sandbox is off)"
    );
}

#[test]
fn sandbox_expose_display_empty() {
    assert_eq!(format_sandbox_expose_display(&[]), "(none)");
}

#[test]
fn sandbox_expose_display_multi_value() {
    let values = vec!["~/.ssh/known_hosts".to_string(), "~/.aws".to_string()];
    assert_eq!(
        format_sandbox_expose_display(&values),
        "~/.ssh/known_hosts, ~/.aws"
    );
}

/// `--print-config` must report the *effective* sandbox-expose list, the
/// same one `build_sandbox` would actually apply, not the raw CLI/config
/// values: a value that is not a masked entry or subpath of one (and so gets
/// rejected by `partition_expose`) must never appear in the printed row,
/// even though it is still present in the unvalidated input.
#[test]
fn sandbox_expose_display_omits_rejected_values() {
    let home = std::env::temp_dir().join(format!(
        "zerostack-print-config-expose-{}",
        std::process::id()
    ));
    let ssh = home.join(".ssh");
    let raw = vec!["~/.ssh".to_string(), "/etc".to_string()];

    let (valid, rejected) = partition_expose(&raw, std::slice::from_ref(&ssh), Some(&home));
    let display = format_sandbox_expose_display(
        &valid
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>(),
    );

    assert_eq!(display, ssh.display().to_string());
    assert_eq!(
        rejected,
        vec!["/etc".to_string()],
        "the rejected value must still be reported to whoever logs the startup warning"
    );
}

/// When every raw value is rejected, the row must fall back to the same
/// `(none)` rendering as an empty list, not print the rejected values as if
/// they were in effect.
#[test]
fn sandbox_expose_display_all_rejected_renders_none() {
    let home = std::env::temp_dir().join(format!(
        "zerostack-print-config-expose-none-{}",
        std::process::id()
    ));
    let ssh = home.join(".ssh");
    let raw = vec!["/etc".to_string()];

    let (valid, _rejected) = partition_expose(&raw, std::slice::from_ref(&ssh), Some(&home));
    let display = format_sandbox_expose_display(
        &valid
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>(),
    );

    assert_eq!(display, "(none)");
}
