use crate::permission::checker::{CheckResult, PermissionChecker};
use crate::permission::{Action, PermissionConfig, SecurityMode, ToolPerm};

fn make_checker(mode: SecurityMode) -> PermissionChecker {
    PermissionChecker::new(
        &PermissionConfig::default(),
        mode,
        Some(std::path::PathBuf::from("/home/user/project")),
    )
}

// --- SecurityMode behavior ---

#[test]
fn yolo_allows_everything() {
    let mut checker = make_checker(SecurityMode::Yolo);
    assert_eq!(checker.check("bash", "rm -rf /"), CheckResult::Allowed);
    assert_eq!(checker.check("write", "/etc/passwd"), CheckResult::Allowed);
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

#[test]
fn accept_auto_allows_inside_working_dir() {
    let config = PermissionConfig {
        write: Some(ToolPerm::Simple(Action::Ask)),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(
        &config,
        SecurityMode::Accept,
        Some(std::path::PathBuf::from("/home/user/project")),
    );
    let result = checker.check_path("write", "/home/user/project/src/main.rs");
    assert!(matches!(result, CheckResult::Allowed));
}

#[test]
fn accept_asks_for_external_path() {
    let mut checker = make_checker(SecurityMode::Accept);
    let external_path = if cfg!(windows) {
        "D:\\outside\\file.txt"
    } else {
        "/etc/config.conf"
    };
    let result = checker.check_path("write", external_path);
    assert!(
        matches!(result, CheckResult::Ask),
        "expected Ask, got {:?} for path: {}",
        result,
        external_path,
    );
}

// --- Deny rules ---

#[test]
fn deny_rule_blocks_regardless_of_mode() {
    let mut checker = make_checker(SecurityMode::Standard);
    let result = checker.check("bash", "rm -rf /home/user/project");
    assert!(matches!(result, CheckResult::Denied(_)));
}

#[test]
fn deny_rule_not_blocked_by_yolo() {
    let mut checker = make_checker(SecurityMode::Yolo);
    let result = checker.check("bash", "rm -rf /home/user/project");
    assert!(matches!(result, CheckResult::Allowed));
}

// --- Doom loop detection ---

#[test]
fn doom_loop_triggers_after_three_repeated_calls() {
    let mut checker = make_checker(SecurityMode::Standard);
    checker.check("bash", "ls");
    checker.check("bash", "ls");
    let result = checker.check("bash", "ls");
    assert!(matches!(result, CheckResult::Ask));
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

// --- Session allowlist ---

#[test]
fn session_allowlist_bypasses_rules() {
    let mut checker = make_checker(SecurityMode::Restrictive);
    checker.add_session_allowlist("bash".into(), "cargo test **");
    let result = checker.check("bash", "cargo test --all");
    assert!(matches!(result, CheckResult::Allowed));
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
    assert!(matches!(result, CheckResult::Ask), "expected Ask, got {:?}", result,);
}

#[test]
fn relative_path_is_not_external() {
    let mut checker = make_checker(SecurityMode::Accept);
    let result = checker.check_path("read", "src/lib.rs");
    assert!(matches!(result, CheckResult::Allowed));
}

// --- Config-driven rules ---

#[test]
fn explicit_granular_rules_take_effect() {
    let config = PermissionConfig {
        read: Some(ToolPerm::Granular(
            [("*.md".to_string(), Action::Allow), ("*.rs".to_string(), Action::Ask)]
                .into(),
        )),
        ..PermissionConfig::default()
    };
    let mut checker = PermissionChecker::new(&config, SecurityMode::Standard, None);
    assert_eq!(checker.check("read", "README.md"), CheckResult::Allowed);
    assert_eq!(checker.check("read", "main.rs"), CheckResult::Ask);
}
