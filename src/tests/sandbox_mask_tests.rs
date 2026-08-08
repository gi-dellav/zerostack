use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use crate::sandbox::{Sandbox, mask_roots_for};
use crate::tests::sandbox_support::{args_of, bwrap_sandbox, pair_at, scratch_dir};

fn dir(name: &str) -> PathBuf {
    scratch_dir("mask", name)
}

/// A scratch directory *inside* the working directory, which is the directory
/// `wrap_command` binds read-write: a mask root under it is the case where
/// that bind would otherwise re-open the mask. `target/` keeps the churn out
/// of the tracked tree.
fn cwd_scratch_dir(name: &str) -> PathBuf {
    std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("zerostack-mask-{}-{}", name, std::process::id()))
}

/// Every mount this argument list places *at* `dst`, in order, as
/// `(index, flag)`. Later mounts shadow earlier ones, so the last entry is
/// what a sandboxed command actually sees there, which is the property the
/// mask invariant is about.
fn mounts_at(args: &[String], dst: &str) -> Vec<(usize, String)> {
    let mut mounts = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arity = match args[i].as_str() {
            "--tmpfs" | "--dev" | "--proc" | "--dir" => 2,
            "--bind" | "--bind-try" | "--ro-bind" | "--ro-bind-try" | "--dev-bind" => 3,
            _ => {
                i += 1;
                continue;
            }
        };
        if args.get(i + arity - 1).is_some_and(|arg| arg == dst) {
            mounts.push((i, args[i].clone()));
        }
        i += arity;
    }
    mounts
}

fn last_mount_at(args: &[String], dst: &Path) -> (usize, String) {
    mounts_at(args, &dst.to_string_lossy())
        .pop()
        .unwrap_or_else(|| panic!("expected something mounted at {}: {args:?}", dst.display()))
}

