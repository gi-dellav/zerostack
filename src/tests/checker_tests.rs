use crate::permission::checker::{CheckResult, PermissionChecker};
use crate::permission::{Action, PermissionConfig, PermissionConfigs, SecurityMode, ToolPerm};

fn default_modes() -> Option<Vec<String>> {
    Some(vec![
        "guarded".to_string(),
        "standard".to_string(),
        "yolo".to_string(),
    ])
}

fn make_checker(mode: SecurityMode) -> PermissionChecker {
    PermissionChecker::new(
        &PermissionConfigs::default(),
        mode,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    )
}

#[allow(dead_code)]
fn make_checker_with_modes(mode: SecurityMode, modes: Option<Vec<String>>) -> PermissionChecker {
    PermissionChecker::new(
        &PermissionConfigs::default(),
        mode,
        Some(std::path::PathBuf::from("/home/user/project")),
        modes,
    )
}

fn configs_from(config: PermissionConfig) -> PermissionConfigs {
    PermissionConfigs::from(config)
}

// --- SecurityMode behavior ---

#[test]
fn readonly_denies_write_bash_and_edit() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    assert!(matches!(
        checker.check("write", "/etc/passwd"),
        CheckResult::Denied(_)
    ));
    assert!(matches!(
        checker.check("edit", "src/main.rs"),
        CheckResult::Denied(_)
    ));
    assert!(matches!(
        checker.check("bash", "ls"),
        CheckResult::Denied(_)
    ));
    assert!(matches!(
        checker.check("bash", "rm -rf /"),
        CheckResult::Denied(_)
    ));
}

#[test]
fn yolo_denies_destructive_bash() {
    let mut checker = make_checker(SecurityMode::Yolo);
    let result = checker.check("bash", "rm -rf /");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for rm -rf / in YOLO, got {:?}",
        result,
    );
}

#[test]
fn yolo_denies_destructive_bash_with_pattern() {
    let mut checker = make_checker(SecurityMode::Yolo);
    let result = checker.check("bash", "dd if=/dev/zero of=/dev/sda");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for dd in YOLO, got {:?}",
        result,
    );
}

#[test]
fn restrictive_makes_unconfigured_tool_ask() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    let result = checker.check("some_tool", "any input");
    assert!(matches!(result, CheckResult::Ask));
}

#[test]
fn standard_allows_unknown_tool_with_default() {
    let mut checker = make_checker(SecurityMode::Standard);
    let result = checker.check("some_tool", "any input");
    assert!(matches!(result, CheckResult::Allowed));
}

// --- ReadOnly mode ---

#[test]
fn readonly_allows_read_tools() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    assert!(matches!(
        checker.check("read", "/etc/passwd"),
        CheckResult::Allowed
    ));
    assert!(matches!(
        checker.check("grep", "pattern"),
        CheckResult::Allowed
    ));
    assert!(matches!(
        checker.check("find_files", "*.rs"),
        CheckResult::Allowed
    ));
    assert!(matches!(
        checker.check("list_dir", "/home/user"),
        CheckResult::Allowed
    ));
}

#[test]
fn readonly_denies_path_tools_outside_read() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    assert!(matches!(
        checker.check_path("write", "/home/user/project/new.rs"),
        CheckResult::Denied(_),
    ));
    assert!(matches!(
        checker.check_path("edit", "/home/user/project/src/main.rs"),
        CheckResult::Denied(_),
    ));
}

// --- Guarded mode ---

#[test]
fn guarded_allows_read_tools() {
    let mut checker = make_checker(SecurityMode::Guarded);
    assert!(matches!(
        checker.check("read", "/etc/passwd"),
        CheckResult::Allowed
    ));
    assert!(matches!(
        checker.check("grep", "pattern"),
        CheckResult::Allowed
    ));
    assert!(matches!(
        checker.check("list_dir", "/home/user"),
        CheckResult::Allowed
    ));
}

#[test]
fn guarded_asks_for_write_and_bash() {
    let mut checker = make_checker(SecurityMode::Guarded);
    assert!(matches!(
        checker.check("write", "/etc/passwd"),
        CheckResult::Ask
    ));
    assert!(matches!(
        checker.check("edit", "src/main.rs"),
        CheckResult::Ask
    ));
    // Bash: no default rule matches (it's a different pattern)
    assert!(matches!(checker.check("bash", "wget"), CheckResult::Ask));
    // But configured defaults like ls still apply
    assert!(matches!(
        checker.check("bash", "ls -la"),
        CheckResult::Allowed
    ));
}

// --- Deny rules ---

#[test]
fn deny_rule_blocks_regardless_of_mode() {
    let mut checker = make_checker(SecurityMode::Standard);
    let result = checker.check("bash", "rm -rf /home/user/project");
    assert!(matches!(result, CheckResult::Denied(_)));
}

#[test]
fn deny_rule_is_denied_in_yolo() {
    let mut checker = make_checker(SecurityMode::Yolo);
    let result = checker.check("bash", "rm -rf /home/user/project");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for destructive bash in YOLO, got {:?}",
        result,
    );
}

