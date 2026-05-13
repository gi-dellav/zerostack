use std::io;
use std::path::Path;

use ignore::WalkBuilder;
use regex::Regex;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Msg(String),
}

impl From<io::Error> for ToolError {
    fn from(e: io::Error) -> Self {
        ToolError::Msg(e.to_string())
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(e: serde_json::Error) -> Self {
        ToolError::Msg(e.to_string())
    }
}

const MAX_GREP_RESULTS: usize = 200;
const MAX_FIND_RESULTS: usize = 200;

// "node_modules" and "target" are common dependency directories
// that may not always be listed in .gitignore (e.g., fresh clones).
fn is_skip_dir(name: &str) -> bool {
    matches!(name, "node_modules" | "target")
}

// --- ReadTool ---

#[derive(Deserialize)]
pub struct ReadArgs {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

pub struct ReadTool;

impl Tool for ReadTool {
    const NAME: &'static str = "read";

    type Error = ToolError;
    type Args = ReadArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "read".to_string(),
            description: "Read the contents of a file. Supports text files. Defaults to first 2000 lines. Use offset/limit for large files.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
                    "offset": { "type": "integer", "description": "Line number to start from (1-indexed)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines to read" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: ReadArgs) -> Result<String, ToolError> {
        let metadata = tokio::fs::metadata(&args.path).await?;
        let file_size = metadata.len();
        if file_size > 10 * 1024 * 1024 {
            return Err(ToolError::Msg(format!(
                "File too large ({} bytes). Max 10MB.",
                file_size
            )));
        }
        let content = tokio::fs::read_to_string(&args.path).await?;
        let total_lines = content.lines().count();

        let offset = args.offset.unwrap_or(1).max(1) - 1;
        let limit = args.limit.unwrap_or(2000);
        let end = (offset + limit).min(total_lines);

        let excerpt: String = content.lines()
            .skip(offset)
            .take(end - offset)
            .collect::<Vec<_>>()
            .join("\n");
        let info = format!(
            "File: {} ({} lines total, showing lines {}-{})\n\n{}",
            args.path,
            total_lines,
            offset + 1,
            end,
            excerpt
        );
        Ok(info)
    }
}

// --- WriteTool ---

#[derive(Deserialize)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
}

pub struct WriteTool;

impl Tool for WriteTool {
    const NAME: &'static str = "write";

    type Error = ToolError;
    type Args = WriteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "write".to_string(),
            description: "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
                    "content": { "type": "string", "description": "Content to write to the file" }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call(&self, args: WriteArgs) -> Result<String, ToolError> {
        let path = Path::new(&args.path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let bytes = args.content.len();
        tokio::fs::write(path, &args.content).await?;
        Ok(format!("Written {} bytes to {}", bytes, args.path))
    }
}

// --- EditTool ---

#[derive(Deserialize)]
pub struct EditArgs {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
    pub replace_all: Option<bool>,
}

pub struct EditTool;

impl EditTool {
    fn show_diff(path: &str, content: &str, byte_pos: usize, old_text: &str, new_text: &str) -> String {
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
        // Read raw bytes to detect CRLF
        let bytes = tokio::fs::read(&args.path).await?;
        let has_crlf = bytes.windows(2).any(|w| w == b"\r\n");

        // Normalize to LF for matching
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

        // Restore CRLF if the file originally had it
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
                &args.path, &content, byte_pos, &args.old_text, &args.new_text,
            ));
        }
        Ok(result)
    }
}

// --- BashTool ---

#[derive(Deserialize)]
pub struct BashArgs {
    pub command: String,
    pub timeout: Option<u64>,
}

pub struct BashTool;

impl Tool for BashTool {
    const NAME: &'static str = "bash";

    type Error = ToolError;
    type Args = BashArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "bash".to_string(),
            description: "Execute a bash command in the current working directory. Returns stdout and stderr.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Bash command to execute" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds (optional)" }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: BashArgs) -> Result<String, ToolError> {
        let output = if let Some(secs) = args.timeout {
            timeout(Duration::from_secs(secs), Command::new("bash")
                .arg("-c").arg(&args.command)
                .output()).await
                .map_err(|_| ToolError::Msg("Command timed out".to_string()))?
        } else {
            Command::new("bash")
                .arg("-c").arg(&args.command)
                .output().await
        }?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code().unwrap_or(-1);

