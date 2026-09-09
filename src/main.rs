#![deny(unsafe_code)]

/// Agent lifecycle: rig construction, streaming runner, prompts, and tools.
mod agent;
/// API key resolution across providers, env vars, and config.
mod auth;
/// Clap CLI argument definitions and per-flag resolvers.
mod cli;
/// TOML/YAML/JSON configuration loading and typed settings.
mod config;
/// Embedded prompts, themes, and workspace AGENTS.md loading.
mod context;
/// Embedded user docs and first-run global file setup.
mod docs;
/// Headless programmatic execution (Engine + run_string) without a TUI.
pub mod engine;
/// Shared AgentEvent/UserEvent channel types for agent and TUI.
mod event;
/// Optional feature-gated extensions (MCP, loop, subagents, hooks, ...).
mod extras;
/// Filesystem helpers (atomic writes, path utilities).
mod fs;
/// Tracing subscriber setup, log files, and panic hook.
mod logging;
/// Embedded static model catalog with pricing and context windows.
mod models_catalog;
/// Tool permission checking, ask flow, and pattern matching.
mod permission;
/// Token pricing and per-turn cost estimation.
mod pricing;
/// One-shot --print output, config dump, and session listing.
mod print;
/// LLM provider clients, model routing, and agent factory.
mod provider;
/// Retryable-error classification and backoff for model streams.
mod retry;
/// bwrap/zerobox sandbox wrapping for shell commands.
mod sandbox;
/// Conversation session state, JSON storage, and chat history.
mod session;
/// Interactive --setup wizard for providers and models.
mod setup;
/// Startup orchestration: config, session, client, and mode dispatch.
mod startup;
/// Interactive crossterm TUI: event loop, renderer, input, slash commands.
mod ui;

#[cfg(test)]
/// In-crate integration and unit tests (test builds only).
mod tests;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::Context;
use clap::Parser;

#[cfg_attr(
    feature = "multithread",
    tokio::main(flavor = "multi_thread", worker_threads = 4)
)]
#[cfg_attr(not(feature = "multithread"), tokio::main(flavor = "current_thread"))]
async fn main() -> anyhow::Result<()> {
    run().await.context(
        "This error might derive from an incomplete configuration: run `zerostack --setup` to configure your providers and models interactively, or `zerostack --tutor` to see the getting started guide",
    )
}

async fn run() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    logging::install_panic_hook();
    logging::init(&cli);

    let (mut cfg, is_first_startup) = config::load();

    // CLI MCP flags override config; parse errors exit before anything runs.
    #[cfg(feature = "mcp")]
    if let Err(e) = cli.merge_cli_mcp(&mut cfg) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }

    if cli.print_config {
        print::print_config(&cli, &cfg);
        return Ok(());
    }

    if cli.setup {
        match setup::run(&mut cfg)? {
            setup::SetupOutcome::Quit => return Ok(()),
            setup::SetupOutcome::LaunchAutoconfigure => {
                // autoconfigure was already applied in setup; fall through to launch
            }
            setup::SetupOutcome::Launch => {
                // fall through to launch
            }
        }
    }

    if cli.tutor {
        return docs::show_get_started();
    }

    if cli.resume && cli.session.is_none() {
        print::print_sessions();
        return Ok(());
    }

    let version_changed = docs::ensure_global()?;
    let is_interactive = !cli.print;
    #[cfg(feature = "acp")]
    let is_interactive = is_interactive && !cli.acp_enabled;
    #[cfg(feature = "loop")]
    let is_interactive = is_interactive && !cli.loop_mode;

    // ── Hooks: load settings.json config, apply trust, install dispatcher ──
    // Done this early (before provider/API-key resolution) so `--hooks-test`
    // is a pure config/dispatch dry run that needs no API key and makes no
    // model call.
    #[cfg(feature = "hooks")]
    {
        crate::extras::hooks::init_dispatcher(
            crate::extras::hooks::trust::load_dispatcher_async(cli.no_hooks, !is_interactive).await,
        );

        if let Some(tool_name) = &cli.hooks_test {
            let tool_input: serde_json::Value = cli
                .hooks_test_input
                .as_deref()
                .map(|s| serde_json::from_str(s).unwrap_or(serde_json::Value::Null))
                .unwrap_or_else(|| serde_json::json!({}));
            println!(
                "{}",
                crate::extras::hooks::hooks_test_dry_run(tool_name, tool_input).await
            );
            return Ok(());
        }
    }

    let phase_start = std::time::Instant::now();
    let mut startup =
        startup::Startup::init(cli, cfg, is_first_startup, version_changed, is_interactive).await?;
    tracing::debug!("startup: init took {:?}", phase_start.elapsed());

    // ACP mode: serve and exit before feature init
    #[cfg(feature = "acp")]
    if startup.cli.acp_enabled {
        return extras::acp::serve(startup.cli, startup.cfg, startup.context).await;
    }

    let phase_start = std::time::Instant::now();
    startup.init_features().await?;
    tracing::debug!("startup: init_features took {:?}", phase_start.elapsed());
    let phase_start = std::time::Instant::now();
    startup.resolve_prompts().await?;
    tracing::debug!("startup: resolve_prompts took {:?}", phase_start.elapsed());
    startup.dispatch().await
}