// --- Doom loop detection ---

#[test]
fn doom_loop_triggers_after_three_repeated_calls() {
    let mut checker = make_checker(SecurityMode::Standard);
    checker.check("bash", "ls");
    checker.check("bash", "ls");
    let result = checker.check("bash", "ls");
    assert!(
        matches!(result, CheckResult::AllowedWithCoaching(_)),
        "expected AllowedWithCoaching from doom loop in Standard, got {:?}",
        result,
    );
}

#[test]
fn doom_loop_does_not_trigger_before_three() {
    let mut checker = make_checker(SecurityMode::Standard);
    checker.check("bash", "ls");
    let result = checker.check("bash", "ls");
    assert!(matches!(result, CheckResult::Allowed));
}

#[test]
fn doom_loop_resets_for_different_inputs() {
    let mut checker = make_checker(SecurityMode::Standard);
    checker.check("bash", "ls");
    checker.check("bash", "ls");
    checker.check("bash", "pwd");
    let result = checker.check("bash", "pwd");
    assert!(matches!(result, CheckResult::Allowed));
}

#[test]
fn doom_loop_detects_consecutive_repeats() {
    let mut checker = make_checker(SecurityMode::Standard);
    checker.check("bash", "ls");
    checker.check("bash", "ls");
    let result = checker.check("bash", "ls");
    assert!(
        matches!(result, CheckResult::AllowedWithCoaching(_)),
        "three consecutive identical calls should trigger doom loop coaching, got {:?}",
        result,
    );
}

#[cfg(feature = "hooks")]
#[test]
fn record_blocked_feeds_doom_loop_detection() {
    let mut checker = make_checker(SecurityMode::Standard);
    checker.record_blocked("bash", "ls -la");
    checker.record_blocked("bash", "ls -la");
    let result = checker.check("bash", "ls -la");
    assert!(
        matches!(result, CheckResult::AllowedWithCoaching(_)),
        "a hook-denied call repeated via record_blocked should still count toward \
         doom-loop detection, got {:?}",
        result,
    );
}

#[cfg(feature = "hooks")]
#[test]
fn force_ask_once_forces_ask_for_the_next_call_regardless_of_mode() {
    // Yolo would otherwise allow bash unconditionally.
    let mut checker = make_checker(SecurityMode::Yolo);
    checker.force_ask_once("bash".to_string());
    let result = checker.check("bash", "ls -la");
    assert!(matches!(result, CheckResult::Ask));
}

#[cfg(feature = "hooks")]
#[test]
fn force_ask_once_is_consumed_after_one_call() {
    let mut checker = make_checker(SecurityMode::Yolo);
    checker.force_ask_once("bash".to_string());
    let _ = checker.check("bash", "ls -la");
    let result = checker.check("bash", "ls -la");
    assert!(matches!(result, CheckResult::Allowed));
}

#[cfg(feature = "hooks")]
#[test]
fn force_ask_once_never_overrides_a_deny_rule() {
    let config = PermissionConfig {
        bash: Some(ToolPerm::Granular({
            let mut m = std::collections::HashMap::new();
            m.insert("rm -rf important".to_string(), Action::Deny);
            m
        })),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Yolo,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    );
    checker.force_ask_once("bash".to_string());
    let result = checker.check("bash", "rm -rf important");
    assert!(matches!(result, CheckResult::Denied(_)));
}

#[cfg(feature = "hooks")]
#[test]
fn allow_once_suppresses_the_prompt_for_the_next_call() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    checker.allow_once("bash".to_string());
    let result = checker.check("bash", "ls -la");
    assert!(matches!(result, CheckResult::Allowed));
}

#[cfg(feature = "hooks")]
#[test]
fn allow_once_is_consumed_after_one_call() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    checker.allow_once("bash".to_string());
    let _ = checker.check("bash", "ls -la");
    let result = checker.check("bash", "ls -la");
    assert!(matches!(result, CheckResult::Ask));
}

#[cfg(feature = "hooks")]
#[test]
fn allow_once_never_overrides_a_deny_rule() {
    let config = PermissionConfig {
        bash: Some(ToolPerm::Granular({
            let mut m = std::collections::HashMap::new();
            m.insert("rm -rf important".to_string(), Action::Deny);
            m
        })),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Yolo,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    );
    checker.allow_once("bash".to_string());
    let result = checker.check("bash", "rm -rf important");
    assert!(matches!(result, CheckResult::Denied(_)));
}

// --- Session allowlist ---

#[test]
fn session_allowlist_bypasses_rules() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    checker.add_session_allowlist("bash".into(), "cargo test **");
    let result = checker.check("bash", "cargo test --all");
    assert!(matches!(result, CheckResult::Allowed));
}