        let mut result = String::new();
        if !stdout.is_empty() { result.push_str(&stdout); }
        if !stderr.is_empty() {
            if !result.is_empty() { result.push('\n'); }
            result.push_str(&stderr);
        }
        if exit_code != 0 {
            result.push_str(&format!("\nExit code: {}", exit_code));
        }
        Ok(result)
    }
}

// --- GrepTool (Rust-native, .gitignore-aware) ---

#[derive(Deserialize)]
pub struct GrepArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub include: Option<String>,
    pub context_lines: Option<usize>,
}

pub struct GrepTool;

impl GrepTool {
    fn glob_to_regex(glob: &str) -> String {
        let mut re = String::with_capacity(glob.len() * 2);
        for c in glob.chars() {
            match c {
                '.' => re.push_str("\\."),
                '*' => re.push_str(".*"),
                '?' => re.push('.'),
                '{' => re.push_str("(?:"),
                '}' => re.push(')'),
                ',' => re.push('|'),
                _ => re.push(c),
            }
        }
        re
    }

    fn is_binary(data: &[u8]) -> bool {
        data.iter().take(8192).any(|&b| b == 0)
    }
}

impl Tool for GrepTool {
    const NAME: &'static str = "grep";

    type Error = ToolError;
    type Args = GrepArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "grep".to_string(),
            description: "Search file contents using a regex pattern (Rust regex syntax). Respects .gitignore. Skips binary files, node_modules, and target.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for (supports Rust regex syntax)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (defaults to current working directory)"
                    },
                    "include": {
                        "type": "string",
                        "description": "Optional file glob pattern to filter (e.g. '*.rs', '*.{ts,tsx}')"
                    },
                    "context_lines": {
                        "type": "integer",
                        "description": "Number of context lines to show before and after each match (like grep -C)"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: GrepArgs) -> Result<String, ToolError> {
        let re = Regex::new(&args.pattern)
            .map_err(|e| ToolError::Msg(format!("Invalid regex pattern: {}", e)))?;

        let search_path = args.path.as_deref().unwrap_or(".");
        let context = args.context_lines.unwrap_or(0);

        let include_re = args.include.as_ref().map(|g| {
            let pattern = format!("^(?:{})$", Self::glob_to_regex(g));
            Regex::new(&pattern).unwrap_or_else(|_| Regex::new(".*").unwrap())
        });

        let walker = WalkBuilder::new(search_path)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .hidden(false)
            .filter_entry(|entry| {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    !is_skip_dir(entry.file_name().to_str().unwrap_or(""))
                } else {
                    true
                }
            })
            .build();

        let mut file_count = 0;
        let mut all_results: Vec<String> = Vec::new();

        for entry in walker.flatten().filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)) {
            if all_results.len() >= MAX_GREP_RESULTS {
                break;
            }

            if let Some(ref re_include) = include_re {
                let fname = entry.file_name().to_string_lossy();
                if !re_include.is_match(&fname) {
                    continue;
                }
            }

            if let Ok(meta) = entry.metadata()
                && meta.len() > 10 * 1024 * 1024
            {
                continue;
            }

            let path_str = entry.path().to_string_lossy().to_string();

            match tokio::fs::read(entry.path()).await {
                Ok(data) => {
                    if Self::is_binary(&data) {
                        continue;
                    }
                    file_count += 1;
                    let content = String::from_utf8_lossy(&data);
                    let lines: Vec<&str> = content.lines().collect();
                    let total = lines.len();

                    let match_lines: Vec<usize> = lines.iter().enumerate()
                        .filter(|(_, l)| re.is_match(l))
                        .map(|(i, _)| i)
                        .collect();

                    if match_lines.is_empty() {
                        continue;
                    }

                    if context == 0 {
                        for &ml in &match_lines {
                            all_results.push(format!("{}:{}:{}", path_str, ml + 1, lines[ml]));
                            if all_results.len() >= MAX_GREP_RESULTS {
                                break;
                            }
                        }
                    } else {
                        let mut shown = vec![false; total];
                        for &ml in &match_lines {
                            let start = ml.saturating_sub(context);
                            let end = (ml + 1 + context).min(total);
                            for s in &mut shown[start..end] {
                                *s = true;
                            }
                        }

                        let mut i = 0;
                        while i < total && all_results.len() < MAX_GREP_RESULTS {
                            if !shown[i] {
                                i += 1;
                                continue;
                            }

                            if !all_results.is_empty() {
                                all_results.push("--".to_string());
                            }

                            while i < total && shown[i] && all_results.len() < MAX_GREP_RESULTS {
                                let is_match = match_lines.binary_search(&i).is_ok();
                                let sep = if is_match { ':' } else { '-' };
                                all_results.push(format!("{}-{}{} {}", path_str, i + 1, sep, lines[i]));
                                i += 1;
                            }
                        }
                    }
                }
                Err(_) => continue,
            }
        }

        if all_results.is_empty() {
            return Ok("No matches found.".to_string());
        }

        let total = all_results.len();
        if total >= MAX_GREP_RESULTS {
            Ok(format!(
                "{} results (showing first {}, searched {} files):\n{}\n\n... and {} more matches",
                total,
                MAX_GREP_RESULTS,
                file_count,
                all_results.join("\n"),
                total - MAX_GREP_RESULTS
            ))
        } else {
            Ok(format!(
                "{} results (searched {} files):\n{}",
                total,
                file_count,
                all_results.join("\n")
            ))
        }
    }
}

