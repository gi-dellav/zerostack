use ignore::WalkBuilder;
use regex::Regex;
use rig::tool::PortableTool as Tool;

use crate::agent::tools::{
    AskSender, FindFilesArgs, PermCheck, ToolError, check_perm, is_skip_dir,
};

pub struct FindFilesTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
    pub max_results: u64,
}

impl FindFilesTool {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>, max_results: u64) -> Self {
        FindFilesTool {
            permission,
            ask_tx,
            max_results,
        }
    }
}

impl Tool for FindFilesTool {
    const NAME: &'static str = "find_files";

    type Error = ToolError;
    type Args = FindFilesArgs;
    type Output = String;

    fn description(&self) -> String {
        "Recursively find files matching a regex pattern on the filename (e.g. `.*\\.rs$`, `config.*\\.toml`). Respects .gitignore. Skips node_modules and target.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern matched against the filename (e.g. `.*\\.rs$`). Not a glob — escape dots, use `.*` for wildcards."
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (defaults to current working directory)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, args: FindFilesArgs) -> Result<String, ToolError> {
        tracing::debug!(
            "tool find_files start: pattern={}, path={}",
            args.pattern,
            args.path.as_deref().unwrap_or("."),
        );
        let coaching =
            check_perm(&self.permission, &self.ask_tx, "find_files", &args.pattern).await?;

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

        for entry in walker
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        {
            let fname = entry.file_name().to_string_lossy();
            if re.is_match(&fname) {
                results.push(entry.path().to_string_lossy().to_string());
                if (results.len() as u64) >= self.max_results {
                    break;
                }
            }
        }

        if results.is_empty() {
            let msg = "No files found matching the pattern.".to_string();
            return Ok(match coaching {
                Some(c) => format!("{}\n\n{}", c, msg),
                None => msg,
            });
        }

        results.sort();

        let total = results.len();
        let max_results = self.max_results as usize;
        let result = if total >= max_results {
            format!(
                "{} files found (showing first {}):\n{}\n\n[truncated after {} entries — {} more; narrow the pattern or path]",
                total,
                max_results,
                results[..max_results].join("\n"),
                max_results,
                total - max_results
            )
        } else {
            format!("{} files found:\n{}", total, results.join("\n"))
        };

        tracing::debug!(
            "tool find_files done: results={}, truncated={}",
            total,
            total >= max_results,
        );
        Ok(match coaching {
            Some(c) => format!("{}\n\n{}", c, result),
            None => result,
        })
    }
}