#[test]
fn session_allowlist_cannot_bypass_deny_rules() {
    // Security fix: deny rules must be evaluated before the session allowlist
    // so that AllowAlways cannot bypass a deny rule.
    let config = PermissionConfig {
        bash: Some(ToolPerm::Granular(
            [("rm *".to_string(), Action::Deny)].into_iter().collect(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        None,
        default_modes(),
    );
    checker.add_session_allowlist("bash".into(), "rm *");
    let result = checker.check("bash", "rm -rf important");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "deny rule must not be bypassed by session allowlist, got {:?}",
        result,
    );
}

#[test]
fn session_allowlist_is_tool_specific() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    checker.add_session_allowlist("read".into(), "**");
    assert!(matches!(
        checker.check("read", "/etc/passwd"),
        CheckResult::Allowed
    ));
    assert!(matches!(
        checker.check("write", "some/file.txt"),
        CheckResult::Ask
    ));
}

// --- External path detection ---

#[test]
fn external_absolute_path_outside_cwd_is_detected() {
    let mut checker = make_checker(SecurityMode::Standard);
    let external_path = if cfg!(windows) {
        "D:\\outside\\secret.txt"
    } else {
        "/etc/shadow"
    };
    let result = checker.check_path("write", external_path);
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask, got {:?}",
        result,
    );
}

#[test]
fn relative_path_is_not_external() {
    let mut checker = make_checker(SecurityMode::Standard);
    let result = checker.check_path("read", "src/lib.rs");
    assert!(matches!(result, CheckResult::Allowed));
}

// --- Config-driven rules ---