// --- FindFilesTool (Rust-native, .gitignore-aware) ---

#[derive(Deserialize)]
pub struct FindFilesArgs {
    pub pattern: String,
    pub path: Option<String>,
}

pub struct FindFilesTool;

impl Tool for FindFilesTool {
    const NAME: &'static str = "find_files";

    type Error = ToolError;
    type Args = FindFilesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "find_files".to_string(),
            description: "Recursively find files matching a regex pattern in their filename. Respects .gitignore. Skips node_modules and target.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to match file names against"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (defaults to current working directory)"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: FindFilesArgs) -> Result<String, ToolError> {
        let re = Regex::new(&args.pattern)
            .map_err(|e| ToolError::Msg(format!("Invalid regex: {}", e)))?;

        let search_path = args.path.as_deref().unwrap_or(".");

        let walker = WalkBuilder::new(search_path)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .hidden(false)
            .filter_entry(|entry| {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    !is_skip_dir(entry.file_name().to_str().unwrap_or(""))
                } else {
                    true
                }
            })
            .build();

        let mut results: Vec<String> = Vec::new();

        for entry in walker.flatten().filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)) {
            let fname = entry.file_name().to_string_lossy();
            if re.is_match(&fname) {
                results.push(entry.path().to_string_lossy().to_string());
                if results.len() >= MAX_FIND_RESULTS {
                    break;
                }
            }
        }

        if results.is_empty() {
            return Ok("No files found matching the pattern.".to_string());
        }

        results.sort();

        let total = results.len();
        if total >= MAX_FIND_RESULTS {
            Ok(format!(
                "{} files found (showing first {}):\n{}\n\n... and {} more",
                total,
                MAX_FIND_RESULTS,
                results[..MAX_FIND_RESULTS].join("\n"),
                total - MAX_FIND_RESULTS
            ))
        } else {
            Ok(format!("{} files found:\n{}", total, results.join("\n")))
        }
    }
}

// --- ListDirTool (.gitignore-aware) ---

#[derive(Deserialize)]
pub struct ListDirArgs {
    pub path: Option<String>,
}

pub struct ListDirTool;

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

fn count_dir_entries(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .map(|rd| rd.count() as u64)
        .unwrap_or(0)
}

impl Tool for ListDirTool {
    const NAME: &'static str = "list_dir";

    type Error = ToolError;
    type Args = ListDirArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_dir".to_string(),
            description: "List files and directories in a directory. Respects .gitignore. Shows type, size, entry count for subdirectories. Sorted: directories first, then alphabetical.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path (defaults to current working directory)"
                    }
                },
                "required": []
            }),
        }
    }

    async fn call(&self, args: ListDirArgs) -> Result<String, ToolError> {
        let path = args.path.as_deref().unwrap_or(".");

        // Use ignore::Walk with max_depth(1) for .gitignore-aware listing
        let walker = WalkBuilder::new(path)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .hidden(false)
            .max_depth(Some(1))
            .filter_entry(|entry| {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    !is_skip_dir(entry.file_name().to_str().unwrap_or(""))
                } else {
                    true
                }
            })
            .build();

        let mut entries: Vec<(String, String, String)> = Vec::new();

        for result in walker {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.file_name().to_string_lossy().to_string();

            // Skip the root directory entry itself (depth 0)
            if entry.depth() == 0 {
                continue;
            }

            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            let kind = if meta.is_dir() {
                let count = count_dir_entries(entry.path());
                format!("dir({})", count)
            } else if meta.is_symlink() {
                "link".to_string()
            } else {
                "file".to_string()
            };

            let size = if meta.is_file() {
                format_size(meta.len())
            } else {
                String::new()
            };

            entries.push((name, kind, size));
        }

        entries.sort_by(|a, b| {
            let a_is_dir = a.1.starts_with("dir") || a.1 == "link";
            let b_is_dir = b.1.starts_with("dir") || b.1 == "link";
            if a_is_dir != b_is_dir {
                b_is_dir.cmp(&a_is_dir)
            } else {
                a.0.cmp(&b.0)
            }
        });

        if entries.is_empty() {
            return Ok(format!("Listing {}:\n(empty directory)", path));
        }

        let max_name = entries.iter().map(|e| e.0.len()).max().unwrap_or(0);
        let mut result = format!("Listing {}:\n", path);
        for (name, kind, size) in &entries {
            let padded = format!("{:width$}", name, width = max_name);
            let size_str = if size.is_empty() {
                String::new()
            } else {
                format!("  {}", size)
            };
            result.push_str(&format!("  [{}]  {}{}\n", kind, padded, size_str));
        }
        Ok(result)
    }
}

