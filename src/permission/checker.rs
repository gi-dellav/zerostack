use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use smallvec::SmallVec;

use crate::permission::pattern::Pattern;
use crate::permission::{Action, PermissionConfig, PermissionConfigs, SecurityMode, ToolPerm};

pub type PermCheck = Arc<Mutex<PermissionChecker>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    Allowed,
    AllowedWithCoaching(String),
    Ask,
    Denied(String),
}

impl CheckResult {
    pub fn allowed_with_coaching(tool: &str, _input: &str, count: usize) -> Self {
        CheckResult::AllowedWithCoaching(format!(
            "Coaching: You've called {tool} on the same input {count} times in a row. \
             This looks like a loop — try a different approach.",
        ))
    }
}

pub struct PermissionChecker {
    rules: HashMap<String, Vec<(Pattern, Action)>>,
    default_action: Action,
    ext_dir_rules: Vec<(Pattern, Action)>,
    doom_loop_action: Action,
    working_dir: String,
    session_allowlist: Vec<(String, Pattern)>,
    last_call: Option<(String, String)>,
    consecutive_repeat_count: usize,
    mode: SecurityMode,
    user_mode: SecurityMode,
    permission_modes: Vec<SecurityMode>,
    allow_all_mcp_calls: bool,
}

impl PermissionChecker {
    fn compile_config(
        config: &PermissionConfig,
        is_regex: bool,
    ) -> HashMap<String, Vec<(Pattern, Action)>> {
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
            ("mcp_tool", &config.mcp_tool),
        ] {
            let Some(tp) = tool_perm else { continue };
            let mut entries = Vec::new();
            match tp {
                ToolPerm::Simple(action) => {
                    let pat = if is_regex {
                        Pattern::new_regex(".*")
                    } else {
                        Pattern::new("*")
                    };
                    entries.push((pat, *action));
                }
                ToolPerm::Granular(map) => {
                    for (pat, action) in map {
                        let pat = if is_regex {
                            Pattern::new_regex(pat)
                        } else {
                            Pattern::new(pat)
                        };
                        entries.push((pat, *action));
                    }
                }
            }
            rules.insert(tool_name.to_string(), entries);
        }
        rules
    }

    pub fn new(
        configs: &PermissionConfigs,
        mode: SecurityMode,
        working_dir: Option<std::path::PathBuf>,
        permission_modes: Option<Vec<String>>,
    ) -> Self {
        let default_action = configs
            .glob
            .default
            .or(configs.regex.default)
            .unwrap_or(Action::Allow);
        let doom_loop_action = configs
            .glob
            .doom_loop
            .or(configs.regex.doom_loop)
            .unwrap_or(Action::Ask);

        let mut rules = Self::compile_config(&configs.glob, false);
        let regex_rules = Self::compile_config(&configs.regex, true);
        for (tool, entries) in regex_rules {
            let entry = rules.entry(tool).or_default();
            entry.extend(entries);
        }

        fn merge_entries(
            rules: &mut HashMap<String, Vec<(Pattern, Action)>>,
            entries: &Option<HashMap<String, Vec<String>>>,
            action: Action,
        ) {
            if let Some(map) = entries {
                for (tool, patterns) in map {
                    let entry = rules.entry(tool.clone()).or_default();
                    for pat in patterns {
                        entry.push((Pattern::new(pat), action));
                    }
                }
            }
        }

        merge_entries(&mut rules, &configs.glob.allow_entries, Action::Allow);
        merge_entries(&mut rules, &configs.glob.ask_entries, Action::Ask);
        merge_entries(&mut rules, &configs.glob.deny_entries, Action::Deny);

        if !rules.contains_key("bash") {
            let mut defaults = Vec::new();
            for (pat, action) in crate::permission::default_bash_rules() {
                defaults.push((Pattern::new(pat), action));
            }
            rules.insert("bash".to_string(), defaults);
        }

        for (tool, regex) in crate::permission::default_deny_regex_rules() {
            rules
                .entry(tool.to_string())
                .or_default()
                .push((Pattern::new_regex(regex), Action::Deny));
        }

        let ext_dir_rules = configs
            .glob
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

        let resolved_modes: Vec<SecurityMode> = {
            let raw = permission_modes.unwrap_or_else(|| {
                vec![
                    "guarded".to_string(),
                    "standard".to_string(),
                    "yolo".to_string(),
                ]
            });
            raw.into_iter()
                .filter_map(|s| match s.as_str() {
                    "restrictive" => Some(SecurityMode::Restrictive),
                    "readonly" => Some(SecurityMode::ReadOnly),
                    "planwrite" => Some(SecurityMode::PlanWrite),
                    "guarded" => Some(SecurityMode::Guarded),
                    "standard" => Some(SecurityMode::Standard),
                    "yolo" => Some(SecurityMode::Yolo),
                    _ => None,
                })
                .collect()
        };

        PermissionChecker {
            rules,
            default_action,
            ext_dir_rules,
            doom_loop_action,
            working_dir,
            session_allowlist: Vec::new(),
            last_call: None,
            consecutive_repeat_count: 0,
            mode,
            user_mode: mode,
            permission_modes: resolved_modes,
            allow_all_mcp_calls: false,
        }
    }

    fn apply_rules(&self) -> bool {
        self.permission_modes.contains(&self.mode) || self.mode == SecurityMode::Yolo
    }

    fn is_read_tool(&self, tool: &str) -> bool {
        matches!(tool, "read" | "grep" | "find_files" | "list_dir")
    }

    fn resolve_check_action(&self, tool: &str, matched: &SmallVec<[Action; 4]>) -> Action {
        let base = matched.last().copied();
        match self.mode {
            SecurityMode::Restrictive => base.unwrap_or(Action::Ask),
            SecurityMode::ReadOnly | SecurityMode::PlanWrite => base.unwrap_or_else(|| {
                if self.is_read_tool(tool) {
                    Action::Allow
                } else {
                    Action::Deny
                }
            }),
            SecurityMode::Guarded => base.unwrap_or_else(|| {
                if self.is_read_tool(tool) {
                    Action::Allow
                } else {
                    Action::Ask
                }
            }),
            SecurityMode::Standard => base.unwrap_or(self.default_action),
            SecurityMode::Yolo => match base {
                Some(Action::Deny) => Action::Deny,
                Some(other) => other,
                None => Action::Allow,
            },
        }
    }

    fn resolve_path_action(
        &self,
        tool: &str,
        matched: &SmallVec<[Action; 4]>,
        abs_path: &str,
    ) -> Action {
        let base = matched.last().copied();
        match self.mode {
            SecurityMode::Restrictive => base.unwrap_or(Action::Ask),
            SecurityMode::ReadOnly => base.unwrap_or_else(|| {
                if self.is_read_tool(tool) {
                    Action::Allow
                } else {
                    Action::Deny
                }
            }),
            SecurityMode::PlanWrite => base.unwrap_or_else(|| {
                if self.is_read_tool(tool) {
                    Action::Allow
                } else if matches!(tool, "write" | "edit") && is_plan_file(abs_path) {
                    Action::Allow
                } else {
                    Action::Deny
                }
            }),
            SecurityMode::Guarded => base.unwrap_or_else(|| {
                if self.is_read_tool(tool) {
                    Action::Allow
                } else {
                    Action::Ask
                }
            }),
            SecurityMode::Standard => {
                let a = base.unwrap_or(self.default_action);
                if matched.is_empty() && self.is_path_tool(tool) && !self.is_external_path(abs_path)
                {
                    Action::Allow
                } else if matched.is_empty()
                    && a == Action::Allow
                    && self.is_external_path(abs_path)
                {
                    self.match_ext_dir(abs_path).unwrap_or(Action::Ask)
                } else {
                    a
                }
            }
            SecurityMode::Yolo => match base {
                Some(Action::Deny) => Action::Deny,
                Some(other) => other,
                None => Action::Allow,
            },
        }
    }

    fn doom_loop_check(&mut self, tool: &str, doom_key: &str, action: Action) -> CheckResult {
        if action != Action::Deny {
            self.track_doom_loop(tool, doom_key);
            if self.is_doom_loop() {
                if action == Action::Allow {
                    let count = self.count_doom_loop();
                    return CheckResult::allowed_with_coaching(tool, doom_key, count);
                }
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

    pub fn check(&mut self, tool: &str, input: &str) -> CheckResult {
        if tool == "write_todo_list" {
            return CheckResult::Allowed;
        }
        if self.allow_all_mcp_calls && tool == "mcp_tool" {
            return CheckResult::Allowed;
        }
        if self.is_session_allowed(tool, input) {
            return CheckResult::Allowed;
        }
        if tool == "mcp_tool"
            && matches!(self.mode, SecurityMode::ReadOnly | SecurityMode::PlanWrite)
            && is_read_equivalent_mcp(input)
        {
            return CheckResult::Allowed;
        }

        let mut matched: SmallVec<[Action; 4]> = SmallVec::new();
        if self.apply_rules()
            && let Some(rules) = self.rules.get(tool)
        {
            for (pattern, action) in rules {
                if pattern.matches(input) {
                    matched.push(*action);
                }
            }
        }

        let action = self.resolve_check_action(tool, &matched);
        self.doom_loop_check(tool, input, action)
    }

    pub fn check_path(&mut self, tool: &str, path: &str) -> CheckResult {
        if tool == "write_todo_list" {
            return CheckResult::Allowed;
        }

        let expanded = crate::fs::expand_tilde(path);
        let abs_path = resolve_absolute(&expanded, &self.working_dir);

        if self.is_session_allowed(tool, &expanded) || self.is_session_allowed(tool, &abs_path) {
            return CheckResult::Allowed;
        }

        let mut matched: SmallVec<[Action; 4]> = SmallVec::new();
        if self.apply_rules()
            && let Some(rules) = self.rules.get(tool)
        {
            for (pattern, action) in rules {
                if pattern.matches(&abs_path) || pattern.matches(&expanded) {
                    matched.push(*action);
                }
            }
        }

        let action = self.resolve_path_action(tool, &matched, &abs_path);
        self.doom_loop_check(tool, &expanded, action)
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
        self.session_allowlist.push((tool.clone(), pattern));
        if self.is_path_tool(&tool) {
            let expanded = crate::fs::expand_tilde(pattern_str);
            let abs = resolve_absolute(&expanded, &self.working_dir);
            if abs != expanded {
                self.session_allowlist.push((tool, Pattern::new(&abs)));
            }
        }
    }

    pub fn load_session_allowlist(&mut self, entries: &[(String, String)]) {
        for (tool, pat) in entries {
            let pattern = Pattern::new(pat);
            self.session_allowlist.push((tool.clone(), pattern));
            if self.is_path_tool(tool) {
                let expanded = crate::fs::expand_tilde(pat);
                let abs = resolve_absolute(&expanded, &self.working_dir);
                if abs != expanded {
                    self.session_allowlist
                        .push((tool.clone(), Pattern::new(&abs)));
                }
            }
        }
    }

    pub fn set_mode(&mut self, mode: SecurityMode) {
        self.mode = mode;
        self.user_mode = mode;
    }

    pub fn set_prompt_mode(&mut self, mode: SecurityMode) {
        self.mode = mode;
    }

    pub fn restore_user_mode(&mut self) {
        self.mode = self.user_mode;
    }

    pub fn mode(&self) -> SecurityMode {
        self.mode
    }

    pub fn set_allow_all_mcp_calls(&mut self, allow: bool) {
        self.allow_all_mcp_calls = allow;
    }

    fn is_path_tool(&self, tool: &str) -> bool {
        matches!(tool, "read" | "write" | "edit" | "list_dir")
    }

    fn is_external_path(&self, path_str: &str) -> bool {
        let p = Path::new(path_str);
        let p = if p.is_absolute() {
            p.to_path_buf()
        } else {
            Path::new(&self.working_dir).join(p)
        };
        let cwd = Path::new(&self.working_dir);
        let normalized = normalize_path(&p);
        let normalized_cwd = normalize_path(cwd);
        !normalized.starts_with(&normalized_cwd)
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
        let current = (tool.to_string(), input.to_string());
        match &self.last_call {
            Some(prev) if *prev == current => {
                self.consecutive_repeat_count += 1;
            }
            _ => {
                self.last_call = Some(current);
                self.consecutive_repeat_count = 1;
            }
        }
    }

    fn is_doom_loop(&self) -> bool {
        self.consecutive_repeat_count >= 3
    }

    fn count_doom_loop(&self) -> usize {
        self.consecutive_repeat_count
    }
}

fn resolve_absolute(path: &str, working_dir: &str) -> String {
    let expanded = crate::fs::expand_tilde(path);
    let p = Path::new(&expanded);
    if p.is_absolute() {
        p.to_string_lossy().to_string()
    } else {
        Path::new(working_dir).join(p).to_string_lossy().to_string()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            other => {
                result.push(other);
            }
        }
    }
    result
}

fn is_plan_file(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with("PLAN") && name.ends_with(".md"))
}

fn is_read_equivalent_mcp(input: &str) -> bool {
    let lower = input.to_lowercase();
    lower.starts_with("mcp_tool:exa web search:")
        || lower.starts_with("mcp_tool:context7:")
        || lower.starts_with("mcp_tool:grep.app:")
}
