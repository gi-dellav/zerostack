// These tests deliberately hold EDIT_SYSTEM_GUARD across .await points to keep
// the process-global edit system fixed for the whole async test (see the guard
// doc below), so the lint does not apply here.
#![allow(clippy::await_holding_lock)]

use crate::agent::tools::crc::crc32_hex;
use crate::agent::tools::set_edit_system;
use crate::agent::tools::{EditArgs, EditOp, edit};
use crate::config::types::EditSystem;
use rig::tool::PortableTool as Tool;

/// The edit system is a process-global, and `cargo test` runs tests in parallel,
/// so a `Similarity` test could otherwise have the global flipped to `Hashedit`
/// by a concurrent test mid-run. Serialize every test that touches it: lock this
/// shared mutex (held for the test's lifetime via the returned guard) and set
/// the system atomically.
static EDIT_SYSTEM_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serialize_edit_system(es: EditSystem) -> std::sync::MutexGuard<'static, ()> {
    let guard = EDIT_SYSTEM_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    set_edit_system(es);
    guard
}

struct TempFile(String);

impl TempFile {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("zerostack_test_{}", name))
            .to_string_lossy()
            .to_string();
        TempFile(path)
    }

    fn path(&self) -> &str {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ── Similarity (V1) tests ──────────────────────────────────────────────

#[tokio::test]
async fn test_sim_rejects_no_blocks() {
    let _edit_guard = serialize_edit_system(EditSystem::Similarity);
    let tmp = TempFile::new("noblocks.txt");
    std::fs::write(tmp.path(), "hello world\n").unwrap();
    let tool = edit::EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: Some("no blocks here".into()),
            file_crc: None,
            edits: None,
        })
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("No SEARCH/REPLACE blocks found"));
}

#[tokio::test]
async fn test_sim_rejects_empty_search() {
    let _edit_guard = serialize_edit_system(EditSystem::Similarity);
    let tmp = TempFile::new("emptysearch.txt");
    std::fs::write(tmp.path(), "hello world\n").unwrap();
    let tool = edit::EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: Some("<<<<<<< SEARCH\n=======\nreplacement\n>>>>>>> REPLACE".into()),
            file_crc: None,
            edits: None,
        })
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("has empty search text"));
}

#[tokio::test]
async fn test_sim_search_not_found() {
    let _edit_guard = serialize_edit_system(EditSystem::Similarity);
    let tmp = TempFile::new("notfound2.txt");
    std::fs::write(tmp.path(), "hello world\n").unwrap();
    let tool = edit::EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: Some(
                "<<<<<<< SEARCH\nthis does not exist in file\n=======\nreplacement\n>>>>>>> REPLACE"
                    .into(),
            ),
            file_crc: None,
            edits: None,
        })
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("not found"));
}

#[tokio::test]
async fn test_sim_single_block_replacement() {
    let _edit_guard = serialize_edit_system(EditSystem::Similarity);
    let tmp = TempFile::new("single2.txt");
    std::fs::write(tmp.path(), "before after done\n").unwrap();
    let tool = edit::EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: Some("<<<<<<< SEARCH\nafter\n=======\nmiddle\n>>>>>>> REPLACE".into()),
            file_crc: None,
            edits: None,
        })
        .await
        .unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert_eq!(content, "before middle done\n");
    assert!(result.contains("Applied 1 edit(s)"));
}

#[tokio::test]
async fn test_sim_multi_block_atomic() {
    let _edit_guard = serialize_edit_system(EditSystem::Similarity);
    let tmp = TempFile::new("multiblock.txt");
    std::fs::write(tmp.path(), "aaa\nbbb\nccc\n").unwrap();
    let tool = edit::EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: Some(
                "\
<<<<<<< SEARCH
aaa
=======
AAA
>>>>>>> REPLACE

<<<<<<< SEARCH
ccc
=======
CCC
>>>>>>> REPLACE"
                    .into(),
            ),
            file_crc: None,
            edits: None,
        })
        .await
        .unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert_eq!(content, "AAA\nbbb\nCCC\n");
    assert!(result.contains("Applied 2 edit(s)"));
}