#[test]
fn test_existing_mask_root_is_masked_between_root_bind_and_cwd_bind() {
    let root = dir("existing");
    std::fs::create_dir_all(&root).unwrap();
    let cache_dir = dir("existing-cache");
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
    let root = dir("missing");
    let _ = std::fs::remove_dir_all(&root);
    let cache_dir = dir("missing-cache");
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
        sandbox.masked_roots().is_empty(),
        "a mask root missing on the host is not a masked path"
    );

    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_zerobox_invocation_is_unchanged_by_masks() {
    let root = dir("zerobox");
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
    let root = dir("shadowed");
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
    let base = dir("sibling");
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
fn test_cwd_reached_through_a_symlinked_home_is_shadowed() {
    // Same mismatch on the warning side: the working directory arrives from
    // `getcwd(3)` fully resolved while the mask root is spelled through the
    // `/home -> /data/home` symlink, so a lexical test reports no shadowing and
    // the user is never told the project bind re-opens part of the mask.
    let base = dir("symlinked-home-shadow");
    let _ = std::fs::remove_dir_all(&base);
    let real_home = base.join("data/home/tester");
    let project = real_home.join(".gnupg/notes");
    std::fs::create_dir_all(&project).unwrap();
    let elsewhere = real_home.join("notes");
    std::fs::create_dir_all(&elsewhere).unwrap();
    symlink(base.join("data/home"), base.join("home")).unwrap();
    let root = base.join("home/tester/.gnupg");
    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_mask_roots(vec![root.clone()]);

    assert_eq!(
        sandbox.shadowed_mask_root(&project.canonicalize().unwrap()),
        Some(root.clone()),
        "a project inside the mask root's target is shadowed by the project bind, however the root is spelled"
    );
    assert_eq!(
        sandbox.shadowed_mask_root(&elsewhere.canonicalize().unwrap()),
        None,
        "resolving the paths must not turn the containment test into a blanket yes"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn test_mask_root_under_the_cwd_is_remasked_after_the_project_bind() {
    // Running zerostack from `$HOME` puts every credential directory under the
    // read-write project bind, which is emitted after the masks: without a
    // second mask layer they come back, and writable at that.
    let root = cwd_scratch_dir("under-cwd");
    std::fs::create_dir_all(&root).unwrap();
    let cache_dir = dir("under-cwd-cache");
    let sandbox = bwrap_sandbox(vec![root.clone()], cache_dir.clone());

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let cwd = std::env::current_dir().unwrap();
    let cwd_bind = pair_at(&args, "--bind", &cwd.to_string_lossy())
        .expect("the working directory should be bound");
    let (index, flag) = last_mount_at(&args, &root);

    assert_eq!(
        flag, "--tmpfs",
        "the tmpfs must be the last mount at a mask root the project bind swallows, or the sandbox gets a writable credential directory: {args:?}"
    );
    assert!(
        index > cwd_bind,
        "the re-mask must come after the project bind to shadow it: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_mask_root_under_the_cache_bind_is_remasked() {
    // Same shape via the other read-write bind: the cache directory is bound
    // after the masks too.
    let cache_dir = dir("cache-swallow");
    let root = cache_dir.join(".aws");
    std::fs::create_dir_all(&root).unwrap();
    let sandbox = bwrap_sandbox(vec![root.clone()], cache_dir.clone());

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let cache_bind = pair_at(&args, "--bind", &cache_dir.to_string_lossy())
        .expect("the cache directory should be bound");
    let (index, flag) = last_mount_at(&args, &root);

    assert_eq!(
        flag, "--tmpfs",
        "the tmpfs must be the last mount at a mask root the cache bind swallows: {args:?}"
    );
    assert!(
        index > cache_bind,
        "the re-mask must come after the cache bind to shadow it: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_mask_root_symlinked_into_the_project_is_remasked() {
    // `~/.ssh -> ~/dotfiles/ssh` with the project at `~/dotfiles`: the tmpfs
    // lands on the resolved path, which is inside the read-write project bind,
    // so the bind re-opens it writable. Spelled as written the mask root does
    // not start with the project path at all, so only a symlink-resolved
    // containment test sees the collision.
    let real = cwd_scratch_dir("dotfiles");
    std::fs::create_dir_all(real.join("ssh")).unwrap();
    let home = dir("symlinked-ssh");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let root = home.join(".ssh");
    symlink(real.join("ssh"), &root).unwrap();
    let cache_dir = dir("symlinked-ssh-cache");
    let sandbox = bwrap_sandbox(vec![root.clone()], cache_dir.clone());

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let cwd = std::env::current_dir().unwrap();
    let cwd_bind = pair_at(&args, "--bind", &cwd.to_string_lossy())
        .expect("the working directory should be bound");
    let (index, flag) = last_mount_at(&args, &root);

    assert_eq!(
        flag, "--tmpfs",
        "a mask root whose target the project bind swallows must end up masked: {args:?}"
    );
    assert!(
        index > cwd_bind,
        "the re-mask must come after the project bind, or the symlinked credential directory is writable inside the sandbox: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&real);
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_mask_root_under_a_symlinked_home_is_remasked() {
    // The other shape: `$HOME` itself traverses a symlink (`/home ->
    // /data/home`, routine on NFS and LVM layouts), so every mask root is
    // spelled through it while the bind, coming from `getcwd(3)`, is already
    // resolved. Modelled on the cache bind, which the seams let a test place
    // anywhere.
    let base = dir("symlinked-home");
    let _ = std::fs::remove_dir_all(&base);
    let real_home = base.join("data/home/tester");
    std::fs::create_dir_all(real_home.join(".aws")).unwrap();
    symlink(base.join("data/home"), base.join("home")).unwrap();
    let root = base.join("home/tester/.aws");
    let sandbox = bwrap_sandbox(vec![root.clone()], real_home.clone());

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let cache_bind = pair_at(&args, "--bind", &real_home.to_string_lossy())
        .expect("the cache directory should be bound");
    let (index, flag) = last_mount_at(&args, &root);

    assert_eq!(
        flag, "--tmpfs",
        "a mask root reached through a symlinked home must end up masked: {args:?}"
    );
    assert!(
        index > cache_bind,
        "the re-mask must come after the read-write bind that contains the resolved root: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn test_expose_under_a_remasked_root_stays_visible_read_only() {
    let root = cwd_scratch_dir("under-cwd-expose");
    std::fs::create_dir_all(&root).unwrap();
    let exposed = root.join("known_hosts");
    std::fs::write(&exposed, b"").unwrap();
    let cache_dir = dir("under-cwd-expose-cache");
    let sandbox =
        bwrap_sandbox(vec![root.clone()], cache_dir.clone()).with_expose(vec![exposed.clone()]);

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let cwd = std::env::current_dir().unwrap();
    let cwd_bind = pair_at(&args, "--bind", &cwd.to_string_lossy())
        .expect("the working directory should be bound");
    let (mask, mask_flag) = last_mount_at(&args, &root);
    let (expose, expose_flag) = last_mount_at(&args, &exposed);

    assert_eq!(mask_flag, "--tmpfs");
    assert_eq!(
        expose_flag, "--ro-bind-try",
        "an exposed subpath must stay restored, and read-only: {args:?}"
    );
    assert!(
        cwd_bind < mask && mask < expose,
        "the re-mask lands after the project bind, so the exposed subpath has to be restored after it in turn: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_mask_root_containing_the_cwd_keeps_the_project_bind_as_the_last_word() {
    // The other side of the same rule: a project living inside a masked
    // directory stays usable, so a mask root that *contains* the working
    // directory is masked once and left shadowed there.
    let cwd = std::env::current_dir().unwrap();
    let root = cwd
        .parent()
        .expect("cwd should have a parent")
        .to_path_buf();
    let cache_dir = dir("contains-cwd-cache");
    let sandbox = bwrap_sandbox(vec![root.clone()], cache_dir.clone());

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    let mounts = mounts_at(&args, &root.to_string_lossy());
    let cwd_bind = pair_at(&args, "--bind", &cwd.to_string_lossy())
        .expect("the working directory should be bound");

    assert_eq!(
        mounts.len(),
        1,
        "a mask root containing the working directory must be masked exactly once: {args:?}"
    );
    assert!(
        mounts[0].0 < cwd_bind,
        "the mask comes first so the project bind can shadow it: {args:?}"
    );
    assert_eq!(
        last_mount_at(&args, &cwd).1,
        "--bind",
        "the project bind must stay the last word on its own subtree: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_mask_root_outside_every_read_write_bind_is_masked_once() {
    let root = dir("outside-binds");
    std::fs::create_dir_all(&root).unwrap();
    let cache_dir = dir("outside-binds-cache");
    let sandbox = bwrap_sandbox(vec![root.clone()], cache_dir.clone());

    let args = args_of(&sandbox.wrap_command("echo hello").unwrap());
    assert_eq!(
        mounts_at(&args, &root.to_string_lossy()).len(),
        1,
        "nothing re-opens this root, so a second mask layer would be dead weight: {args:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_optional_sandbox_falling_back_to_bare_masks_nothing() {
    // An optional sandbox runs bare when the cached probe finds no backend,
    // even if a fresh probe would find one. A masked-path claim there would
    // tell the model a file was hidden while the command ran bare and read it
    // fine.
    let root = dir("probe");
    std::fs::create_dir_all(&root).unwrap();
    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(false)
        .with_backend_installed_now(true)
        .with_mask_roots(vec![root.clone()]);

    let cmd = sandbox.wrap_command("echo hello").unwrap();
    assert_eq!(
        cmd.as_std().get_program(),
        "bash",
        "sanity: the cached probe says the backend is missing, so the command runs unsandboxed"
    );
    assert!(
        sandbox.masked_roots().is_empty(),
        "an unsandboxed command masks nothing"
    );

    let sandboxed = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_mask_roots(vec![root.clone()]);
    assert_eq!(
        sandboxed.masked_roots(),
        vec![root.clone()],
        "sanity: the same root is masked once the backend actually runs"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_required_sandbox_masks_on_the_fresh_probe_that_launches_it() {
    // A required sandbox launches bwrap on the fresh probe when the cached one
    // is stale (the backend was installed after the process-wide probe ran).
    // Masking has to follow it there: gating on the cached probe alone would
    // run bwrap with an empty mask list, leaving every credential directory
    // readable inside the sandbox of the user who set `sandbox-required`.
    let root = dir("required-probe");
    std::fs::create_dir_all(&root).unwrap();
    let cache_dir = dir("required-probe-cache");
    let sandbox = Sandbox::new(true, "bwrap")
        .with_required(true)
        .with_backend_available(false)
        .with_backend_installed_now(true)
        .with_cache_dir(cache_dir.clone())
        .with_mask_roots(vec![root.clone()]);

    let cmd = sandbox.wrap_command("echo hello").unwrap();
    assert_eq!(
        cmd.as_std().get_program(),
        "bwrap",
        "sanity: a required sandbox launches on the fresh probe, so this command is sandboxed"
    );
    assert_eq!(
        sandbox.masked_roots(),
        vec![root.clone()],
        "a sandboxed command must mask, whichever probe let it launch"
    );
    assert!(
        pair_at(&args_of(&cmd), "--tmpfs", &root.to_string_lossy()).is_some(),
        "the mask has to reach the bwrap invocation, not just masked_roots()"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_shadowed_mask_warning_is_computed_against_the_bound_working_directory() {
    // The sandbox binds this process's working directory, never a directory
    // handed in by a caller (an ACP client's session cwd, say), so the warning
    // has to be about that one.
    let cwd = std::env::current_dir().unwrap();
    let sandbox = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_mask_roots(vec![cwd.clone()]);
    let warning = sandbox
        .shadowed_mask_warning()
        .expect("a mask root containing the bound working directory must warn");
    assert!(
        warning.contains(&cwd.display().to_string()),
        "the warning should name the shadowed root: {warning}"
    );

    let elsewhere = dir("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let other = Sandbox::new(true, "bwrap")
        .with_backend_available(true)
        .with_mask_roots(vec![elsewhere.clone()]);
    assert_eq!(
        other.shadowed_mask_warning(),
        None,
        "a mask root the bound working directory is not inside must stay quiet"
    );
    assert_eq!(
        other.shadowed_mask_root(&elsewhere.join("project")),
        Some(elsewhere.clone()),
        "sanity: that root would warn if some other directory decided the answer"
    );

    let _ = std::fs::remove_dir_all(&elsewhere);
}

// --- The built-in mask list ---

#[test]
fn test_mask_roots_follow_xdg_config_home_when_set() {
    // XDG_CONFIG_HOME is forwarded into the sandbox, so masking `~/.config/gh`
    // on a host that relocated its config base hides nothing the tools read.
    let home = PathBuf::from("/home/tester");
    let xdg = PathBuf::from("/home/tester/cfg");

    assert_eq!(
        mask_roots_for(&home, Some(&xdg)),
        vec![
            home.join(".ssh"),
            home.join(".aws"),
            home.join(".gnupg"),
            home.join(".kube"),
            home.join(".docker"),
            xdg.join("gh"),
            xdg.join("gcloud"),
            xdg.join("op"),
            xdg.join("sops/age"),
        ]
    );
}

#[test]
fn test_mask_roots_default_to_dot_config() {
    let home = PathBuf::from("/home/tester");

    assert_eq!(
        mask_roots_for(&home, None),
        vec![
            home.join(".ssh"),
            home.join(".aws"),
            home.join(".gnupg"),
            home.join(".kube"),
            home.join(".docker"),
            home.join(".config/gh"),
            home.join(".config/gcloud"),
            home.join(".config/op"),
            home.join(".config/sops/age"),
        ]
    );
}

#[test]
fn test_relative_xdg_config_home_is_ignored() {
    // A relative XDG_CONFIG_HOME is invalid per the spec; falling back beats
    // masking a path relative to whatever the working directory happens to be.
    let home = PathBuf::from("/home/tester");

    assert_eq!(
        mask_roots_for(&home, Some(Path::new("cfg"))),
        mask_roots_for(&home, None)
    );
}

#[test]
fn test_disabled_sandbox_masks_nothing() {
    let root = dir("disabled");
    std::fs::create_dir_all(&root).unwrap();
    let sandbox = Sandbox::new(false, "bwrap")
        .with_backend_available(true)
        .with_mask_roots(vec![root.clone()]);

    assert!(sandbox.masked_roots().is_empty());
    assert_eq!(sandbox.shadowed_mask_root(&root), None);

    let _ = std::fs::remove_dir_all(&root);
}
