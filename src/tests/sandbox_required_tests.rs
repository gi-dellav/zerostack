use std::sync::{Arc, Mutex};

use crate::agent::tools::BashArgs;
use crate::agent::tools::bash::BashTool;
use crate::cli::Cli;
use crate::config::Config;
use crate::permission::checker::{PermCheck, PermissionChecker};
use crate::permission::{PermissionConfigs, SecurityMode};
use crate::sandbox::Sandbox;
use rig::tool::Tool;

fn missing_backend(backend: &str) -> Sandbox {
    Sandbox::new(true, backend)
        .with_required(true)
        .with_backend_available(false)
}

fn scratch_cache_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("zerostack-sandbox-{}-{}", name, std::process::id()))
}

#[test]
fn test_required_backend_missing_refuses_wrap_command() {
    let sandbox = missing_backend("bwrap");
    let err = sandbox.wrap_command("echo hello").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("bwrap"),
        "message should name the backend: {msg}"
    );
    assert!(
        msg.contains("sandbox-required"),
        "message should name the config key: {msg}"
    );
}

#[test]
fn test_required_zerobox_missing_names_zerobox() {
    let sandbox = missing_backend("zerobox");
    let err = sandbox.wrap_command("echo hello").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("zerobox"),
        "message should name the backend: {msg}"
    );
    assert!(
        msg.contains("sandbox-required"),
        "message should name the config key: {msg}"
    );
}

#[test]
fn test_refusal_names_the_configured_backend_verbatim() {
    // A misspelled backend routes to the bwrap probe, but the refusal has to
    // name what the user configured, not what zerostack fell back to.
    let sandbox = missing_backend("bubblewrap");
    let msg = sandbox.wrap_command("echo hello").unwrap_err().to_string();
    assert!(
        msg.contains("'bubblewrap'"),
        "message should name the configured backend: {msg}"
    );
    assert!(
        !msg.contains("bwrap"),
        "message should not invent a backend: {msg}"
    );
}

#[tokio::test]
async fn test_required_backend_missing_refuses_output_command() {
    let sandbox = missing_backend("bwrap");
    let err = sandbox.output_command("echo hello").await.unwrap_err();
    assert!(err.to_string().contains("sandbox-required"));
}

#[tokio::test]
async fn test_not_required_backend_missing_runs_unsandboxed() {
    let sandbox = Sandbox::new(true, "bwrap").with_backend_available(false);
    let output = sandbox.output_command("echo hello").await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "hello");
}

#[test]
fn test_required_backend_present_wraps_command() {
    let cache_dir = scratch_cache_dir("present");
    let sandbox = Sandbox::new(true, "bwrap")
        .with_required(true)
        .with_backend_available(true)
        .with_cache_dir(cache_dir.clone());
    let cmd = sandbox.wrap_command("echo hello").unwrap();
    assert_eq!(cmd.as_std().get_program(), "bwrap");
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_backend_installed_after_cached_probe_lifts_the_refusal() {
    // The PATH probe is cached for the process lifetime, so the refusal path
    // re-probes: installing the backend mid-session must start working.
    let cache_dir = scratch_cache_dir("installed-later");
    let sandbox = Sandbox::new(true, "bwrap")
        .with_required(true)
        .with_backend_available(false)
        .with_backend_installed_now(true)
        .with_cache_dir(cache_dir.clone());
    assert!(sandbox.refusal_reason().is_none());
    let cmd = sandbox.wrap_command("echo hello").unwrap();
    assert_eq!(cmd.as_std().get_program(), "bwrap");
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn test_required_zerobox_present_wraps_command() {
    let sandbox = Sandbox::new(true, "zerobox")
        .with_required(true)
        .with_backend_available(true);
    let cmd = sandbox.wrap_command("echo hello").unwrap();
    assert_eq!(cmd.as_std().get_program(), "zerobox");
}

#[test]
fn test_disabled_sandbox_ignores_required() {
    let sandbox = Sandbox::new(false, "bwrap")
        .with_required(true)
        .with_backend_available(false);
    assert!(sandbox.refusal_reason().is_none());
    let cmd = sandbox.wrap_command("echo hello").unwrap();
    assert_eq!(cmd.as_std().get_program(), "bash");
}

fn bash_tool(sandbox: Sandbox, permission: Option<PermCheck>) -> BashTool {
    BashTool::new(
        permission,
        None,
        sandbox,
        None,
        #[cfg(feature = "rtk")]
        None,
    )
}

fn echo_args() -> BashArgs {
    BashArgs {
        command: "echo hello".to_string(),
        timeout: None,
    }
}

#[tokio::test]
async fn test_bash_tool_surfaces_refusal_to_the_model() {
    let err = bash_tool(missing_backend("bwrap"), None)
        .call(echo_args())
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("bwrap"),
        "tool error should name the backend: {msg}"
    );
    assert!(
        msg.contains("sandbox-required"),
        "tool error should name the config key: {msg}"
    );
}

#[tokio::test]
async fn test_bash_tool_refuses_before_checking_permissions() {
    // Read-only mode denies bash outright. The sandbox refusal has to win, so
    // that users are never asked to approve a command that cannot run.
    let permission: PermCheck = Arc::new(Mutex::new(PermissionChecker::new(
        &PermissionConfigs::default(),
        SecurityMode::ReadOnly,
        None,
        None,
    )));
    let denied = bash_tool(Sandbox::new(false, "bwrap"), Some(permission.clone()))
        .call(echo_args())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        denied.contains("Permission denied"),
        "the checker must deny bash for this test to mean anything: {denied}"
    );

    let msg = bash_tool(missing_backend("bwrap"), Some(permission))
        .call(echo_args())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        msg.contains("sandbox-required"),
        "sandbox refusal should come before the permission check: {msg}"
    );
}

#[test]
fn test_resolve_sandbox_required_defaults_to_false() {
    let cli = Cli::default();
    let cfg = Config::default();
    assert!(!cli.resolve_sandbox_required(&cfg));
    assert!(!cli.resolve_sandbox(&cfg));
    assert!(!cli.sandbox_setting_conflict(&cfg));
}

#[test]
fn test_resolve_sandbox_required_from_config() {
    let cli = Cli::default();
    let cfg = Config {
        sandbox_required: Some(true),
        ..Default::default()
    };
    assert!(cli.resolve_sandbox_required(&cfg));
    assert!(cli.resolve_sandbox(&cfg));
    assert!(!cli.sandbox_setting_conflict(&cfg));
}

#[test]
fn test_resolve_sandbox_required_cli_overrides_config() {
    let cli = Cli {
        sandbox_required: true,
        ..Default::default()
    };
    let cfg = Config {
        sandbox_required: Some(false),
        ..Default::default()
    };
    assert!(cli.resolve_sandbox_required(&cfg));
}

#[test]
fn test_required_wins_over_disabled_sandbox() {
    let cli = Cli::default();
    let cfg = Config {
        sandbox: Some(false),
        sandbox_required: Some(true),
        ..Default::default()
    };
    assert!(cli.resolve_sandbox(&cfg));
    assert!(cli.resolve_sandbox_required(&cfg));
    assert!(cli.sandbox_setting_conflict(&cfg));
}
