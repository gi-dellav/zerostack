use ignore::WalkBuilder;
use regex::Regex;
use rig::completion::ToolDefinition;
use rig::tool::Tool;

use crate::agent::tools::{
    AskSender, FindFilesArgs, MAX_FIND_RESULTS, PermCheck, ToolError, check_perm, is_skip_dir,
};

pub struct FindFilesTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
}

impl FindFilesTool {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        FindFilesTool { permission, ask_tx }
    }
}

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