#[test]
fn explicit_granular_rules_take_effect() {
    let config = PermissionConfig {
        read: Some(ToolPerm::Granular(
            [
                ("*.md".to_string(), Action::Allow),
                ("*.rs".to_string(), Action::Ask),
            ]
            .into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        None,
        default_modes(),
    );
    assert_eq!(checker.check("read", "README.md"), CheckResult::Allowed);
    assert_eq!(checker.check("read", "main.rs"), CheckResult::Ask);
}

// --- Standard mode: allow path tools in CWD only when no rule matches ---

#[test]
fn standard_path_tools_in_cwd_without_rules_are_allowed() {
    let mut checker = make_checker(SecurityMode::Standard);
    assert!(matches!(
        checker.check_path("read", "/home/user/project/src/main.rs"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check_path("write", "/home/user/project/new_file.rs"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check_path("list_dir", "/home/user/project/src"),
        CheckResult::Allowed,
    ));
}

#[test]
fn standard_respects_deny_rules_for_path_tools_in_cwd() {
    // Config rules are more dominant than mode defaults, so explicit Deny rules win.
    // Use ** pattern to match paths with slashes.
    let config = PermissionConfig {
        read: Some(ToolPerm::Granular(
            [("**".to_string(), Action::Deny)].into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    );
    let result = checker.check_path("read", "/home/user/project/src/main.rs");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for CWD path with explicit deny rule, got {:?}",
        result,
    );
}

#[test]
fn standard_respects_deny_rules_for_write_in_cwd() {
    let config = PermissionConfig {
        write: Some(ToolPerm::Granular(
            [("**".to_string(), Action::Deny)].into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    );
    let result = checker.check_path("write", "/home/user/project/new_file.rs");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for CWD write with explicit deny rule, got {:?}",
        result,
    );
}

#[test]
fn standard_asks_external_path_even_for_path_tools() {
    // External paths should still trigger Ask in Standard mode
    let mut checker = make_checker(SecurityMode::Standard);
    let external = if cfg!(windows) {
        "D:\\outside\\file.txt"
    } else {
        "/etc/config.conf"
    };
    let result = checker.check_path("read", external);
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask for external path, got {:?}",
        result,
    );
}

#[test]
fn standard_deny_still_works_for_non_path_tools() {
    // Non-path tools (bash, grep, etc.) should still respect deny rules
    let mut checker = make_checker(SecurityMode::Standard);
    let result = checker.check("bash", "rm -rf /home/user/project");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for bash deny rule, got {:?}",
        result,
    );
}

#[test]
fn standard_allows_configured_bash_commands() {
    let mut checker = make_checker(SecurityMode::Standard);
    assert!(matches!(
        checker.check("bash", "ls -la"),
        CheckResult::Allowed
    ));
    assert!(matches!(
        checker.check("bash", "git status"),
        CheckResult::Allowed
    ));
    assert!(matches!(
        checker.check("bash", "cargo build"),
        CheckResult::Allowed
    ));
}

// --- Regex permission rules ---

#[test]
fn regex_granular_rules_take_effect() {
    let config = PermissionConfig {
        read: Some(ToolPerm::Granular(
            [
                (r"\.md$".to_string(), Action::Allow),
                (r"\.rs$".to_string(), Action::Ask),
            ]
            .into(),
        )),
        ..PermissionConfig::default()
    };
    let configs = PermissionConfigs {
        regex: config,
        ..PermissionConfigs::default()
    };
    let mut checker =
        PermissionChecker::new(&configs, SecurityMode::Standard, None, default_modes());
    assert_eq!(checker.check("read", "README.md"), CheckResult::Allowed);
    assert_eq!(checker.check("read", "main.rs"), CheckResult::Ask);
    assert_eq!(checker.check("read", "main.py"), CheckResult::Allowed);
}

#[test]
fn regex_simple_action() {
    let config = PermissionConfig {
        bash: Some(ToolPerm::Simple(Action::Ask)),
        ..PermissionConfig::default()
    };
    let configs = PermissionConfigs {
        regex: config,
        ..PermissionConfigs::default()
    };
    let mut checker =
        PermissionChecker::new(&configs, SecurityMode::Standard, None, default_modes());
    let result = checker.check("bash", "anything");
    assert!(matches!(result, CheckResult::Ask));
}

#[test]
fn regex_and_glob_rules_merge() {
    let glob = PermissionConfig {
        read: Some(ToolPerm::Granular(
            [("*.md".to_string(), Action::Allow)].into(),
        )),
        ..PermissionConfig::default()
    };
    let regex = PermissionConfig {
        read: Some(ToolPerm::Granular(
            [(r"\.rs$".to_string(), Action::Ask)].into(),
        )),
        ..PermissionConfig::default()
    };
    let configs = PermissionConfigs { glob, regex };
    let mut checker =
        PermissionChecker::new(&configs, SecurityMode::Standard, None, default_modes());
    assert_eq!(checker.check("read", "README.md"), CheckResult::Allowed);
    assert_eq!(checker.check("read", "main.rs"), CheckResult::Ask);
}

#[test]
fn regex_default_action_used_when_no_glob_default() {
    let glob = PermissionConfig::default();
    let regex = PermissionConfig {
        default: Some(Action::Ask),
        ..PermissionConfig::default()
    };
    let configs = PermissionConfigs { glob, regex };
    let mut checker =
        PermissionChecker::new(&configs, SecurityMode::Standard, None, default_modes());
    // Default from regex config should be used when glob has no default
    let result = checker.check("unknown_tool", "anything");
    assert!(matches!(result, CheckResult::Ask));
}

#[test]
fn regex_glob_default_precedence() {
    let glob = PermissionConfig {
        default: Some(Action::Allow),
        ..PermissionConfig::default()
    };
    let regex = PermissionConfig {
        default: Some(Action::Ask),
        ..PermissionConfig::default()
    };
    let configs = PermissionConfigs { glob, regex };
    let mut checker =
        PermissionChecker::new(&configs, SecurityMode::Standard, None, default_modes());
    // Glob default should take precedence over regex default
    let result = checker.check("unknown_tool", "anything");
    assert!(matches!(result, CheckResult::Allowed));
}

// --- Path traversal detection (normalize_path) ---

#[test]
fn path_traversal_with_dotdot_is_detected_as_external() {
    let mut checker = make_checker(SecurityMode::Standard);
    let traversal = if cfg!(windows) {
        "C:\\home\\user\\project\\..\\etc\\shadow"
    } else {
        "/home/user/project/../etc/shadow"
    };
    let result = checker.check_path("read", traversal);
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask for traversal path, got {:?}",
        result,
    );
}

#[test]
fn dot_components_are_normalized_away() {
    let mut checker = make_checker(SecurityMode::Standard);
    let path = if cfg!(windows) {
        "C:\\home\\user\\project\\.\\src\\main.rs"
    } else {
        "/home/user/project/./src/main.rs"
    };
    let result = checker.check_path("read", path);
    assert!(
        matches!(result, CheckResult::Allowed),
        "expected Allowed for dot-normalized CWD path, got {:?}",
        result,
    );
}

#[test]
fn nested_dotdot_traverses_to_root() {
    let mut checker = make_checker(SecurityMode::Standard);
    let traversal = if cfg!(windows) {
        "C:\\home\\user\\project\\..\\..\\..\\etc\\passwd"
    } else {
        "/home/user/project/../../../etc/passwd"
    };
    let result = checker.check_path("read", traversal);
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask for deep traversal path, got {:?}",
        result,
    );
}

#[test]
fn relative_dotdot_traversal_is_detected_as_external() {
    let mut checker = make_checker(SecurityMode::Standard);
    let traversal = if cfg!(windows) {
        "..\\..\\..\\etc\\passwd"
    } else {
        "../../../etc/passwd"
    };
    let result = checker.check_path("read", traversal);
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask for relative traversal path, got {:?}",
        result,
    );
}

#[test]
fn relative_dotdot_in_cwd_stays_allowed() {
    let mut checker = make_checker(SecurityMode::Standard);
    let path = if cfg!(windows) {
        "..\\project\\src\\main.rs"
    } else {
        "../project/src/main.rs"
    };
    let result = checker.check_path("read", path);
    assert!(
        matches!(result, CheckResult::Allowed),
        "expected Allowed for relative path staying in CWD, got {:?}",
        result,
    );
}

// --- Session allowlist with absolute paths on check_path ---

#[test]
fn session_allowlist_matches_absolute_path_when_stored_as_relative() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    checker.add_session_allowlist("read".into(), "src/*");
    let result = checker.check_path("read", "/home/user/project/src/main.rs");
    assert!(
        matches!(result, CheckResult::Allowed),
        "expected Allowed for absolute path matching relative allowlist, got {:?}",
        result,
    );
}

