use rig::completion::ToolDefinition;
use rig::tool::Tool;

use crate::agent::tools::{AskSender, EditArgs, PermCheck, ToolError, check_perm_path};

pub struct EditTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
}

impl EditTool {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        EditTool { permission, ask_tx }
    }

    fn show_diff(
        path: &str,
        content: &str,
        byte_pos: usize,
        old_text: &str,
        new_text: &str,
    ) -> String {
        let lines: Vec<&str> = content.lines().collect();
        let old_line_count = old_text.lines().count();
        let new_line_count = new_text.lines().count();
        let ctx: usize = 3;

        let match_line = content[..byte_pos].matches('\n').count();
        let start = match_line.saturating_sub(ctx);
        let ctx_after_start = (match_line + old_line_count).min(lines.len());
        let ctx_after_end = (ctx_after_start + ctx).min(lines.len());

        let ctx_before = match_line - start;
        let ctx_after = ctx_after_end - ctx_after_start;

        let mut result = format!("\n--- a/{}\n+++ b/{}\n", path, path);
        result.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n",
            old_start = start + 1,
            old_count = ctx_before + old_line_count + ctx_after,
            new_start = start + 1,
            new_count = ctx_before + new_line_count + ctx_after,
        ));

        for i in start..match_line {
            if let Some(line) = lines.get(i) {
                result.push_str(&format!(" {}\n", line));
            }
        }
        for line in old_text.lines() {
            result.push_str(&format!("-{}\n", line));
        }
        for line in new_text.lines() {
            result.push_str(&format!("+{}\n", line));
        }
        for i in ctx_after_start..ctx_after_end {
            if let Some(line) = lines.get(i) {
                result.push_str(&format!(" {}\n", line));
            }
        }

        result
    }
}

impl Tool for EditTool {
    const NAME: &'static str = "edit";

    type Error = ToolError;
    type Args = EditArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "edit".to_string(),
            description: "Edit a file by replacing exact text. If old_text appears once, replaces it. If it appears multiple times and replace_all is false, returns all match locations with line numbers. Use replaceAll: true to replace every occurrence. Handles both LF and CRLF line endings.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
                    "old_text": { "type": "string", "description": "Exact text to find and replace" },
                    "new_text": { "type": "string", "description": "New text to replace with" },
                    "replace_all": { "type": "boolean", "description": "Replace all occurrences instead of just the first" }
                },
                "required": ["path", "old_text", "new_text"]
            }),
        }
    }

    async fn call(&self, args: EditArgs) -> Result<String, ToolError> {
        if args.old_text.is_empty() {
            return Err(ToolError::Msg(
                "old_text must not be empty. Provide the exact text to replace.".to_string(),
            ));
        }

        check_perm_path(&self.permission, &self.ask_tx, "edit", &args.path).await?;

        let bytes = tokio::fs::read(&args.path).await?;
        let has_crlf = bytes.windows(2).any(|w| w == b"\r\n");
        let content = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");

        if !content.contains(&args.old_text) {
            return Err(ToolError::Msg(format!(
                "old_text not found in '{}'.\nEnsure the exact text matches including whitespace and line endings.",
                args.path
            )));
        }

        let match_positions: Vec<usize> = content
            .match_indices(&args.old_text)
            .map(|(i, _)| i)
            .collect();

        let do_replace_all = args.replace_all.unwrap_or(false);

        if match_positions.len() > 1 && !do_replace_all {
            let line_starts: Vec<usize> = std::iter::once(0)
                .chain(content.match_indices('\n').map(|(i, _)| i + 1))
                .collect();

            let mut match_info = Vec::new();
            for &byte_idx in &match_positions {
                let line_num = match line_starts.binary_search(&byte_idx) {
                    Ok(i) => i + 1,
                    Err(i) => i,
                };
                let line_start = line_starts.get(line_num - 1).copied().unwrap_or(0);
                let line_end = content[line_start..]
                    .find('\n')
                    .map(|e| line_start + e)
                    .unwrap_or(content.len());
                let line_text = &content[line_start..line_end];
                let truncated: String = line_text.chars().take(100).collect();
                match_info.push(format!("  Line {}: {}", line_num, truncated));
            }

            return Err(ToolError::Msg(format!(
                "old_text matched {} times in {}:\n{}\n\nUse replaceAll: true to replace all occurrences, or provide more surrounding context in old_text to narrow the match.",
                match_positions.len(),
                args.path,
                match_info.join("\n"),
            )));
        }

        let byte_pos = match_positions[0];
        let new_content = if do_replace_all {
            content.replace(&args.old_text, &args.new_text)
        } else {
            content.replacen(&args.old_text, &args.new_text, 1)
        };

        let output = if has_crlf {
            new_content.replace('\n', "\r\n")
        } else {
            new_content
        };

        tokio::fs::write(&args.path, &output).await?;

        let mut result = format!("Applied edit to {}", args.path);
        if do_replace_all {
            result.push_str(&format!(" ({} replacements)", match_positions.len()));
        }

        let old_lines = args.old_text.lines().count();
        let new_lines = args.new_text.lines().count();
        if old_lines <= 20 && new_lines <= 20 {
            result.push_str(&Self::show_diff(
                &args.path,
                &content,
                byte_pos,
                &args.old_text,
                &args.new_text,
            ));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
