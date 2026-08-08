use std::path::PathBuf;

use crate::cli::Cli;
use crate::config::Config;
use crate::sandbox::partition_expose;
use crate::tests::sandbox_support::{args_of, bwrap_sandbox, pair_at, scratch_dir, triple_at};

fn dir(name: &str) -> PathBuf {
    scratch_dir("expose", name)
}

// --- Resolver: CLI replaces config wholesale ---

#[test]
fn test_resolve_sandbox_expose_cli_replaces_config_wholesale() {
    let cli = Cli {
        sandbox_expose: vec!["~/.ssh".to_string()],
        ..Default::default()
    };
    let cfg = Config {
        sandbox_expose: Some(vec!["~/.aws".to_string()]),
        ..Default::default()
    };
    assert_eq!(
        cli.resolve_sandbox_expose(&cfg),
        vec!["~/.ssh".to_string()],
        "a non-empty CLI list must replace the config list wholesale, not merge with it"
    );
}

#[test]
fn test_resolve_sandbox_expose_falls_back_to_config() {
    let cli = Cli::default();
    let cfg = Config {
        sandbox_expose: Some(vec!["~/.aws".to_string()]),
        ..Default::default()
    };
    assert_eq!(cli.resolve_sandbox_expose(&cfg), vec!["~/.aws".to_string()]);
}

#[test]
fn test_resolve_sandbox_expose_defaults_to_empty() {
    let cli = Cli::default();
    let cfg = Config::default();
    assert!(cli.resolve_sandbox_expose(&cfg).is_empty());
}

// --- Partition: validation against the mask list ---

#[test]
fn test_partition_accepts_exact_mask_root() {
    let home = dir("home-exact");
    let ssh = home.join(".ssh");
    let raw = vec!["~/.ssh".to_string()];

    let (valid, rejected) = partition_expose(&raw, std::slice::from_ref(&ssh), Some(&home));

    assert_eq!(valid, vec![ssh]);
    assert!(
        rejected.is_empty(),
        "exact mask root must be accepted: {rejected:?}"
    );
}

#[test]
fn test_partition_accepts_subpath_of_mask_root() {
    let home = dir("home-subpath");
    let ssh = home.join(".ssh");
    let raw = vec!["~/.ssh/known_hosts".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh], Some(&home));

    assert_eq!(valid, vec![home.join(".ssh/known_hosts")]);
    assert!(
        rejected.is_empty(),
        "a subpath of a mask root must be accepted: {rejected:?}"
    );
}

#[test]
fn test_partition_rejects_path_outside_mask_list() {
    let home = dir("home-outside");
    let ssh = home.join(".ssh");
    let raw = vec!["/etc".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh], Some(&home));

    assert!(
        valid.is_empty(),
        "a path outside the mask list must not be exposed: {valid:?}"
    );
    assert_eq!(rejected, vec!["/etc".to_string()]);
}

#[test]
fn test_partition_rejects_sibling_component_trap() {
    // Component-wise containment, not string prefixes: `~/.ssh2` is not under
    // `~/.ssh`, even though the string "~/.ssh" is a text prefix of it.
    let home = dir("home-sibling");
    let ssh = home.join(".ssh");
    let raw = vec!["~/.ssh2".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh], Some(&home));

    assert!(
        valid.is_empty(),
        "`~/.ssh2` must not pass as a subpath of `~/.ssh`: {valid:?}"
    );
    assert_eq!(rejected, vec!["~/.ssh2".to_string()]);
}

#[test]
fn test_partition_accepts_dollar_home_spelling() {
    // Every other path-taking config key accepts `$HOME/...`; expose rejecting
    // it would be a trap with no reason behind it.
    let home = dir("home-dollar");
    let ssh = home.join(".ssh");
    let raw = vec!["$HOME/.ssh".to_string()];

    let (valid, rejected) = partition_expose(&raw, std::slice::from_ref(&ssh), Some(&home));

    assert_eq!(valid, vec![ssh]);
    assert!(
        rejected.is_empty(),
        "`$HOME/.ssh` names the same directory as `~/.ssh`: {rejected:?}"
    );
}

#[test]
fn test_partition_rejects_parent_dir_escape_to_home() {
    // `~/.ssh/..` passes a component-wise subpath test while naming the whole
    // home directory, which would re-bind everything the masks hide.
    let home = dir("home-escape");
    let ssh = home.join(".ssh");
    let raw = vec!["~/.ssh/..".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh], Some(&home));

    assert!(
        valid.is_empty(),
        "a `..` component must never be exposed: {valid:?}"
    );
    assert_eq!(rejected, raw);
}

#[test]
fn test_partition_rejects_parent_dir_escape_into_a_sibling_mask() {
    let home = dir("home-escape-sibling");
    let ssh = home.join(".ssh");
    let aws = home.join(".aws");
    let raw = vec!["~/.ssh/../.aws".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh, aws], Some(&home));

    assert!(
        valid.is_empty(),
        "the direct spelling is the way to expose another mask root: {valid:?}"
    );
    assert_eq!(rejected, raw);
}