// --- WriteTodoList ---

#[derive(Serialize, Deserialize, Clone)]
pub struct TodoItem {
    pub content: String,
    pub status: String,
    pub priority: String,
}

#[derive(Deserialize)]
pub struct TodoWriteArgs {
    pub todos: Vec<TodoItem>,
}

pub static TODO_LIST: std::sync::Mutex<Vec<TodoItem>> = std::sync::Mutex::new(Vec::new());

pub struct WriteTodoList;

impl Tool for WriteTodoList {
    const NAME: &'static str = "write_todo_list";

    type Error = ToolError;
    type Args = TodoWriteArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "write_todo_list".to_string(),
            description: "Create or update a structured task list to track progress in the current coding session. Use this for complex multi-step tasks. Replaces any existing todo list.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": { "type": "string", "description": "Task description" },
                                "status": { "type": "string", "description": "pending, in_progress, completed, or cancelled" },
                                "priority": { "type": "string", "description": "high, medium, or low" }
                            },
                            "required": ["content", "status", "priority"]
                        },
                        "description": "Full list of tasks to track"
                    }
                },
                "required": ["todos"]
            }),
        }
    }

    async fn call(&self, args: TodoWriteArgs) -> Result<String, ToolError> {
        let mut list = TODO_LIST.lock().unwrap();
        *list = args.todos;

        if list.is_empty() {
            return Ok("Todo list cleared.".to_string());
        }

        let total = list.len();
        let completed = list.iter().filter(|t| t.status == "completed").count();
        let in_progress = list.iter().filter(|t| t.status == "in_progress").count();
        let pending = list.iter().filter(|t| t.status == "pending").count();

        let mut result = format!("Todo list ({} items, {} done):\n", total, completed);
        for item in list.iter() {
            let icon = match item.status.as_str() {
                "completed" => "[x]",
                "in_progress" => "[>]",
                "cancelled" => "[-]",
                _ => "[ ]",
            };
            result.push_str(&format!("  {} [{}] {}\n", icon, item.priority, item.content));
        }
        result.push_str(&format!("\nSummary: {} pending, {} in progress, {} completed, {} cancelled",
            pending, in_progress, completed, list.iter().filter(|t| t.status == "cancelled").count()));
        Ok(result)
    }
}

// --- AskUserQuestion ---

pub struct PendingQuestion {
    pub question: String,
    pub answer_tx: oneshot::Sender<String>,
}

pub static PENDING_QUESTION: std::sync::Mutex<Option<PendingQuestion>> = std::sync::Mutex::new(None);

#[derive(Deserialize)]
pub struct AskArgs {
    pub question: String,
}

pub struct AskUserQuestion;

impl Tool for AskUserQuestion {
    const NAME: &'static str = "ask_user_question";

    type Error = ToolError;
    type Args = AskArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "ask_user_question".to_string(),
            description: "Ask the user a question and get their typed response. Use when you need user input, clarification, or a decision.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "The question to ask the user" }
                },
                "required": ["question"]
            }),
        }
    }

    async fn call(&self, args: AskArgs) -> Result<String, ToolError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pq = PENDING_QUESTION.lock().unwrap();
            *pq = Some(PendingQuestion {
                question: args.question,
                answer_tx: tx,
            });
        }
        match rx.await {
            Ok(answer) => Ok(answer),
            Err(_) => Err(ToolError::Msg("Question cancelled by user".to_string())),
        }
    }
}
