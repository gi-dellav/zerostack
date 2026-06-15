use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::AsyncWriteExt;

/// Atomically write `contents` to `path`.
///
/// Writes to a temporary file in the *same directory* as `path`, then renames
/// it over the target. POSIX `rename(2)` is atomic, so neither a concurrent
/// reader nor a process that is killed mid-write (e.g. the terminal being
/// closed → SIGHUP) can ever observe a truncated or partially-written file: the
/// target is always either the complete old contents or the complete new
/// contents.
///
/// This replaces the previous `tokio::fs::write` calls, which used
/// open-truncate-then-stream and could leave an existing file destroyed if the
/// process died after truncation but before the write completed.
///
/// Note: this intentionally does *not* fsync. Atomicity against a killed
/// process comes from `rename` alone; fsync would only buy durability across a
/// power loss / kernel panic, at ~20-50x the per-write cost, which is not the
/// failure mode being defended against here.
///
/// If `path` is a symlink (or chain of symlinks), the write goes *through* it to
/// the file it points at, leaving the link intact; a plain rename would replace
/// the link with a regular file.
///
/// If `path` already exists, its permission bits are copied onto the new file —
/// a plain create-and-rename would otherwise reset them to the umask default
/// and silently drop e.g. the executable bit on scripts.
///
/// The temp file lives in the same directory (not the system temp dir) because
/// `rename` is only atomic within a single filesystem.
pub async fn atomic_write(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    // Write *through* symlinks: if the target is a symlink (or a chain of them),
    // resolve to the final file so the rename replaces that file and leaves the
    // link itself intact. A plain rename over a symlink would instead clobber the
    // link with a regular file. A non-symlink path is returned unchanged.
    let resolved = resolve_symlink_target(path.as_ref()).await;
    let path = resolved.as_path();
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };

    let tmp_path = unique_tmp_path(path, &dir);

    // Write into the temp file, cleaning it up on any failure so we never leave
    // stray `.*.zswrite.*.tmp` files behind on error.
    //
    // Deliberately no fsync: the crash-safety we need here comes entirely from
    // `rename(2)`, which is atomic, so a process killed mid-write (terminal
    // closed → SIGHUP, or a crash) can never leave a truncated target. fsync
    // would only add power-loss durability — at ~20-50x the per-write cost — and
    // that is out of scope for this threat model. The file is closed at the end
    // of this block (scope exit), before the rename.
    let write_result = async {
        let mut f = tokio::fs::File::create(&tmp_path).await?;
        f.write_all(contents.as_ref()).await?;
        Ok::<(), std::io::Error>(())
    }
    .await;
    if let Err(e) = write_result {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    // Preserve the original file's permissions when replacing an existing file.
    // (For brand-new files `metadata` errors out and we keep the default perms.)
    if let Ok(meta) = tokio::fs::metadata(path).await {
        let _ = tokio::fs::set_permissions(&tmp_path, meta.permissions()).await;
    }

    // Atomic replace.
    if let Err(e) = tokio::fs::rename(&tmp_path, path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }
    Ok(())
}

/// Build a collision-free temp path next to `target`, within `dir`.
///
/// Uniqueness across parallel agents comes from the process id; uniqueness
/// within a single process comes from the monotonic counter. Both matter
/// because multistack runs many zerostack processes concurrently.
fn unique_tmp_path(target: &Path, dir: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("tmp");
    dir.join(format!(".{stem}.zswrite.{}.{n}.tmp", std::process::id()))
}

/// Follow a symlink (or chain of symlinks) to the file it ultimately points at,
/// so an atomic write replaces that file rather than the link.
///
/// Relative link targets are resolved against the directory of the link that
/// produced them, matching POSIX semantics. The number of hops is bounded (as
/// the kernel bounds `MAXSYMLINKS`) to avoid looping on a cyclic link. If `path`
/// is not a symlink, or a link is broken/unreadable, the best path resolved so
/// far is returned — so a plain file, a new file, or a broken link all behave
/// sensibly (we just write to that path).
async fn resolve_symlink_target(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match tokio::fs::symlink_metadata(&current).await {
            Ok(meta) if meta.file_type().is_symlink() => match tokio::fs::read_link(&current).await
            {
                Ok(target) => {
                    current = if target.is_absolute() {
                        target
                    } else if let Some(parent) = current.parent() {
                        parent.join(target)
                    } else {
                        target
                    };
                }
                Err(_) => break,
            },
            _ => break,
        }
    }
    current
}

pub fn expand_tilde(s: &str) -> String {
    let home = || dirs::home_dir().map(|p| p.to_string_lossy().to_string());

    if s == "~" || s == "$HOME" {
        if let Some(h) = home() {
            return h;
        }
        return s.to_string();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(h) = home() {
            return std::path::Path::new(&h)
                .join(rest)
                .to_string_lossy()
                .to_string();
        }
        return s.to_string();
    }
    if let Some(rest) = s.strip_prefix("$HOME/")
        && let Some(h) = home()
    {
        return std::path::Path::new(&h)
            .join(rest)
            .to_string_lossy()
            .to_string();
    }
    s.to_string()
}
