use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::permission::pattern::Pattern;
use crate::permission::{Action, PermissionConfig, SecurityMode, ToolPerm};

pub type PermCheck = Arc<Mutex<PermissionChecker>>;

#[derive(Debug, Clone)]
pub enum CheckResult {
    Allowed,
    Ask,
    Denied(String),
}

pub struct PermissionChecker {
    rules: HashMap<String, Vec<(Pattern, Action)>>,
    default_action: Action,
    ext_dir_rules: Vec<(Pattern, Action)>,
    doom_loop_action: Action,
    working_dir: String,
    session_allowlist: Vec<(String, Pattern)>,
    recent_calls: VecDeque<(String, String)>,
    mode: SecurityMode,
}

impl PermissionChecker {
    pub fn new(
        config: &PermissionConfig,
        mode: SecurityMode,
        working_dir: Option<std::path::PathBuf>,
    ) -> Self {
        let default_action = config.default.unwrap_or(Action::Allow);
        let doom_loop_action = config.doom_loop.unwrap_or(Action::Ask);

        let mut rules: HashMap<String, Vec<(Pattern, Action)>> = HashMap::new();
        for (tool_name, tool_perm) in [
            ("bash", &config.bash),
            ("read", &config.read),
            ("write", &config.write),
            ("edit", &config.edit),
            ("grep", &config.grep),
            ("find_files", &config.find_files),
            ("list_dir", &config.list_dir),
            ("write_todo_list", &config.write_todo_list),
        ] {
            let Some(tp) = tool_perm else { continue };
            let mut entries = Vec::new();
            match tp {
                ToolPerm::Simple(action) => {
                    entries.push((Pattern::new("*"), *action));
                }
                ToolPerm::Granular(map) => {
                    for (pat, action) in map {
                        entries.push((Pattern::new(pat), *action));
                    }
                }
            }
            rules.insert(tool_name.to_string(), entries);
        }

        let ext_dir_rules = config
            .external_directory
            .as_ref()
            .map(|map| {
                map.iter()
                    .map(|(pat, action)| (Pattern::new(pat), *action))
                    .collect()
            })
            .unwrap_or_default();

        let working_dir = working_dir
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .to_string_lossy()
            .to_string();

        PermissionChecker {
            rules,
            default_action,
            ext_dir_rules,
            doom_loop_action,
            working_dir,
            session_allowlist: Vec::new(),
            recent_calls: VecDeque::with_capacity(16),
            mode,
        }
    }

    pub fn check(&mut self, tool: &str, input: &str) -> CheckResult {
        if self.mode == SecurityMode::Yolo {
            return CheckResult::Allowed;
        }

        if self.is_session_allowed(tool, input) {
            return CheckResult::Allowed;
        }

        let mut matched: Vec<Action> = Vec::new();
        if let Some(rules) = self.rules.get(tool) {
            for (pattern, action) in rules {
                if pattern.matches(input) {
                    matched.push(*action);
                }
            }
        }

        let base = matched.last().copied().unwrap_or(self.default_action);
        let action = match self.mode {
            SecurityMode::Restrictive => {
                if matched.is_empty() && self.default_action == Action::Allow {
                    Action::Ask
                } else {
                    base
                }
            }
            SecurityMode::Accept => match base {
                Action::Ask => {
                    if self.is_path_tool(tool) && self.is_external_path(input) {
                        self.match_ext_dir(input).unwrap_or(Action::Ask)
                    } else {
                        Action::Allow
                    }
                }
                other => other,
            },
            SecurityMode::Standard => base,
            SecurityMode::Yolo => unreachable!(),
        };

        if action != Action::Deny {
            self.track_doom_loop(tool, input);
            if self.is_doom_loop(tool, input) {
                match self.doom_loop_action {
                    Action::Deny => {
                        return CheckResult::Denied(
                            "Doom loop: repeated identical tool call".to_string(),
                        );
                    }
                    Action::Ask => return CheckResult::Ask,
                    Action::Allow => {}
                }
            }
        }

        match action {
            Action::Allow => CheckResult::Allowed,
            Action::Ask => CheckResult::Ask,
            Action::Deny => CheckResult::Denied("Blocked by permission rules".to_string()),
        }
    }

