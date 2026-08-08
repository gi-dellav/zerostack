//! Scaffolding shared by the sandbox test files (masking, expose, agent
//! cutoff, network). All of them ask the same question in the same way: build
//! a sandbox whose backend is pinned to "available" so the bwrap branch runs
//! on any host, then assert on the argument list it assembles. These are the
//! pieces every one of them needs; anything used by a single file stays in
//! that file.

use std::path::PathBuf;

use tokio::process::Command;

use crate::sandbox::Sandbox;

/// A scratch path under the system temp directory, namespaced by test file
/// (`group`) and by case (`name`) so tests running in parallel, in the same
/// process, never collide on a directory.
pub(super) fn scratch_dir(group: &str, name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "zerostack-{}-{}-{}",
        group,
        name,
        std::process::id()
    ))
}

/// The arguments a wrapped command would be launched with, as strings. Every
/// sandbox assertion is a statement about this list.
pub(super) fn args_of(cmd: &Command) -> Vec<String> {
    cmd.as_std()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// Index of `flag` in an adjacent `flag value` pair, so `--tmpfs /tmp` never
/// answers a question asked about `--tmpfs <mask root>`.
pub(super) fn pair_at(args: &[String], flag: &str, value: &str) -> Option<usize> {
    args.windows(2).position(|w| w[0] == flag && w[1] == value)
}

/// Index of `flag` in an adjacent `flag src dst` triple, matching the
/// `--ro-bind-try <src> <dst>` shape the masks and expose emit.
pub(super) fn triple_at(args: &[String], flag: &str, src: &str, dst: &str) -> Option<usize> {
    args.windows(3)
        .position(|w| w[0] == flag && w[1] == src && w[2] == dst)
}

/// A sandbox on `backend` whose backend probe is pinned to "available", so the
/// argument assembly under test runs on hosts without bwrap installed. The
/// cache bind is pointed at a scratch path because assembling the arguments
/// creates that directory, which must never be the developer's real
/// `~/.cache`, and the mask list is given explicitly so assertions do not
/// depend on which credential directories the host happens to have.
pub(super) fn backend_sandbox(backend: &str, masks: Vec<PathBuf>, cache_dir: PathBuf) -> Sandbox {
    Sandbox::new(true, backend)
        .with_backend_available(true)
        .with_cache_dir(cache_dir)
        .with_mask_roots(masks)
}

/// `backend_sandbox` on the default backend, which is what almost every
/// sandbox test wants.
pub(super) fn bwrap_sandbox(masks: Vec<PathBuf>, cache_dir: PathBuf) -> Sandbox {
    backend_sandbox("bwrap", masks, cache_dir)
}