#[test]
fn session_allowlist_matches_relative_path_when_stored_as_absolute() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    checker.add_session_allowlist("read".into(), "/home/user/project/src/*");
    let result = checker.check_path("read", "src/main.rs");
    assert!(
        matches!(result, CheckResult::Allowed),
        "expected Allowed for relative path matching absolute allowlist, got {:?}",
        result,
    );
}

// --- MCP tool config ---

#[test]
fn mcp_tool_simple_rule_is_respected() {
    let config = PermissionConfig {
        mcp_tool: Some(ToolPerm::Simple(Action::Deny)),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        None,
        default_modes(),
    );
    let result = checker.check("mcp_tool", "mcp_tool:filesystem:read_file");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for MCP tool, got {:?}",
        result,
    );
}

#[test]
fn mcp_tool_granular_rules_respected() {
    let config = PermissionConfig {
        mcp_tool: Some(ToolPerm::Granular(
            [
                ("mcp_tool:fs:allow_*".to_string(), Action::Allow),
                ("mcp_tool:fs:deny_*".to_string(), Action::Deny),
            ]
            .into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        None,
        default_modes(),
    );
    assert_eq!(
        checker.check("mcp_tool", "mcp_tool:fs:allow_read"),
        CheckResult::Allowed
    );
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:fs:deny_write"),
        CheckResult::Denied(_)
    ));
}

#[test]
fn mcp_tool_default_action_when_no_rules() {
    let mut checker = make_checker(SecurityMode::Standard);
    let result = checker.check("mcp_tool", "mcp_tool:some_server:some_tool");
    assert!(
        matches!(result, CheckResult::Allowed),
        "expected Allowed for MCP tool with no rules (default), got {:?}",
        result,
    );
}

// --- Restricted mode: ask for everything ---

#[test]
fn restrictive_asks_for_everything() {
    // With default modes (Restrictive not in list), no rules apply -> always Ask
    let mut checker = make_checker(SecurityMode::Restrictive);
    assert!(matches!(
        checker.check("read", "anything"),
        CheckResult::Ask
    ));
    assert!(matches!(
        checker.check("write", "anything"),
        CheckResult::Ask
    ));
    assert!(matches!(checker.check("bash", "ls"), CheckResult::Ask));
    assert!(matches!(
        checker.check("bash", "rm -rf /"),
        CheckResult::Ask
    ));
}

#[test]
fn restrictive_with_rules_in_permission_modes_respects_matched() {
    // When Restrictive is explicitly added to permission_modes, matched rules are respected.
    // Use ** pattern to match inputs with slashes.
    let config = PermissionConfig {
        read: Some(ToolPerm::Granular(
            [("**".to_string(), Action::Allow)].into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Restrictive,
        Some(std::path::PathBuf::from("/home/user/project")),
        Some(vec!["restrictive".to_string(), "standard".to_string()]),
    );
    // read has an explicit Allow for ** -> Allowed
    assert!(matches!(
        checker.check("read", "/etc/passwd"),
        CheckResult::Allowed
    ));
    // write has no rule -> unmatched -> Ask
    assert!(matches!(
        checker.check("write", "anything"),
        CheckResult::Ask
    ));
}

// --- Permission modes filtering ---

#[test]
fn apply_rules_skipped_when_mode_not_in_permission_modes() {
    let config = PermissionConfig {
        bash: Some(ToolPerm::Granular(
            [("safe-*".to_string(), Action::Allow)].into(),
        )),
        ..PermissionConfig::default()
    };
    // Guarded is NOT in the modes list -> rules not applied
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Guarded,
        Some(std::path::PathBuf::from("/home/user/project")),
        Some(vec!["standard".to_string()]),
    );
    // Without rules, Guarded asks for non-read tools
    let result = checker.check("bash", "safe-command");
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask when rules are skipped by permission_modes, got {:?}",
        result,
    );
}

#[test]
fn apply_rules_applied_when_mode_in_permission_modes() {
    let config = PermissionConfig {
        bash: Some(ToolPerm::Granular(
            [("safe-*".to_string(), Action::Allow)].into(),
        )),
        ..PermissionConfig::default()
    };
    // Standard IS in the modes list -> rules apply
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        Some(std::path::PathBuf::from("/home/user/project")),
        Some(vec!["standard".to_string()]),
    );
    let result = checker.check("bash", "safe-command");
    assert!(
        matches!(result, CheckResult::Allowed),
        "expected Allowed when rules apply via permission_modes, got {:?}",
        result,
    );
}

// --- Guarded respects config rules ---

