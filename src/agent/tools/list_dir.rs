use std::path::Path;

use ignore::WalkBuilder;
use rig::completion::ToolDefinition;
use rig::tool::Tool;

use crate::agent::tools::{
    AskSender, ListDirArgs, PermCheck, ToolError, check_perm_path, is_skip_dir,
};

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

pub struct ListDirTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
}

impl ListDirTool {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        ListDirTool { permission, ask_tx }
    }
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
        check_perm_path(&self.permission, &self.ask_tx, "list_dir", path).await?;

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
