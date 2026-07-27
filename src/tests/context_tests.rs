use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use include_dir::{Dir, include_dir};

use crate::context::{copy_embedded_to, embedded_changed_files};

static PROMPTS: Dir = include_dir!("$CARGO_MANIFEST_DIR/data/prompts");

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("zerostack_ctx_test_{}_{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn embedded_file_count() -> usize {
    PROMPTS.files().count()
}

#[test]
fn changed_files_reports_everything_when_dest_missing() {
    let dir = temp_dir();
    let missing = dir.join("nonexistent");
    let changed = embedded_changed_files(&PROMPTS, &missing);
    assert_eq!(changed.len(), embedded_file_count());
    assert!(changed.windows(2).all(|w| w[0] <= w[1]), "sorted output");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copy_writes_all_then_nothing_when_up_to_date() {
    let dir = temp_dir();
    let written = copy_embedded_to(&PROMPTS, &dir).unwrap();
    assert_eq!(written, embedded_file_count());
    assert!(embedded_changed_files(&PROMPTS, &dir).is_empty());

    let rewritten = copy_embedded_to(&PROMPTS, &dir).unwrap();
    assert_eq!(rewritten, 0, "identical files must not be rewritten");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn changed_files_detects_modified_and_missing() {
    let dir = temp_dir();
    copy_embedded_to(&PROMPTS, &dir).unwrap();

    let first = PROMPTS.files().next().unwrap();
    let first_name = first
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    std::fs::write(dir.join(&first_name), "user edit").unwrap();

    let second = PROMPTS.files().nth(1).unwrap();
    let second_name = second
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    std::fs::remove_file(dir.join(&second_name)).unwrap();

    let changed = embedded_changed_files(&PROMPTS, &dir);
    assert_eq!(changed, vec![first_name.clone(), second_name]);

    // Regen restores modified/missing files; afterwards the diff is empty.
    let written = copy_embedded_to(&PROMPTS, &dir).unwrap();
    assert_eq!(written, 2);
    assert!(embedded_changed_files(&PROMPTS, &dir).is_empty());
    let restored = std::fs::read_to_string(dir.join(&first_name)).unwrap();
    assert_eq!(restored, first.contents_utf8().unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copy_preserves_user_files() {
    let dir = temp_dir();
    std::fs::write(dir.join("my-custom.md"), "mine").unwrap();
    copy_embedded_to(&PROMPTS, &dir).unwrap();
    let content = std::fs::read_to_string(dir.join("my-custom.md")).unwrap();
    assert_eq!(content, "mine", "user files must survive regeneration");
    let _ = std::fs::remove_dir_all(&dir);
}