#[test]
fn guarded_respects_explicit_config_allow() {
    let config = PermissionConfig {
        bash: Some(ToolPerm::Granular(
            [("wget **".to_string(), Action::Allow)].into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Guarded,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    );
    // bash has an explicit Allow rule for wget -> Allowed
    assert!(matches!(
        checker.check("bash", "wget http://example.com"),
        CheckResult::Allowed
    ));
    // Other bash commands (no rule) -> Ask (mode default for non-read in Guarded)
    assert!(matches!(
        checker.check("bash", "unknown-cmd"),
        CheckResult::Ask
    ));
}

#[test]
fn guarded_respects_explicit_config_deny() {
    let config = PermissionConfig {
        read: Some(ToolPerm::Granular(
            [("*.secret".to_string(), Action::Deny)].into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Guarded,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    );
    // read has explicit Deny for .secret files -> Denied
    assert!(matches!(
        checker.check("read", "private.secret"),
        CheckResult::Denied(_)
    ));
    // Other reads (no rule) -> Allowed (read is a read tool)
    assert!(matches!(
        checker.check("read", "README.md"),
        CheckResult::Allowed
    ));
}

// --- Standard mode: external path handling with unmatched rules ---

#[test]
fn standard_external_path_with_default_allow_asks() {
    // Default allow (no config override) + external path = Ask
    let mut checker = make_checker(SecurityMode::Standard);
    let result = checker.check_path("write", "/tmp/outside.txt");
    assert!(matches!(result, CheckResult::Ask));
}

// --- YOLO: standard mode fallback for unknown commands ---

#[test]
fn yolo_unknown_bash_is_allowed() {
    // Commands not in default_bash_rules are not matched -> base is None -> YOLO returns Allow
    let mut checker = make_checker(SecurityMode::Yolo);
    assert!(matches!(
        checker.check("bash", "ed somefile"),
        CheckResult::Allowed
    ));
}

#[test]
fn yolo_allows_todo_write() {
    let mut checker = make_checker(SecurityMode::Yolo);
    assert!(matches!(
        checker.check("todo_write", ""),
        CheckResult::Allowed
    ));
}

// --- MCP allow-all via checker ---

#[test]
fn allow_all_mcp_does_not_override_deny_rules() {
    // Security fix: deny rules are the baseline and must be evaluated before
    // allow_all_mcp_calls so that deny rules cannot be bypassed.
    let config = PermissionConfig {
        mcp_tool: Some(ToolPerm::Simple(Action::Deny)),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        None,
        default_modes(),
    );
    checker.set_allow_all_mcp_calls(true);
    let result = checker.check("mcp_tool", "mcp_tool:filesystem:read_file");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for MCP tool with deny rule even when allow_all_mcp_calls is set, got {:?}",
        result,
    );
}

#[test]
fn allow_all_mcp_does_not_affect_non_mcp_tools() {
    let config = PermissionConfig {
        bash: Some(ToolPerm::Simple(Action::Deny)),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        None,
        default_modes(),
    );
    checker.set_allow_all_mcp_calls(true);
    let result = checker.check("bash", "ls");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for bash even with allow_all_mcp_calls, got {:?}",
        result,
    );
}

// --- todo_write always allowed ---

#[test]
fn todo_write_always_allowed_in_restrictive() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    assert!(matches!(
        checker.check("todo_write", ""),
        CheckResult::Allowed
    ));
}

#[test]
fn todo_write_always_allowed_in_readonly() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    assert!(matches!(
        checker.check("todo_write", ""),
        CheckResult::Allowed
    ));
}

#[test]
fn todo_write_always_allowed_in_guarded() {
    let mut checker = make_checker(SecurityMode::Guarded);
    assert!(matches!(
        checker.check("todo_write", ""),
        CheckResult::Allowed
    ));
}

#[test]
fn todo_write_always_allowed_in_yolo() {
    let mut checker = make_checker(SecurityMode::Yolo);
    assert!(matches!(
        checker.check("todo_write", ""),
        CheckResult::Allowed
    ));
}

#[test]
fn todo_write_path_check_always_allowed() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    assert!(matches!(
        checker.check_path("todo_write", "/any/path"),
        CheckResult::Allowed
    ));
}

// --- Empty permission_modes (all modes skip config rules) ---

#[test]
fn empty_permission_modes_skips_rules_for_all_modes() {
    let config = PermissionConfig {
        read: Some(ToolPerm::Simple(Action::Allow)),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        Some(std::path::PathBuf::from("/home/user/project")),
        Some(vec![]), // empty list: no modes apply rules
    );
    // Standard with no rules applied: path tools in CWD still get auto-allow
    assert!(matches!(
        checker.check_path("read", "/home/user/project/src/main.rs"),
        CheckResult::Allowed
    ));
    // Bash has no rules, default action is Allow
    assert!(matches!(
        checker.check("bash", "some_command"),
        CheckResult::Allowed
    ));
}

// --- Standard mode with external_directory rules ---

#[test]
fn standard_external_dir_allow_rule_overrides_default_ask() {
    let config = PermissionConfig {
        external_directory: Some([("/tmp/work/**".to_string(), Action::Allow)].into()),
        ..PermissionConfig::default()
    };
    let configs = configs_from(config);
    let mut checker = PermissionChecker::new(
        &configs,
        SecurityMode::Standard,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    );
    // External path but covered by external_directory allow rule
    let result = checker.check_path("write", "/tmp/work/notes.txt");
    assert!(
        matches!(result, CheckResult::Allowed),
        "expected Allowed for external path covered by allow rule, got {:?}",
        result,
    );
}

