#![deny(unsafe_code)]

mod agent;
mod auth;
mod cli;
mod config;
mod context;
mod docs;
mod event;
mod extras;
mod fs;
mod logging;
mod models_catalog;
mod permission;
mod pricing;
mod print;
mod provider;
mod retry;
mod sandbox;
mod session;
mod setup;
mod startup;
mod ui;

#[cfg(test)]
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
        crate::extras::hooks::init_dispatcher(crate::extras::hooks::trust::load_dispatcher(
            cli.no_hooks,
            !is_interactive,
        ));

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

    let mut boot = ui::boot::Boot::new(is_interactive && cfg.resolve_show_boot_screen());

    // Config was already read above (its `show_boot_screen` key gates the
    // startup log itself); report where it came from as completed steps —
    // one line for the global file, a separate one for a project-local
    // override when present.
    let config_path = config::config_file_path();
    let config_source = if is_first_startup {
        format!(
            "{} (created with defaults)",
            ui::boot::abbreviate_home(&config_path)
        )
    } else {
        ui::boot::abbreviate_home(&config_path)
    };
    boot.loaded("Global config", &config_source);
    if std::path::Path::new(config::LOCAL_CONFIG_PATH).exists() {
        boot.loaded("Local config", config::LOCAL_CONFIG_PATH);
    }

    let mut startup = startup::Startup::init(
        cli,
        cfg,
        is_first_startup,
        version_changed,
        is_interactive,
        &mut boot,
    )
    .await?;

    // ACP mode: serve and exit before feature init
    #[cfg(feature = "acp")]
    if startup.cli.acp_enabled {
        return extras::acp::serve(startup.cli, startup.cfg, startup.context).await;
    }

    startup.init_features(&mut boot).await?;
    startup.resolve_prompts().await?;
    startup.dispatch(boot).await
}
