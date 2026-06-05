use std::path::{Path, PathBuf};

pub struct Workflow {
    pub name: String,
    pub path: PathBuf,
    pub slash_commands: Vec<String>,
    pub message: String,
}

pub struct ResolvedWorkflow {
    pub slash_commands: Vec<String>,
    pub message: String,
}

fn workflows_dir_global() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("zerostack").join("workflows"))
}

fn workflows_dir_local() -> PathBuf {
    PathBuf::from(".zerostack").join("workflows")
}

pub fn load_all() -> Vec<Workflow> {
    let mut workflows: Vec<Workflow> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    if let Some(global_dir) = workflows_dir_global()
        && global_dir.is_dir()
    {
        if let Ok(entries) = std::fs::read_dir(&global_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "workflow") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Some(mut wf) = load_file(&path) {
                            wf.name = name.to_string();
                            wf.path = path.clone();
                            seen.insert(name.to_string());
                            workflows.push(wf);
                        }
                    }
                }
            }
        }
    }

    let local_dir = workflows_dir_local();
    if local_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&local_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "workflow") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Some(mut wf) = load_file(&path) {
                            wf.name = name.to_string();
                            wf.path = path.clone();
                            if seen.contains(name) {
                                workflows.retain(|w| w.name != name);
                            }
                            seen.insert(name.to_string());
                            workflows.push(wf);
                        }
                    }
                }
            }
        }
    }

    workflows.sort_by(|a, b| a.name.cmp(&b.name));
    workflows
}

pub fn load(name: &str) -> Option<ResolvedWorkflow> {
    let local_dir = workflows_dir_local();
    if local_dir.is_dir() {
        let local_path = local_dir.join(format!("{}.workflow", name));
        if local_path.is_file() {
            if let Some(wf) = load_file(&local_path) {
                return Some(ResolvedWorkflow {
                    slash_commands: wf.slash_commands,
                    message: wf.message,
                });
            }
        }
    }

    if let Some(global_dir) = workflows_dir_global()
        && global_dir.is_dir()
    {
        let global_path = global_dir.join(format!("{}.workflow", name));
        if global_path.is_file() {
            if let Some(wf) = load_file(&global_path) {
                return Some(ResolvedWorkflow {
                    slash_commands: wf.slash_commands,
                    message: wf.message,
                });
            }
        }
    }

    None
}

fn load_file(path: &Path) -> Option<Workflow> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(parse_workflow(content))
}

pub fn parse_workflow(content: String) -> Workflow {
    let mut slash_commands: Vec<String> = Vec::new();
    let mut message_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('/') {
            slash_commands.push(trimmed.to_string());
        } else {
            message_lines.push(trimmed);
        }
    }

    Workflow {
        name: String::new(),
        path: PathBuf::new(),
        slash_commands,
        message: message_lines.join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_workflow() {
        let content = "Do the thing\nThen do another thing".to_string();
        let wf = parse_workflow(content);
        assert!(wf.slash_commands.is_empty());
        assert_eq!(wf.message, "Do the thing\nThen do another thing");
    }

    #[test]
    fn parse_workflow_with_slash_commands() {
        let content = "/model claude\n/prompt code\n\nReview the changes".to_string();
        let wf = parse_workflow(content);
        assert_eq!(wf.slash_commands, vec!["/model claude", "/prompt code"]);
        assert_eq!(wf.message, "Review the changes");
    }

    #[test]
    fn parse_workflow_comments_ignored() {
        let content = "# Set up\n/model claude\n# Now review\nReview this".to_string();
        let wf = parse_workflow(content);
        assert_eq!(wf.slash_commands, vec!["/model claude"]);
        assert_eq!(wf.message, "Review this");
    }

    #[test]
    fn parse_workflow_only_slash_commands() {
        let content = "/model claude\n/prompt code".to_string();
        let wf = parse_workflow(content);
        assert_eq!(wf.slash_commands.len(), 2);
        assert!(wf.message.is_empty());
    }

    #[test]
    fn parse_workflow_only_message() {
        let content = "Do thing one\nDo thing two".to_string();
        let wf = parse_workflow(content);
        assert!(wf.slash_commands.is_empty());
        assert_eq!(wf.message, "Do thing one\nDo thing two");
    }

    #[test]
    fn parse_workflow_blank_lines_filtered() {
        let content = "/model claude\n\n\nReview this".to_string();
        let wf = parse_workflow(content);
        assert_eq!(wf.slash_commands, vec!["/model claude"]);
        assert_eq!(wf.message, "Review this");
    }

    #[test]
    fn parse_workflow_slash_in_message() {
        let content = "Run git diff\nThen /review the output".to_string();
        let wf = parse_workflow(content);
        assert!(wf.slash_commands.is_empty());
        assert_eq!(wf.message, "Run git diff\nThen /review the output");
    }

    #[test]
    fn parse_workflow_empty() {
        let content = String::new();
        let wf = parse_workflow(content);
        assert!(wf.slash_commands.is_empty());
        assert!(wf.message.is_empty());
    }
}