#[tokio::test]
async fn test_sim_multi_match_returns_error() {
    let _edit_guard = serialize_edit_system(EditSystem::Similarity);
    let tmp = TempFile::new("multi2.txt");
    std::fs::write(tmp.path(), "hello world, hello there\n").unwrap();
    let tool = edit::EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: Some("<<<<<<< SEARCH\nhello\n=======\nbye\n>>>>>>> REPLACE".into()),
            file_crc: None,
            edits: None,
        })
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("matched 2 times"));
}

#[tokio::test]
async fn test_sim_preserves_crlf_line_endings() {
    let _edit_guard = serialize_edit_system(EditSystem::Similarity);
    let tmp = TempFile::new("crlf2.txt");
    std::fs::write(tmp.path(), "line1\r\nline2\r\nline3\r\n").unwrap();
    let tool = edit::EditTool::new(None, None);
    tool.call(EditArgs {
        path: tmp.path().into(),
        block: Some("<<<<<<< SEARCH\nline2\n=======\nmodified\n>>>>>>> REPLACE".into()),
        file_crc: None,
        edits: None,
    })
    .await
    .unwrap();
    let raw = std::fs::read(tmp.path()).unwrap();
    assert!(
        raw.windows(2).any(|w| w == b"\r\n"),
        "CRLF should be preserved"
    );
}

// ── Hashedit (V2) tests ─────────────────────────────────────────────────

fn make_tagged_line(line_num: usize, content: &str) -> String {
    let tag = crc32_hex(content.as_bytes());
    format!("   {}|{} {}", line_num, tag, content)
}

#[tokio::test]
async fn test_hash_single_line_edit() {
    let _edit_guard = serialize_edit_system(EditSystem::Hashedit);
    let tmp = TempFile::new("hash_single.txt");
    let original = "use std::io;\nuse std::fs;\n\nfn main() {\n    println!(\"hi\");\n}\n";
    std::fs::write(tmp.path(), original).unwrap();
    let file_crc = crc32_hex(original.as_bytes());

    let tool = edit::EditTool::new(None, None);
    let tagged = make_tagged_line(4, "fn main() {");
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: None,
            file_crc: Some(file_crc),
            edits: Some(vec![EditOp {
                line: Some(tagged),
                lines: None,
                text: "fn run() {".into(),
            }]),
        })
        .await
        .unwrap();

    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(
        content.contains("fn run() {"),
        "expected 'fn run() {{', got: {content}"
    );
    assert!(!content.contains("fn main() {"));
    assert!(result.contains("Applied 1 edit(s)"));
}

#[tokio::test]
async fn test_hash_range_edit() {
    let _edit_guard = serialize_edit_system(EditSystem::Hashedit);
    let tmp = TempFile::new("hash_range.txt");
    let original = "line1\nline2\nline3\nline4\nline5\n";
    std::fs::write(tmp.path(), original).unwrap();
    let file_crc = crc32_hex(original.as_bytes());

    let tool = edit::EditTool::new(None, None);
    let l2 = make_tagged_line(2, "line2");
    let l3 = make_tagged_line(3, "line3");
    let l4 = make_tagged_line(4, "line4");
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: None,
            file_crc: Some(file_crc),
            edits: Some(vec![EditOp {
                line: None,
                lines: Some(format!("{}\n{}\n{}", l2, l3, l4)),
                text: "CHANGED_A\nCHANGED_B".into(),
            }]),
        })
        .await
        .unwrap();

    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert_eq!(content, "line1\nCHANGED_A\nCHANGED_B\nline5\n");
    assert!(result.contains("Applied 1 edit(s)"));
}

#[tokio::test]
async fn test_hash_delete_via_empty_text() {
    let _edit_guard = serialize_edit_system(EditSystem::Hashedit);
    let tmp = TempFile::new("hash_delete.txt");
    let original = "keep me\nremove me\nkeep me too\n";
    std::fs::write(tmp.path(), original).unwrap();
    let file_crc = crc32_hex(original.as_bytes());

    let tool = edit::EditTool::new(None, None);
    let tagged = make_tagged_line(2, "remove me");
    tool.call(EditArgs {
        path: tmp.path().into(),
        block: None,
        file_crc: Some(file_crc),
        edits: Some(vec![EditOp {
            line: Some(tagged),
            lines: None,
            text: String::new(),
        }]),
    })
    .await
    .unwrap();

    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert_eq!(content, "keep me\n\nkeep me too\n");
}