#[test]
fn standard_external_dir_deny_rule_overrides_default_ask() {
    let config = PermissionConfig {
        external_directory: Some([("/etc/**".to_string(), Action::Deny)].into()),
        ..PermissionConfig::default()
    };
    let configs = configs_from(config);
    let mut checker = PermissionChecker::new(
        &configs,
        SecurityMode::Standard,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    );
    let result = checker.check_path("write", "/etc/config.conf");
    assert!(
        matches!(result, CheckResult::Denied(_)),
        "expected Denied for external path with deny rule, got {:?}",
        result,
    );
}

// --- ReadOnly with explicit config rules ---

#[test]
fn readonly_respects_explicit_config_allow() {
    let config = PermissionConfig {
        write: Some(ToolPerm::Granular(
            [("**".to_string(), Action::Allow)].into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::ReadOnly,
        Some(std::path::PathBuf::from("/home/user/project")),
        Some(vec!["readonly".to_string()]),
    );
    // ReadOnly in permission_modes, config rule says write:allow -> Allowed
    assert!(matches!(
        checker.check("write", "/etc/passwd"),
        CheckResult::Allowed
    ));
}

// --- Guarded path operations ---

#[test]
fn guarded_asks_for_external_path_write() {
    let mut checker = make_checker(SecurityMode::Guarded);
    let result = checker.check_path("write", "/etc/config.conf");
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask for external write in Guarded, got {:?}",
        result,
    );
}

#[test]
fn guarded_allows_internal_path_read() {
    let mut checker = make_checker(SecurityMode::Guarded);
    assert!(matches!(
        checker.check_path("read", "/home/user/project/src/main.rs"),
        CheckResult::Allowed,
    ));
}

// --- Doom loop across different modes ---

#[test]
fn doom_loop_triggers_in_guarded() {
    let mut checker = make_checker(SecurityMode::Guarded);
    // "echo test" matches echo ** allow rule, so action is Allow.
    // Doom loop should coach instead of asking.
    checker.check("bash", "echo test");
    checker.check("bash", "echo test");
    let result = checker.check("bash", "echo test");
    assert!(
        matches!(result, CheckResult::AllowedWithCoaching(_)),
        "expected AllowedWithCoaching from doom loop in Guarded, got {:?}",
        result,
    );
}

#[test]
fn doom_loop_still_asks_for_read_tool_in_restrictive() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    // In Restrictive, first 2 calls ask (or ask through mode default)
    checker.check("read", "some_file");
    checker.check("read", "some_file");
    let result = checker.check("read", "some_file");
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask from doom loop in Restrictive, got {:?}",
        result,
    );
}

#[test]
fn doom_loop_path_coaches_in_standard_auto_allow() {
    let mut checker = make_checker(SecurityMode::Standard);
    // In Standard, path tools within CWD are auto-allowed.
    // Doom loop should coach instead of asking.
    assert!(matches!(
        checker.check_path("edit", "src/main.rs"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check_path("edit", "src/main.rs"),
        CheckResult::Allowed,
    ));
    let result = checker.check_path("edit", "src/main.rs");
    assert!(
        matches!(result, CheckResult::AllowedWithCoaching(_)),
        "expected AllowedWithCoaching for path doom loop in Standard, got {:?}",
        result,
    );
}

// --- Path edge cases ---

#[test]
fn check_path_with_relative_is_not_external_in_standard() {
    let mut checker = make_checker(SecurityMode::Standard);
    assert!(matches!(
        checker.check_path("read", "src/main.rs"),
        CheckResult::Allowed,
    ));
}

#[test]
fn check_path_with_tilde_expansion_internal() {
    // ~ expands to home, which is outside the CWD /home/user/project
    // So this should Ask in Standard mode
    let mut checker = make_checker(SecurityMode::Standard);
    let result = checker.check_path("write", "~/outside.txt");
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask for ~ path outside CWD in Standard, got {:?}",
        result,
    );
}

// --- YOLO mode edge cases ---

#[test]
fn yolo_destructive_patterns_are_denied() {
    let mut checker = make_checker(SecurityMode::Yolo);
    // rm -rf /** deny rule now actually denies in YOLO
    assert!(matches!(
        checker.check("bash", "rm -rf /sensitive/data"),
        CheckResult::Denied(_)
    ));
}

#[test]
fn yolo_deny_rules_for_mcp_are_denied() {
    let config = PermissionConfig {
        mcp_tool: Some(ToolPerm::Granular(
            [("mcp_tool:fs:delete_*".to_string(), Action::Deny)].into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Yolo,
        None,
        default_modes(),
    );
    // Deny rules now actually deny in YOLO
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:fs:delete_file"),
        CheckResult::Denied(_)
    ));
    // Non-destructive MCP still Allowed
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:fs:read_file"),
        CheckResult::Allowed
    ));
}

