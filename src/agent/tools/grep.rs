use ignore::WalkBuilder;
use regex::Regex;
use rig::completion::ToolDefinition;
use rig::tool::Tool;

use crate::agent::tools::{
    AskSender, GrepArgs, MAX_GREP_RESULTS, PermCheck, ToolError, check_perm, is_skip_dir,
};

pub struct GrepTool {
    pub permission: Option<PermCheck>,
    pub ask_tx: Option<AskSender>,
}

impl GrepTool {
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        GrepTool { permission, ask_tx }
    }

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
        check_perm(&self.permission, &self.ask_tx, "grep", &args.pattern).await?;

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

        for entry in walker
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        {
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

                    let match_lines: Vec<usize> = lines
                        .iter()
                        .enumerate()
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
                                all_results.push(format!(
                                    "{}-{}{} {}",
                                    path_str,
                                    i + 1,
                                    sep,
                                    lines[i]
                                ));
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
