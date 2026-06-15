//! Tests for `crate::fs::atomic_write`.
//!
//! These exercise the public contract through `atomic_write` only: that writes
//! are atomic (a reader never sees a truncated file), that permissions are
//! preserved, that symlinks are written through rather than clobbered, and that
//! no temp-file residue is left behind.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::fs::atomic_write;

/// A unique temp directory per call, removed on drop. Uniqueness (process id +
/// monotonic counter) keeps parallel test runs from colliding without pulling
/// in an external temp-dir crate.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "zerostack_atomic_test_{}_{}_{}",
            tag,
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Count leftover temp files created by `atomic_write` in a directory.
fn temp_residue(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains(".zswrite."))
                .count()
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn creates_new_file() {
    let dir = TempDir::new("new");
    let f = dir.join("new.txt");
    atomic_write(&f, b"hello world").await.unwrap();
    assert_eq!(std::fs::read(&f).unwrap(), b"hello world");
    assert_eq!(temp_residue(dir.path()), 0);
}

#[tokio::test]
async fn overwrites_existing_file() {
    let dir = TempDir::new("overwrite");
    let f = dir.join("f.txt");
    std::fs::write(&f, b"old contents").unwrap();
    atomic_write(&f, b"new contents").await.unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "new contents");
    assert_eq!(temp_residue(dir.path()), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn preserves_permissions_on_overwrite() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("perms");
    let f = dir.join("script.sh");
    std::fs::write(&f, b"#!/bin/sh\necho old\n").unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();

    atomic_write(&f, b"#!/bin/sh\necho new\n").await.unwrap();

    let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "executable bit must survive the atomic replace");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "#!/bin/sh\necho new\n");
}

#[cfg(unix)]
#[tokio::test]
async fn writes_through_symlink() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new("symlink");
    let real = dir.join("real.txt");
    let link = dir.join("link.txt");
    std::fs::write(&real, b"old").unwrap();
    symlink(&real, &link).unwrap();

    atomic_write(&link, b"new via link").await.unwrap();

    // The link must still be a link, and the real file must hold the new data.
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the symlink itself must be preserved, not replaced with a regular file"
    );
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "new via link");
    assert_eq!(std::fs::read_to_string(&link).unwrap(), "new via link");
    assert_eq!(temp_residue(dir.path()), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn writes_through_symlink_chain() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new("chain");
    let real = dir.join("real.txt");
    let link = dir.join("link.txt");
    let link2 = dir.join("link2.txt");
    std::fs::write(&real, b"old").unwrap();
    symlink(&real, &link).unwrap();
    symlink(&link, &link2).unwrap();

    atomic_write(&link2, b"via chain").await.unwrap();

    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert!(std::fs::symlink_metadata(&link2).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "via chain");
}

#[cfg(unix)]
#[tokio::test]
async fn resolves_relative_symlink_target() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new("relsym");
    let real = dir.join("r.txt");
    let link = dir.join("rlink.txt");
    std::fs::write(&real, b"x").unwrap();
    // Relative target: must resolve against the link's own directory.
    symlink(Path::new("r.txt"), &link).unwrap();

    atomic_write(&link, b"relative ok").await.unwrap();

    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "relative ok");
}

#[tokio::test]
async fn concurrent_writes_to_distinct_files_leave_no_residue() {
    let dir = TempDir::new("concurrent");
    let mut handles = Vec::new();
    for i in 0..50 {
        let p = dir.join(&format!("p{i}.txt"));
        handles.push(tokio::spawn(async move {
            atomic_write(&p, format!("file {i}").into_bytes())
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    for i in 0..50 {
        let p = dir.join(&format!("p{i}.txt"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("file {i}"));
    }
    assert_eq!(temp_residue(dir.path()), 0);
}

/// The core guarantee: while one writer repeatedly replaces a file, a separate
/// OS-thread reader must never observe a truncated or partially-written state —
/// only the complete old value or the complete new value. This is what `rename`
/// buys us and what the old truncate-then-stream write could not.
#[tokio::test]
async fn no_torn_reads_during_rewrites() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let dir = TempDir::new("torn");
    let target = dir.join("hot.txt");
    let size = 64 * 1024;
    atomic_write(&target, vec![b'A'; size]).await.unwrap();

    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let path = target.clone();
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut torn = 0u64;
            while !done.load(Ordering::Relaxed) {
                if let Ok(bytes) = std::fs::read(&path) {
                    let homogeneous =
                        bytes.iter().all(|&c| c == b'A') || bytes.iter().all(|&c| c == b'B');
                    if !(bytes.len() == size && homogeneous) {
                        torn += 1;
                    }
                }
            }
            torn
        })
    };

    for r in 0..300 {
        let fill = if r % 2 == 0 { b'B' } else { b'A' };
        atomic_write(&target, vec![fill; size]).await.unwrap();
    }
    done.store(true, Ordering::Relaxed);

    let torn = reader.join().unwrap();
    assert_eq!(torn, 0, "reader observed {torn} torn/partial states");
    assert_eq!(temp_residue(dir.path()), 0);
}
