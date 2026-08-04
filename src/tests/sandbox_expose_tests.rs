use std::path::PathBuf;

use tokio::process::Command;

use crate::cli::Cli;
use crate::config::Config;
use crate::sandbox::{Sandbox, partition_expose};

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zerostack-expose-{}-{}", name, std::process::id()))
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
/// `--ro-bind-try <path> <path>` shape expose emits.
fn triple_at(args: &[String], flag: &str, src: &str, dst: &str) -> Option<usize> {
    args.windows(3)
        .position(|w| w[0] == flag && w[1] == src && w[2] == dst)
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
    let home = scratch_dir("home-exact");
    let ssh = home.join(".ssh");
    let raw = vec!["~/.ssh".to_string()];

    let (valid, rejected) = partition_expose(&raw, std::slice::from_ref(&ssh), &home);

    assert_eq!(valid, vec![ssh]);
    assert!(
        rejected.is_empty(),
        "exact mask root must be accepted: {rejected:?}"
    );
}

#[test]
fn test_partition_accepts_subpath_of_mask_root() {
    let home = scratch_dir("home-subpath");
    let ssh = home.join(".ssh");
    let raw = vec!["~/.ssh/known_hosts".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh], &home);

    assert_eq!(valid, vec![home.join(".ssh/known_hosts")]);
    assert!(
        rejected.is_empty(),
        "a subpath of a mask root must be accepted: {rejected:?}"
    );
}

#[test]
fn test_partition_rejects_path_outside_mask_list() {
    let home = scratch_dir("home-outside");
    let ssh = home.join(".ssh");
    let raw = vec!["/etc".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh], &home);

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
    let home = scratch_dir("home-sibling");
    let ssh = home.join(".ssh");
    let raw = vec!["~/.ssh2".to_string()];

    let (valid, rejected) = partition_expose(&raw, &[ssh], &home);

    assert!(
        valid.is_empty(),
        "`~/.ssh2` must not pass as a subpath of `~/.ssh`: {valid:?}"
    );
    assert_eq!(rejected, vec!["~/.ssh2".to_string()]);
}

// --- Arg assembly: --ro-bind-try after masks, before the cwd bind ---

#[test]
fn test_expose_emits_ro_bind_try_between_masks_and_cwd_bind() {
    let root = scratch_dir("expose-arg-assembly");
    std::fs::create_dir_all(&root).unwrap();
    let cache_dir = scratch_dir("expose-arg-assembly-cache");

    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_cache_dir(cache_dir.clone())
        .with_mask_roots(vec![root.clone()])
        .with_expose(vec![root.clone()]);

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
    let root = scratch_dir("no-expose");
    std::fs::create_dir_all(&root).unwrap();
    let cache_dir = scratch_dir("no-expose-cache");

    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_cache_dir(cache_dir.clone())
        .with_mask_roots(vec![root.clone()]);

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let root_str = root.to_string_lossy();
    assert!(
        triple_at(&args, "--ro-bind-try", &root_str, &root_str).is_none(),
        "no expose configured, so no --ro-bind-try for the mask root should appear: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache_dir);
}
