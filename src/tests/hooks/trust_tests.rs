use std::io::Write;

use crate::extras::hooks::settings::HookHandler;
use crate::extras::hooks::trust;

fn handler(command: &str) -> HookHandler {
    HookHandler {
        kind: "command".to_string(),
        command: Some(command.to_string()),
        args: None,
        timeout: Some(5),
        is_async: false,
        condition: None,
        once: false,
    }
}

fn unique_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "zerostack-hooks-trust-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn write_settings(path: &std::path::Path, json: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(json.as_bytes()).unwrap();
}

fn missing_path(name: &str) -> std::path::PathBuf {
    unique_path(&format!("missing-{name}"))
}

fn project_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/repo/test-project")
}

#[test]
fn hash_changes_when_command_changes() {
    let root = std::path::Path::new("/repo/a");
    let h1 = handler("echo one");
    let h2 = handler("echo two");
    let hash1 = trust::hash_hook_binding(root, "PreToolUse", Some("Bash"), &h1);
    let hash2 = trust::hash_hook_binding(root, "PreToolUse", Some("Bash"), &h2);
    assert_ne!(hash1, hash2);
}

#[test]
fn hash_stable_for_identical_binding() {
    let root = std::path::Path::new("/repo/a");
    let h1 = handler("echo one");
    let h2 = handler("echo one");
    let hash1 = trust::hash_hook_binding(root, "PreToolUse", Some("Bash"), &h1);
    let hash2 = trust::hash_hook_binding(root, "PreToolUse", Some("Bash"), &h2);
    assert_eq!(hash1, hash2);
}

#[test]
fn hash_changes_when_matcher_changes() {
    let root = std::path::Path::new("/repo/a");
    let h = handler("echo one");
    let hash1 = trust::hash_hook_binding(root, "PreToolUse", Some("Bash"), &h);
    let hash2 = trust::hash_hook_binding(root, "PreToolUse", Some("*"), &h);
    assert_ne!(hash1, hash2);
}

#[test]
fn hash_changes_when_project_root_changes() {
    let h = handler("./guard.sh");
    let hash1 = trust::hash_hook_binding(
        std::path::Path::new("/repo/project-a"),
        "PreToolUse",
        Some("Bash"),
        &h,
    );
    let hash2 = trust::hash_hook_binding(
        std::path::Path::new("/repo/project-b"),
        "PreToolUse",
        Some("Bash"),
        &h,
    );
    assert_ne!(
        hash1, hash2,
        "trusting a binding in one project must not trust the identical binding in another"
    );
}

#[test]
fn trust_store_round_trips_and_is_visible_to_a_fresh_load() {
    let path = unique_path("store");
    let _ = std::fs::remove_file(&path);

    assert!(!trust::load_trust_store(&path).contains("abc123"));
    let mut store = trust::load_trust_store(&path);
    store.insert("abc123".to_string());
    trust::save_trust_store(&path, &store);

    // Simulates a child process independently loading the same trust file.
    assert!(trust::load_trust_store(&path).contains("abc123"));
    assert!(!trust::load_trust_store(&path).contains("does-not-exist"));
}

#[test]
fn global_only_settings_load_without_consulting_confirmation() {
    let global = unique_path("global");
    write_settings(
        &global,
        r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
    );
    let project = missing_path("project");
    let managed = missing_path("managed");
    let trust_path = unique_path("trust");

    let dispatcher = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &project_root(),
        false,
        false,
        &trust_path,
        &|_| panic!("global-sourced hooks must never require confirmation"),
    );

    assert!(!dispatcher.is_empty());
}

#[test]
fn missing_files_are_not_an_error() {
    let global = missing_path("g");
    let project = missing_path("p");
    let managed = missing_path("m");
    let trust_path = unique_path("trust");

    let dispatcher = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &project_root(),
        false,
        false,
        &trust_path,
        &|_| false,
    );
    assert!(dispatcher.is_empty());
}

