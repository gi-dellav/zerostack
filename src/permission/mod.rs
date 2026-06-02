pub mod ask;
pub mod checker;
pub mod pattern;

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ToolPerm {
    Simple(Action),
    Granular(HashMap<String, Action>),
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PermissionConfig {
    #[serde(rename = "*")]
    pub default: Option<Action>,
    pub bash: Option<ToolPerm>,
    pub read: Option<ToolPerm>,
    pub write: Option<ToolPerm>,
    pub edit: Option<ToolPerm>,
    pub grep: Option<ToolPerm>,
    pub find_files: Option<ToolPerm>,
    pub list_dir: Option<ToolPerm>,
    pub write_todo_list: Option<ToolPerm>,
    pub mcp_tool: Option<ToolPerm>,
    pub external_directory: Option<HashMap<String, Action>>,
    pub doom_loop: Option<Action>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_entries: Option<HashMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_entries: Option<HashMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny_entries: Option<HashMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Default)]
pub struct PermissionConfigs {
    pub glob: PermissionConfig,
    pub regex: PermissionConfig,
}

impl From<PermissionConfig> for PermissionConfigs {
    fn from(glob: PermissionConfig) -> Self {
        PermissionConfigs {
            glob,
            regex: PermissionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SecurityMode {
    Standard,
    Restrictive,
    ReadOnly,
    Guarded,
    Yolo,
}

impl SecurityMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "standard" => Some(SecurityMode::Standard),
            "restrictive" => Some(SecurityMode::Restrictive),
            "readonly" => Some(SecurityMode::ReadOnly),
            "guarded" => Some(SecurityMode::Guarded),
            "yolo" => Some(SecurityMode::Yolo),
            _ => None,
        }
    }
}

impl std::fmt::Display for SecurityMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecurityMode::Standard => write!(f, "standard"),
            SecurityMode::Restrictive => write!(f, "restrictive"),
            SecurityMode::ReadOnly => write!(f, "readonly"),
            SecurityMode::Guarded => write!(f, "guarded"),
            SecurityMode::Yolo => write!(f, "yolo"),
        }
    }
}

/// Parse a `%%mode=X` directive from the first line of a prompt file.
/// Returns the mode string (e.g. "restrictive", "last_user_mode") if found.
/// Also returns the content with the directive line stripped.
pub fn parse_prompt_mode(content: &str) -> (Option<&str>, &str) {
    let Some(first) = content.lines().next() else {
        return (None, content);
    };
    let trimmed = first.trim();
    if let Some(mode_str) = trimmed.strip_prefix("%%mode=") {
        let mode_str = mode_str.trim();
        if mode_str.is_empty() {
            return (None, content);
        }
        // Strip the first line from the content
        let rest = if let Some(pos) = content.find('\n') {
            &content[pos + 1..]
        } else {
            ""
        };
        (Some(mode_str), rest)
    } else {
        (None, content)
    }
}

/// Auto-deny regex patterns that are always active regardless of config.
/// These are appended to the end of each relevant tool's rules, so they
/// take precedence over user-configured allow/ask entries.
pub fn default_deny_regex_rules() -> Vec<(/* tool */ &'static str, /* regex */ &'static str)> {
    vec![("bash", r"^rm\s+.")]
}

pub fn default_bash_rules() -> Vec<(&'static str, Action)> {
    vec![
        ("ls **", Action::Allow),
        ("cd **", Action::Allow),
        ("pwd", Action::Allow),
        ("echo **", Action::Allow),
        ("which **", Action::Allow),
        ("type **", Action::Allow),
        ("cat **", Action::Allow),
        ("head **", Action::Allow),
        ("tail **", Action::Allow),
        ("wc **", Action::Allow),
        ("sort **", Action::Allow),
        ("uniq **", Action::Allow),
        ("cut **", Action::Allow),
        ("diff **", Action::Allow),
        ("grep **", Action::Allow),
        ("rg **", Action::Allow),
        ("find **", Action::Allow),
        ("fd **", Action::Allow),
        ("fdfind **", Action::Allow),
        ("git status", Action::Allow),
        ("git log **", Action::Allow),
        ("git diff **", Action::Allow),
        ("git show **", Action::Allow),
        ("git branch **", Action::Allow),
        ("cargo check", Action::Allow),
        ("cargo build", Action::Allow),
        ("cargo test", Action::Allow),
        ("cargo fmt", Action::Allow),
        ("cargo clippy", Action::Allow),
        ("cargo install **", Action::Allow),
        ("mkdir **", Action::Allow),
        ("touch **", Action::Allow),
        ("cp **", Action::Allow),
        ("npm run **", Action::Allow),
        ("pip list", Action::Allow),
        ("pip show **", Action::Allow),
        ("rm -rf /**", Action::Deny),
        ("sudo rm -rf /**", Action::Deny),
        ("dd **", Action::Deny),
        ("mkfs **", Action::Deny),
        ("fdisk **", Action::Deny),
        ("mkswap **", Action::Deny),
        ("editor **", Action::Deny),
        ("vim **", Action::Deny),
        ("vi **", Action::Deny),
        ("nano **", Action::Deny),
    ]
}