// --- permission=None equivalent (dangerously-skip-permissions) ---
// Test that when permission is None, check_perm returns Ok(None)
// This is tested via check_perm in tools/mod.rs, but we verify the checker
// itself would be bypassed by testing with PermissionChecker not created.

#[tokio::test]
async fn check_perm_skipped_when_permission_is_none() {
    // When permission is None, tools/mod.rs check_perm returns Ok(None) immediately.
    // This test verifies the logic path: None means no checks run.
    let perm: Option<std::sync::Arc<std::sync::Mutex<PermissionChecker>>> = None;
    let ask_tx: Option<crate::permission::ask::AskSender> = None;
    let result = crate::agent::tools::check_perm(&perm, &ask_tx, "bash", "rm -rf /").await;
    assert!(result.is_ok(), "expected Ok when permission is None");
    assert!(
        result.unwrap().is_none(),
        "expected None coaching when permission is None"
    );
}

#[tokio::test]
async fn check_perm_path_skipped_when_permission_is_none() {
    let perm: Option<std::sync::Arc<std::sync::Mutex<PermissionChecker>>> = None;
    let ask_tx: Option<crate::permission::ask::AskSender> = None;
    let result = crate::agent::tools::check_perm_path(&perm, &ask_tx, "write", "/etc/passwd").await;
    assert!(result.is_ok(), "expected Ok when permission is None");
    assert!(
        result.unwrap().is_none(),
        "expected None coaching when permission is None"
    );
}

// --- MCP deny in Guarded mode ---

#[test]
fn guarded_mcp_tool_asks_when_no_rule() {
    let mut checker = make_checker(SecurityMode::Guarded);
    // MCP tool is not a read tool -> Ask
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:fs:write_file"),
        CheckResult::Ask,
    ));
}

// --- Read-equivalent MCP tools allowed in ReadOnly / PlanWrite ---

#[test]
fn readonly_allows_exa_mcp_tools() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Exa Web Search:websearch"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Exa Web Search:webfetch"),
        CheckResult::Allowed,
    ));
}

#[test]
fn planwrite_allows_exa_mcp_tools() {
    let mut checker = make_checker(SecurityMode::PlanWrite);
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Exa Web Search:websearch"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Exa Web Search:webfetch"),
        CheckResult::Allowed,
    ));
}

#[test]
fn readonly_allows_context7_mcp_tools() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Context7:get_context"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Context7:search_docs"),
        CheckResult::Allowed,
    ));
}

#[test]
fn planwrite_allows_context7_mcp_tools() {
    let mut checker = make_checker(SecurityMode::PlanWrite);
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Context7:get_context"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Context7:search_docs"),
        CheckResult::Allowed,
    ));
}

#[test]
fn readonly_allows_grepapp_mcp_tools() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Grep.app:search_code"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Grep.app:search_repos"),
        CheckResult::Allowed,
    ));
}

#[test]
fn planwrite_allows_grepapp_mcp_tools() {
    let mut checker = make_checker(SecurityMode::PlanWrite);
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Grep.app:search_code"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Grep.app:search_repos"),
        CheckResult::Allowed,
    ));
}

#[test]
fn readonly_case_insensitive_mcp_prefix_match() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:exa web search:websearch"),
        CheckResult::Allowed,
    ));
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:CONTEXT7:some_tool"),
        CheckResult::Allowed,
    ));
}

#[test]
fn readonly_denies_non_read_equivalent_mcp_tools() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:filesystem:write_file"),
        CheckResult::Denied(_),
    ));
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:other_server:some_tool"),
        CheckResult::Denied(_),
    ));
}

#[test]
fn readonly_denies_unrelated_prefix() {
    let mut checker = make_checker(SecurityMode::ReadOnly);
    // Similar-looking prefixes that don"t match read-equivalent servers
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:exa:websearch"),
        CheckResult::Denied(_),
    ));
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:context7extra:some_tool"),
        CheckResult::Denied(_),
    ));
}

#[test]
fn standard_mode_still_allows_exa_mcp_via_default() {
    let mut checker = make_checker(SecurityMode::Standard);
    assert!(matches!(
        checker.check("mcp_tool", "mcp_tool:Exa Web Search:websearch"),
        CheckResult::Allowed,
    ));
}

// --- Standard mode respects config allow for specific paths ---

#[test]
fn standard_respects_config_allow_over_cwd_auto_allow() {
    // CWD auto-allow already returns Allow, but we test that an explicit
    // Allow rule for an external path overrides the Ask default
    let config = PermissionConfig {
        bash: Some(ToolPerm::Granular(
            [("pip install **".to_string(), Action::Allow)].into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &configs_from(config),
        SecurityMode::Standard,
        Some(std::path::PathBuf::from("/home/user/project")),
        default_modes(),
    );
    assert!(matches!(
        checker.check("bash", "pip install requests"),
        CheckResult::Allowed,
    ));
}