    pub fn check_path(&mut self, tool: &str, path: &str) -> CheckResult {
        if self.mode == SecurityMode::Yolo {
            return CheckResult::Allowed;
        }

        if self.is_session_allowed(tool, path) {
            return CheckResult::Allowed;
        }

        let abs_path = resolve_absolute(path);
        let mut matched: Vec<Action> = Vec::new();
        if let Some(rules) = self.rules.get(tool) {
            for (pattern, action) in rules {
                if pattern.matches(&abs_path) || pattern.matches(path) {
                    matched.push(*action);
                }
            }
        }

        let base = matched.last().copied().unwrap_or(self.default_action);
        let action = match self.mode {
            SecurityMode::Restrictive => {
                if matched.is_empty() && self.default_action == Action::Allow {
                    Action::Ask
                } else {
                    base
                }
            }
            SecurityMode::Accept => match base {
                Action::Ask => {
                    if self.is_external_path(&abs_path) {
                        self.match_ext_dir(&abs_path).unwrap_or(Action::Ask)
                    } else {
                        Action::Allow
                    }
                }
                other => other,
            },
            SecurityMode::Standard => base,
            SecurityMode::Yolo => unreachable!(),
        };

        if action != Action::Deny {
            self.track_doom_loop(tool, path);
            if self.is_doom_loop(tool, path) {
                match self.doom_loop_action {
                    Action::Deny => {
                        return CheckResult::Denied(
                            "Doom loop: repeated identical tool call".to_string(),
                        );
                    }
                    Action::Ask => return CheckResult::Ask,
                    Action::Allow => {}
                }
            }
        }

        match action {
            Action::Allow => CheckResult::Allowed,
            Action::Ask => CheckResult::Ask,
            Action::Deny => CheckResult::Denied("Blocked by permission rules".to_string()),
        }
    }

    fn is_session_allowed(&self, tool: &str, input: &str) -> bool {
        for (allowed_tool, pattern) in &self.session_allowlist {
            if allowed_tool == tool && pattern.matches(input) {
                return true;
            }
        }
        false
    }

    pub fn add_session_allowlist(&mut self, tool: String, pattern_str: &str) {
        let pattern = Pattern::new(pattern_str);
        self.session_allowlist.push((tool, pattern));
    }

    pub fn load_session_allowlist(&mut self, entries: &[(String, String)]) {
        for (tool, pat) in entries {
            self.session_allowlist
                .push((tool.clone(), Pattern::new(pat)));
        }
    }

    #[allow(dead_code)]
    pub fn allowlist_entries(&self) -> Vec<(String, String)> {
        self.session_allowlist
            .iter()
            .map(|(t, p)| (t.clone(), p.original.clone()))
            .collect()
    }

    pub fn set_mode(&mut self, mode: SecurityMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> SecurityMode {
        self.mode
    }

    fn is_path_tool(&self, tool: &str) -> bool {
        matches!(tool, "read" | "write" | "edit" | "list_dir")
    }

    fn is_external_path(&self, path_str: &str) -> bool {
        let p = Path::new(path_str);
        if !p.is_absolute() {
            return false;
        }
        let cwd = Path::new(&self.working_dir);
        !p.starts_with(cwd)
    }

    fn match_ext_dir(&self, path_str: &str) -> Option<Action> {
        for (pattern, action) in &self.ext_dir_rules {
            if pattern.matches(path_str) {
                return Some(*action);
            }
        }
        None
    }

    fn track_doom_loop(&mut self, tool: &str, input: &str) {
        self.recent_calls
            .push_back((tool.to_string(), input.to_string()));
        if self.recent_calls.len() > 16 {
            self.recent_calls.pop_front();
        }
    }

    fn is_doom_loop(&self, tool: &str, input: &str) -> bool {
        let count = self
            .recent_calls
            .iter()
            .filter(|(t, i)| t == tool && i == input)
            .count();
        count >= 3
    }
}

fn resolve_absolute(path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_string_lossy().to_string()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(p)
            .to_string_lossy()
            .to_string()
    }
}