#[tokio::test]
async fn test_hash_file_crc_mismatch() {
    let _edit_guard = serialize_edit_system(EditSystem::Hashedit);
    let tmp = TempFile::new("hash_badcrc.txt");
    std::fs::write(tmp.path(), "hello world\n").unwrap();

    let tool = edit::EditTool::new(None, None);
    let tagged = make_tagged_line(1, "hello world");
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: None,
            file_crc: Some("deadbeef".into()),
            edits: Some(vec![EditOp {
                line: Some(tagged),
                lines: None,
                text: "bye".into(),
            }]),
        })
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("CRC mismatch"));
}

#[tokio::test]
async fn test_hash_tag_mismatch() {
    let _edit_guard = serialize_edit_system(EditSystem::Hashedit);
    let tmp = TempFile::new("hash_badtag.txt");
    let original = "hello world\n";
    std::fs::write(tmp.path(), original).unwrap();
    let file_crc = crc32_hex(original.as_bytes());

    let tool = edit::EditTool::new(None, None);
    // Tag is for "different content" not for "hello world"
    let bad_tag = crc32_hex(b"different content");
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: None,
            file_crc: Some(file_crc),
            edits: Some(vec![EditOp {
                line: Some(format!("   1|{} hello world", bad_tag)),
                lines: None,
                text: "bye".into(),
            }]),
        })
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("Tag mismatch"));
}

#[tokio::test]
async fn test_hash_invalid_tag_format() {
    let _edit_guard = serialize_edit_system(EditSystem::Hashedit);
    let tmp = TempFile::new("hash_badfmt.txt");
    let original = "hello world\n";
    std::fs::write(tmp.path(), original).unwrap();
    let file_crc = crc32_hex(original.as_bytes());

    let tool = edit::EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: None,
            file_crc: Some(file_crc),
            edits: Some(vec![EditOp {
                line: Some("not a valid tagged line".into()),
                lines: None,
                text: "bye".into(),
            }]),
        })
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("invalid tagged line"));
}

#[tokio::test]
async fn test_hash_crlf_preserved() {
    let _edit_guard = serialize_edit_system(EditSystem::Hashedit);
    let tmp = TempFile::new("hash_crlf.txt");
    let original = "line1\r\nline2\r\nline3\r\n";
    std::fs::write(tmp.path(), original).unwrap();
    // CRC must be computed on LF-normalized content, same as edit tool normalizes
    let normalized = original.replace("\r\n", "\n");
    let file_crc = crc32_hex(normalized.as_bytes());

    let tool = edit::EditTool::new(None, None);
    let tagged = make_tagged_line(2, "line2");
    tool.call(EditArgs {
        path: tmp.path().into(),
        block: None,
        file_crc: Some(file_crc),
        edits: Some(vec![EditOp {
            line: Some(tagged),
            lines: None,
            text: "modified".into(),
        }]),
    })
    .await
    .unwrap();

    let raw = std::fs::read(tmp.path()).unwrap();
    assert!(
        raw.windows(2).any(|w| w == b"\r\n"),
        "CRLF should be preserved"
    );
}

#[tokio::test]
async fn test_hash_multi_edit_atomic() {
    let _edit_guard = serialize_edit_system(EditSystem::Hashedit);
    let tmp = TempFile::new("hash_multi.txt");
    let original = "aaa\nbbb\nccc\nddd\n";
    std::fs::write(tmp.path(), original).unwrap();
    let file_crc = crc32_hex(original.as_bytes());

    let tool = edit::EditTool::new(None, None);
    let l1 = make_tagged_line(1, "aaa");
    let l4 = make_tagged_line(4, "ddd");
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            block: None,
            file_crc: Some(file_crc),
            edits: Some(vec![
                EditOp {
                    line: Some(l1),
                    lines: None,
                    text: "AAA".into(),
                },
                EditOp {
                    line: Some(l4),
                    lines: None,
                    text: "DDD".into(),
                },
            ]),
        })
        .await
        .unwrap();

    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert_eq!(content, "AAA\nbbb\nccc\nDDD\n");
    assert!(result.contains("Applied 2 edit(s)"));
}