#[test]
fn disable_all_hooks_in_project_settings_disables_non_managed_but_not_managed() {
    let global = unique_path("global2");
    write_settings(
        &global,
        r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
    );
    let project = unique_path("project2");
    write_settings(&project, r#"{"disableAllHooks": true}"#);
    let managed = unique_path("managed2");
    write_settings(
        &managed,
        r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
    );
    let trust_path = unique_path("trust2");

    let dispatcher = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &project_root(),
        false,
        false,
        &trust_path,
        &|_| panic!("no project hooks to confirm here"),
    );

    // Managed hooks alone keep the dispatcher non-empty even though
    // disableAllHooks would otherwise suppress the global hook.
    assert!(!dispatcher.is_empty());
    assert_eq!(dispatcher.handlers_for("PreToolUse", "bash").len(), 1);
}

#[test]
fn headless_unconfirmed_project_hook_is_skipped_without_confirmation() {
    let global = missing_path("g3");
    let project = unique_path("project3");
    write_settings(
        &project,
        r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo untrusted"}]}]}}"#,
    );
    let managed = missing_path("m3");
    let trust_path = unique_path("trust3");
    let _ = std::fs::remove_file(&trust_path);

    let dispatcher = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &project_root(),
        false,
        true, // headless
        &trust_path,
        &|_| panic!("headless must never prompt for confirmation"),
    );

    assert!(dispatcher.is_empty());
}

#[test]
fn interactive_confirmation_accepted_persists_trust() {
    let global = missing_path("g4");
    let project = unique_path("project4");
    write_settings(
        &project,
        r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo trust-me"}]}]}}"#,
    );
    let managed = missing_path("m4");
    let trust_path = unique_path("trust4");
    let _ = std::fs::remove_file(&trust_path);

    let dispatcher = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &project_root(),
        false,
        false,
        &trust_path,
        &|_| true,
    );
    assert!(!dispatcher.is_empty());

    // Re-running against the same trust store should not need confirmation
    // again (a changed/declined confirm callback would panic/return false).
    let dispatcher2 = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &project_root(),
        false,
        false,
        &trust_path,
        &|_| panic!("should already be trusted from the previous run"),
    );
    assert!(!dispatcher2.is_empty());
}

#[test]
fn trust_from_one_project_root_does_not_carry_over_to_another() {
    let global = missing_path("g4b");
    let project = unique_path("project4b");
    write_settings(
        &project,
        r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo trust-me"}]}]}}"#,
    );
    let managed = missing_path("m4b");
    let trust_path = unique_path("trust4b");
    let _ = std::fs::remove_file(&trust_path);
    let root_a = std::path::PathBuf::from("/repo/project-a");
    let root_b = std::path::PathBuf::from("/repo/project-b");

    // Trust the binding under project root A.
    let dispatcher = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &root_a,
        false,
        false,
        &trust_path,
        &|_| true,
    );
    assert!(!dispatcher.is_empty());

    // The identical binding (same settings file, same command) under a
    // different project root must still require confirmation: declining it
    // must exclude the hook, proving trust did not carry over.
    let dispatcher2 = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &root_b,
        false,
        false,
        &trust_path,
        &|_| false,
    );
    assert!(dispatcher2.is_empty());
}

#[test]
fn interactive_confirmation_declined_excludes_the_hook() {
    let global = missing_path("g5");
    let project = unique_path("project5");
    write_settings(
        &project,
        r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo nope"}]}]}}"#,
    );
    let managed = missing_path("m5");
    let trust_path = unique_path("trust5");
    let _ = std::fs::remove_file(&trust_path);

    let dispatcher = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &project_root(),
        false,
        false,
        &trust_path,
        &|_| false,
    );
    assert!(dispatcher.is_empty());
}

#[test]
fn no_hooks_flag_excludes_non_managed_but_not_managed() {
    let global = unique_path("global6");
    write_settings(
        &global,
        r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
    );
    let project = missing_path("p6");
    let managed = unique_path("managed6");
    write_settings(
        &managed,
        r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
    );
    let trust_path = unique_path("trust6");

    let dispatcher = trust::build_dispatcher_from_paths(
        &global,
        &project,
        &managed,
        &project_root(),
        true, // --no-hooks
        false,
        &trust_path,
        &|_| panic!("no project hooks here"),
    );

    assert!(!dispatcher.is_empty());
    assert_eq!(dispatcher.handlers_for("PreToolUse", "bash").len(), 1);
}
