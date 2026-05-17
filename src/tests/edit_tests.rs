use crate::agent::tools::edit::EditTool;
use crate::agent::tools::{EditArgs, ToolError};
use rig::tool::Tool;

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

#[tokio::test]
async fn test_rejects_empty_old_text() {
    let tool = EditTool::new(None, None);
    let args = EditArgs {
        path: "/tmp/test.txt".to_string(),
        old_text: String::new(),
        new_text: "replacement".to_string(),
        replace_all: None,
    };
    let result = tool.call(args).await;
    assert!(result.is_err());
    match result {
        Err(ToolError::Msg(msg)) => {
            assert!(
                msg.contains("old_text must not be empty"),
                "unexpected msg: {msg}"
            );
        }
        _ => panic!("expected ToolError::Msg"),
    }
}

#[tokio::test]
async fn test_old_text_not_found() {
    let tmp = TempFile::new("notfound.txt");
    std::fs::write(tmp.path(), "hello world").unwrap();
    let tool = EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            old_text: "not in file".into(),
            new_text: "replacement".into(),
            replace_all: None,
        })
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("old_text not found"), "msg: {msg}");
}

#[tokio::test]
async fn test_single_replacement() {
    let tmp = TempFile::new("single.txt");
    std::fs::write(tmp.path(), "before after done\n").unwrap();
    let tool = EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            old_text: "after".into(),
            new_text: "middle".into(),
            replace_all: None,
        })
        .await
        .unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert_eq!(content, "before middle done\n");
    assert!(result.contains("Applied edit"), "result: {result}");
}

#[tokio::test]
async fn test_replace_all() {
    let tmp = TempFile::new("replace_all.txt");
    std::fs::write(tmp.path(), "a a a\n").unwrap();
    let tool = EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            old_text: "a".into(),
            new_text: "b".into(),
            replace_all: Some(true),
        })
        .await
        .unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert_eq!(content, "b b b\n");
    assert!(result.contains("3 replacements"), "result: {result}");
}

#[tokio::test]
async fn test_multi_match_without_replace_all_returns_error() {
    let tmp = TempFile::new("multi.txt");
    std::fs::write(tmp.path(), "hello world, hello there\n").unwrap();
    let tool = EditTool::new(None, None);
    let result = tool
        .call(EditArgs {
            path: tmp.path().into(),
            old_text: "hello".into(),
            new_text: "bye".into(),
            replace_all: None,
        })
        .await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("matched 2 times"), "msg: {msg}");
    assert!(msg.contains("replaceAll: true"), "msg: {msg}");
}

#[tokio::test]
async fn test_preserves_crlf_line_endings() {
    let tmp = TempFile::new("crlf.txt");
    std::fs::write(tmp.path(), "line1\r\nline2\r\nline3\r\n").unwrap();
    let tool = EditTool::new(None, None);
    tool.call(EditArgs {
        path: tmp.path().into(),
        old_text: "line2".into(),
        new_text: "modified".into(),
        replace_all: None,
    })
    .await
    .unwrap();
    let raw = std::fs::read(tmp.path()).unwrap();
    assert!(
        raw.windows(2).any(|w| w == b"\r\n"),
        "CRLF should be preserved"
    );
}

#[test]
fn test_show_diff_basic() {
    let diff = EditTool::show_diff("test.txt", "hello world\nfoo bar\nbaz\n", 12, "foo", "qux");
    assert!(diff.contains("--- a/test.txt"), "diff: {diff}");
    assert!(diff.contains("+++ b/test.txt"), "diff: {diff}");
    assert!(diff.contains("-foo"), "diff: {diff}");
    assert!(diff.contains("+qux"), "diff: {diff}");
    assert!(diff.contains("@@"), "diff missing hunk header: {diff}");
}
