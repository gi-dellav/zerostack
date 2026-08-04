use std::path::PathBuf;

use tokio::process::Command;

use crate::sandbox::Sandbox;

fn scratch_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zerostack-mask-{}-{}", name, std::process::id()))
}

fn bwrap_sandbox(masks: Vec<PathBuf>, cache_dir: PathBuf) -> Sandbox {
    Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_cache_dir(cache_dir)
        .with_mask_roots(masks)
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

#[test]
fn test_existing_mask_root_is_masked_between_root_bind_and_cwd_bind() {
    let root = scratch_dir("existing");
    std::fs::create_dir_all(&root).unwrap();
    let cache_dir = scratch_dir("existing-cache");
    let sandbox = bwrap_sandbox(vec![root.clone()], cache_dir.clone());

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let cwd = std::env::current_dir().unwrap();
    let root_bind = pair_at(&args, "--ro-bind", "/").expect("`/` should be ro-bound");
    let mask = pair_at(&args, "--tmpfs", &root.to_string_lossy())
        .unwrap_or_else(|| panic!("existing mask root should be tmpfs-masked: {args:?}"));
    let cwd_bind = pair_at(&args, "--bind", &cwd.to_string_lossy())
        .expect("the working directory should be bound");

    assert!(
        root_bind < mask,
        "the mask must shadow the `/` ro-bind, so it comes after it: {args:?}"
    );
    assert!(
        mask < cwd_bind,
        "the cwd bind must shadow the mask, so it comes after it: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_nonexistent_mask_root_emits_no_tmpfs() {
    // bwrap creates `--tmpfs` mountpoints, which a read-only `/` forbids: a
    // missing entry would abort every sandboxed command on that host.
    let root = scratch_dir("missing");
    let _ = std::fs::remove_dir_all(&root);
    let cache_dir = scratch_dir("missing-cache");
    let sandbox = bwrap_sandbox(vec![root.clone()], cache_dir.clone());

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    assert!(
        pair_at(&args, "--tmpfs", &root.to_string_lossy()).is_none(),
        "a mask root missing on the host must not be mounted: {args:?}"
    );
    assert!(
        pair_at(&args, "--tmpfs", "/tmp").is_some(),
        "the `/tmp` tmpfs should still be there, or the assertion above is vacuous: {args:?}"
    );
    assert!(
        sandbox.masked_paths().is_empty(),
        "a mask root missing on the host is not a masked path"
    );

    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_zerobox_invocation_is_unchanged_by_masks() {
    let root = scratch_dir("zerobox");
    std::fs::create_dir_all(&root).unwrap();
    let sandbox = Sandbox::new(true, "zerobox")
        .with_backend_available(true)
        .with_mask_roots(vec![root.clone()]);

    let cmd = sandbox.wrap_command("echo hello").unwrap();
    assert_eq!(cmd.as_std().get_program(), "zerobox");
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        args_of(&cmd),
        vec!["--allow-write", &cwd, "--", "bash", "-c", "echo hello"],
        "zerobox exposes no mount policy, so masking must not touch its invocation"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_cwd_under_a_mask_root_reports_that_root() {
    let root = scratch_dir("shadowed");
    let cwd = root.join("nvim/lua");
    std::fs::create_dir_all(&cwd).unwrap();
    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_mask_roots(vec![root.clone()]);

    assert_eq!(sandbox.shadowed_mask_root(&cwd), Some(root.clone()));
    assert_eq!(
        sandbox.shadowed_mask_root(&root),
        Some(root.clone()),
        "a cwd that is the mask root itself is shadowed too"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_sibling_of_a_mask_root_is_not_shadowed() {
    // Component-wise containment, not string prefixes: `~/.ssh2` is not under
    // `~/.ssh`.
    let base = scratch_dir("sibling");
    let root = base.join(".ssh");
    let sibling = base.join(".ssh2");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_mask_roots(vec![root]);

    assert_eq!(sandbox.shadowed_mask_root(&sibling), None);
    assert_eq!(sandbox.shadowed_mask_root(&base), None);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn test_disabled_sandbox_masks_nothing() {
    let root = scratch_dir("disabled");
    std::fs::create_dir_all(&root).unwrap();
    let sandbox = Sandbox::new(false, "bwrap")
        .with_backend_available(true)
        .with_mask_roots(vec![root.clone()]);

    assert!(sandbox.masked_paths().is_empty());
    assert_eq!(sandbox.shadowed_mask_root(&root), None);

    let _ = std::fs::remove_dir_all(&root);
}
