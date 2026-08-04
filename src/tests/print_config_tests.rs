use crate::print::format_sandbox_expose_display;
use crate::sandbox::partition_expose;

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
