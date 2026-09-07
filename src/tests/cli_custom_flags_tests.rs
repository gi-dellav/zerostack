//! `--custom-mcp*` / `--prompts-dir` CLI flag resolution.

#![allow(unsafe_code)]

#[cfg(feature = "mcp")]
use std::collections::HashMap;
use std::path::PathBuf;

use clap::Parser;

use crate::cli::Cli;

fn cli_with(args: &[&str]) -> Cli {
    Cli::try_parse_from(std::iter::once("zerostack").chain(args.iter().copied())).unwrap()
}

/// `ZS_PROMPTS_DIR` is process-global: tests that read it must hold this
/// lock and leave the var removed, or parallel tests observe each other's
/// values.
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

fn acquire_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(feature = "mcp")]
fn mcp_map(
    cfg: &crate::config::Config,
) -> &HashMap<String, crate::extras::mcp::config::McpServerConfig> {
    cfg.mcp_servers.as_ref().unwrap()
}

#[cfg(feature = "mcp")]
#[test]
fn custom_mcp_stdio_merges_over_config() {
    let cli = cli_with(&["--custom-mcp", "fs=npx -y server-fs ."]);
    let mut cfg = crate::config::Config::default();
    cli.merge_cli_mcp(&mut cfg).unwrap();
    let servers = mcp_map(&cfg);
    assert_eq!(servers.len(), 1);
    match &servers["fs"] {
        crate::extras::mcp::config::McpServerConfig::Command { command, args, .. } => {
            assert_eq!(command, "npx");
            assert_eq!(args, &vec!["-y", "server-fs", "."]);
        }
        other => panic!("expected stdio Command, got {other:?}"),
    }
}

#[cfg(feature = "mcp")]
#[test]
fn custom_mcp_http_merges_over_config() {
    let cli = cli_with(&["--custom-mcp-http", "exa=https://mcp.exa.ai/mcp"]);
    let mut cfg = crate::config::Config::default();
    cli.merge_cli_mcp(&mut cfg).unwrap();
    let servers = mcp_map(&cfg);
    assert!(matches!(
        &servers["exa"],
        crate::extras::mcp::config::McpServerConfig::Url { .. }
    ));
}

#[cfg(feature = "mcp")]
#[test]
fn custom_mcp_cli_overrides_same_named_config_entry() {
    let cli = cli_with(&["--custom-mcp-http", "srv=https://cli.example/mcp"]);
    let mut cfg: crate::config::Config = toml::from_str(
        r#"[mcp_servers.srv]
command = "old-cmd"
"#,
    )
    .unwrap();
    cli.merge_cli_mcp(&mut cfg).unwrap();
    match &mcp_map(&cfg)["srv"] {
        crate::extras::mcp::config::McpServerConfig::Url { url, .. } => {
            assert_eq!(url, "https://cli.example/mcp");
        }
        other => panic!("expected CLI URL to win, got {other:?}"),
    }
}

#[cfg(feature = "mcp")]
#[test]
fn custom_mcp_later_flag_wins_for_same_name() {
    let args = vec![
        "--custom-mcp",
        "srv=first-cmd",
        "--custom-mcp",
        "srv=second-cmd --flag",
    ];
    let cli = cli_with(&args);
    let mut cfg = crate::config::Config::default();
    cli.merge_cli_mcp(&mut cfg).unwrap();
    match &mcp_map(&cfg)["srv"] {
        crate::extras::mcp::config::McpServerConfig::Command { command, args, .. } => {
            assert_eq!(command, "second-cmd");
            assert_eq!(args, &vec!["--flag"]);
        }
        other => panic!("expected second Command, got {other:?}"),
    }
}

#[cfg(feature = "mcp")]
#[test]
fn custom_mcp_invalid_entry_is_an_error() {
    let cli = cli_with(&["--custom-mcp", "no-equals-here"]);
    let mut cfg = crate::config::Config::default();
    assert!(cli.merge_cli_mcp(&mut cfg).is_err());
    assert!(cfg.mcp_servers.is_none());
}

#[cfg(feature = "mcp")]
#[test]
fn custom_mcp_no_flags_leaves_config_untouched() {
    let cli = cli_with(&[]);
    let mut cfg = crate::config::Config::default();
    cli.merge_cli_mcp(&mut cfg).unwrap();
    assert!(cfg.mcp_servers.is_none());
}

#[test]
fn prompts_dir_flag_is_repeatable_and_ordered() {
    let _lock = acquire_env();
    unsafe {
        std::env::remove_var("ZS_PROMPTS_DIR");
    }
    let cli = cli_with(&["--prompts-dir", "a", "--prompts-dir", "b"]);
    assert_eq!(
        cli.resolve_prompts_dirs(),
        vec![PathBuf::from("a"), PathBuf::from("b")]
    );
}

#[test]
fn prompts_dir_env_appends_after_flags() {
    let _lock = acquire_env();
    unsafe {
        std::env::set_var("ZS_PROMPTS_DIR", "env-a:env-b");
    }
    let cli = cli_with(&["--prompts-dir", "flag"]);
    let dirs = cli.resolve_prompts_dirs();
    unsafe {
        std::env::remove_var("ZS_PROMPTS_DIR");
    }
    assert_eq!(
        dirs,
        vec![
            PathBuf::from("flag"),
            PathBuf::from("env-a"),
            PathBuf::from("env-b"),
        ]
    );
}

#[test]
fn prompts_dir_empty_by_default() {
    let _lock = acquire_env();
    unsafe {
        std::env::remove_var("ZS_PROMPTS_DIR");
    }
    let cli = cli_with(&[]);
    assert!(cli.resolve_prompts_dirs().is_empty());
}