#[test]
fn test_partition_rejects_parent_dir_escape_out_of_home() {
    let home = dir("home-escape-out");
    let ssh = home.join(".ssh");
    let raw = vec!["~/.ssh/../../../etc".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh], Some(&home));

    assert!(
        valid.is_empty(),
        "climbing out of the mask list must be rejected like any other outside value: {valid:?}"
    );
    assert_eq!(rejected, raw);
}

#[test]
fn test_partition_expose_with_no_home_leaves_tilde_form_unexpanded() {
    // `home: None` models a host with no home directory. Per
    // `expand_tilde_with_home`'s documented behavior, a `~` form must be
    // returned unchanged rather than expanded against a manufactured empty
    // path, so it can never accidentally satisfy a `mask_roots` entry.
    let ssh = PathBuf::from("/nonexistent-home/.ssh");
    let raw = vec!["~/.ssh".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh], None);

    assert!(
        valid.is_empty(),
        "a `~` form must not expand against a missing home: {valid:?}"
    );
    assert_eq!(rejected, raw);
}

#[test]
fn test_partition_expose_with_no_home_still_accepts_an_absolute_mask_root() {
    // The `None` home only affects `~`/`$HOME` expansion; an already-absolute
    // value must still validate normally, so `None` is not a blanket
    // rejection of everything.
    let root = PathBuf::from("/nonexistent-home/.ssh");
    let raw = vec!["/nonexistent-home/.ssh".to_string()];

    let (valid, rejected) = partition_expose(&raw, std::slice::from_ref(&root), None);

    assert_eq!(valid, vec![root]);
    assert!(
        rejected.is_empty(),
        "an absolute path needs no home to validate: {rejected:?}"
    );
}

// --- Shared construction: one validation path for every entry point ---

#[test]
fn test_build_sandbox_returns_the_rejected_value_warning() {
    let setup = crate::sandbox::build_sandbox(&crate::sandbox::SandboxSettings {
        enabled: true,
        required: false,
        backend: "bwrap",
        shell: "bash",
        expose: &["/etc".to_string()],
        network: true,
    });

    assert!(
        setup
            .warnings
            .iter()
            .any(|w| w.contains("sandbox-expose value '/etc'")),
        "the out-of-list value must be reported verbatim to whoever logs it: {:?}",
        setup.warnings
    );
}

#[test]
fn test_build_sandbox_is_quiet_without_expose_values() {
    let setup = crate::sandbox::build_sandbox(&crate::sandbox::SandboxSettings {
        enabled: true,
        required: false,
        backend: "bwrap",
        shell: "bash",
        expose: &[],
        network: true,
    });

    assert!(
        !setup
            .warnings
            .iter()
            .any(|w| w.contains("sandbox-expose value")),
        "no expose values, so nothing to warn about: {:?}",
        setup.warnings
    );
}

// --- Arg assembly: --ro-bind-try after masks, before the cwd bind ---

#[test]
fn test_expose_emits_ro_bind_try_between_masks_and_cwd_bind() {
    let root = dir("expose-arg-assembly");
    std::fs::create_dir_all(&root).unwrap();
    let cache_dir = dir("expose-arg-assembly-cache");

    let sandbox =
        bwrap_sandbox(vec![root.clone()], cache_dir.clone()).with_expose(vec![root.clone()]);

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let root_str = root.to_string_lossy();
    let cwd = std::env::current_dir().unwrap();

    let mask = pair_at(&args, "--tmpfs", &root_str)
        .unwrap_or_else(|| panic!("expected the mask tmpfs to still be emitted: {args:?}"));
    let expose = triple_at(&args, "--ro-bind-try", &root_str, &root_str)
        .unwrap_or_else(|| panic!("expected --ro-bind-try for the exposed path: {args:?}"));
    let cwd_bind = pair_at(&args, "--bind", &cwd.to_string_lossy())
        .expect("the working directory should be bound");

    assert!(
        mask < expose,
        "expose must come after the mask tmpfs: {args:?}"
    );
    assert!(
        expose < cwd_bind,
        "expose must come before the cwd bind: {args:?}"
    );
    assert!(
        pair_at(&args, "--bind", &root_str).is_none(),
        "expose must never grant write access via plain --bind: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_no_expose_emits_no_ro_bind_try() {
    let root = dir("no-expose");
    std::fs::create_dir_all(&root).unwrap();
    let cache_dir = dir("no-expose-cache");

    let sandbox = bwrap_sandbox(vec![root.clone()], cache_dir.clone());

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let root_str = root.to_string_lossy();
    assert!(
        triple_at(&args, "--ro-bind-try", &root_str, &root_str).is_none(),
        "no expose configured, so no --ro-bind-try for the mask root should appear: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache_dir);
}
