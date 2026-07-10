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
mod provider;
mod retry;
mod sandbox;
mod session;
mod ui;

#[cfg(test)]
mod tests;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use clap::Parser;
use compact_str::CompactString;
use session::MessageRole;
#[cfg(feature = "advisor")]
use session::{Session, SessionMessage};

use crate::agent::tools;
use crate::extras::status_signals::StatusSignals;
use crate::permission::SecurityMode;
use crate::permission::ask::AskSender;
use crate::permission::checker::{PermCheck, PermissionChecker};

fn resolve_mode(cli: &cli::Cli, cfg: &config::Config) -> SecurityMode {
    if cli.yolo || cfg.yolo.unwrap_or(false) {
        SecurityMode::Yolo
    } else if cli.accept_all || cfg.accept_all.unwrap_or(false) {
        SecurityMode::Standard
    } else if cli.read_only {
        SecurityMode::ReadOnly
    } else if cli.guarded {
        SecurityMode::Guarded
    } else if cli.restrictive || cfg.restrictive.unwrap_or(false) {
        SecurityMode::Restrictive
    } else if let Some(m) = &cfg.default_permission_mode {
        match m.as_str() {
            "yolo" => SecurityMode::Yolo,
            "accept" => SecurityMode::Standard,
            "standard" => SecurityMode::Standard,
            "guarded" => SecurityMode::Guarded,
            "readonly" => SecurityMode::ReadOnly,
            "restrictive" => SecurityMode::Restrictive,
            _ => SecurityMode::Standard,
        }
    } else {
        SecurityMode::Standard
    }
}

fn build_permission_checker(
    cli: &cli::Cli,
    cfg: &config::Config,
) -> (
    Option<PermCheck>,
    Option<AskSender>,
    Option<tokio::sync::mpsc::Receiver<crate::permission::ask::AskRequest>>,
) {
    let no_tools = cli.resolve_no_tools(cfg);
    if no_tools {
        return (None, None, None);
    }

    if cli.dangerously_skip_permissions {
        return (None, None, None);
    }

    let perm_config = cfg.build_permission_config();

    let mode = resolve_mode(cli, cfg);
    let permission_modes = cfg.permission_modes.clone();
    let checker = PermissionChecker::new(&perm_config, mode, None, permission_modes);
    let perm: PermCheck = std::sync::Arc::new(std::sync::Mutex::new(checker));

    let (ask_tx, ask_rx) = tokio::sync::mpsc::channel(64);
    (Some(perm), Some(ask_tx), Some(ask_rx))
}

/// Apply the `[prompt_to_model]` mapping at startup before the TUI is
/// available. Updates `provider`, `model`, and `session` fields so the
/// initial agent is built with the correct model.
fn apply_startup_prompt_model(
    prompt_name: &str,
    cfg: &config::Config,
    provider: &mut CompactString,
    model: &mut CompactString,
    session: &mut session::Session,
) {
    let qm_name = match cfg.resolve_prompt_model(prompt_name) {
        Some(name) => name,
        None => return,
    };
    let qm = config::quick_models_map(cfg);
    let Some(qmc) = qm.get(qm_name) else {
        return;
    };
    *provider = qmc.provider.clone();
    *model = qmc.model.clone();
    session.model = qmc.model.clone();
    session.provider = qmc.provider.clone();
    session.input_token_cost = qmc.input_token_cost;
    session.output_token_cost = qmc.output_token_cost;
    session.update_context_window(cfg.resolve_context_window(
        &session.provider,
        &session.model,
        &qm,
    ));
}

/// Connect configured MCP servers for a headless (`-p`/`--loop`) run. Unlike
/// the TUI (`ui::ensure_mcp_manager`), headless has no alt-screen to protect,
/// so connection failures are printed to stderr instead of staying silent
/// until surfaced by the renderer.
#[cfg(feature = "mcp")]
async fn connect_headless_mcp(
    cfg: &config::Config,
) -> Option<crate::extras::mcp::McpClientManager> {
    let servers = cfg.mcp_servers.as_ref()?;
    if servers.is_empty() {
        return None;
    }
    let manager = crate::extras::mcp::McpClientManager::connect_all(servers).await;
    for notice in &manager.notices {
        eprintln!("{}", notice);
    }
    Some(manager)
}

#[cfg_attr(
    feature = "multithread",
    tokio::main(flavor = "multi_thread", worker_threads = 4)
)]
#[cfg_attr(not(feature = "multithread"), tokio::main(flavor = "current_thread"))]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    logging::install_panic_hook();
    logging::init(&cli);

    let (mut cfg, is_first_startup) = config::load();

    if cli.print_config {
        print_config(&cli, &cfg);
        return Ok(());
    }

    if cli.resume && cli.session.is_none() {
        print_sessions();
        return Ok(());
    }

    let version_changed = docs::ensure_global()?;
    #[cfg(feature = "acp")]
    let is_interactive = !cli.acp_enabled && !cli.print && !cli.loop_mode;
    #[cfg(not(feature = "acp"))]
    let is_interactive = !cli.print && !cli.loop_mode;

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

    // Load context first so prompts/themes are available early.
    // (Version-change / ARCHITECTURE.md prompts are deferred to right before
    // the TUI to avoid blocking startup on stdin.)
    let mut context = context::load(cli.resolve_no_context_files(&cfg));

    let mut provider = cli.resolve_provider(&cfg);
    let mut model = cli.resolve_model(&cfg);

    // --quick-model overrides provider + model
    if let Some(qm) = cli.resolve_quick_model(&cfg) {
        provider = qm.provider.clone();
        model = qm.model.clone();
    }

    let name = cli.name.as_deref().unwrap_or("");
    let qm_map = config::quick_models_map(&cfg);
    let mut session = session::Session::new(
        &provider,
        &model,
        cfg.resolve_context_window(&provider, &model, &qm_map),
        name,
    );

    // Resolve input/output token costs from quick models or defaults
    if let Some(qm) = cli.resolve_quick_model(&cfg) {
        session.input_token_cost = qm.input_token_cost;
        session.output_token_cost = qm.output_token_cost;
    } else if let Some(qm) = qm_map
        .iter()
        .find(|(_, v)| v.model.as_str() == model && v.provider.as_str() == provider)
        .map(|(_, v)| v)
    {
        session.input_token_cost = qm.input_token_cost;
        session.output_token_cost = qm.output_token_cost;
    } else if let Some((input_cost, output_cost)) =
        config::Config::catalog_input_output_cost(&provider, &model)
    {
        session.input_token_cost = input_cost;
        session.output_token_cost = output_cost;
    }

    #[cfg(feature = "hooks")]
    let mut session_resumed = false;

    if cli.continue_session
        && cli.session.is_none()
        && let Ok(sessions) = session::storage::find_recent_sessions(1)
        && let Some(s) = sessions.into_iter().next()
    {
        session = s;
        #[cfg(feature = "hooks")]
        {
            session_resumed = true;
        }
    }

    if let Some(session_id) = &cli.session {
        let sessions = session::storage::find_sessions_by_prefix(session_id)?;
        if sessions.is_empty() {
            // try exact name match as fallback
            if let Some(s) = session::storage::find_session_by_name(session_id)? {
                session = s;
                #[cfg(feature = "hooks")]
                {
                    session_resumed = true;
                }
            } else {
                anyhow::bail!("no session matching '{}'", session_id);
            }
        } else if sessions.len() == 1 {
            session = sessions.into_iter().next().unwrap();
            #[cfg(feature = "hooks")]
            {
                session_resumed = true;
            }
        } else {
            eprintln!("multiple sessions match '{}':", session_id);
            for s in &sessions {
                let preview = s
                    .messages
                    .last()
                    .map(|m| {
                        let truncated: String = m.content.chars().take(40).collect();
                        truncated
                    })
                    .unwrap_or_default();
                let time = crate::ui::events::format_time(&s.updated_at);
                let name_part = if s.name.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", s.name)
                };
                eprintln!(
                    "  {}  {}  {}msgs  {}  {}{}",
                    &s.id[..8],
                    time,
                    s.messages.len(),
                    s.model,
                    preview,
                    name_part
                );
            }
            anyhow::bail!("be more specific with the session ID prefix");
        }
    }

    // A resumed session persisted its context_window when first saved, which can
    // be stale if the model's catalog entry has changed since (e.g. a model that
    // grew from 128k to 1M). Re-derive it from the catalog for the session's own
    // model, unless the user pinned `context_window` in config (then that wins).
    if cfg.context_window.is_none()
        && let Some(cw) = config::Config::catalog_context_window(
            session.provider.as_str(),
            session.model.as_str(),
        )
    {
        session.update_context_window(cw);
    }

    let mut client = provider::create_client(
        &provider,
        cli.api_key.as_deref(),
        &cfg.custom_providers_map(),
        cfg.api_keys.as_ref(),
    )?;

    #[cfg(feature = "subagents")]
    {
        let task_max_turns = cfg.task_max_turns.unwrap_or(20);
        let qm = config::quick_models_map(&cfg);

        // Resolve subagent model: subagent_model config > subagent_provider + model > main model
        let (sub_provider, mut sub_model) = if let Some(sa_model) = &cfg.subagent_model {
            if let Some(q) = qm.get(sa_model.as_str()) {
                (q.provider.clone(), q.model.clone())
            } else {
                let prov = cfg
                    .subagent_provider
                    .clone()
                    .unwrap_or_else(|| provider.clone());
                (prov, sa_model.clone())
            }
        } else if let Some(sa_prov) = &cfg.subagent_provider {
            (sa_prov.clone(), model.clone())
        } else {
            (provider.clone(), model.clone())
        };

        let sub_client = if sub_provider.as_str() == provider {
            client.clone()
        } else {
            match crate::provider::create_client(
                &sub_provider,
                cli.api_key.as_deref(),
                &cfg.custom_providers_map(),
                cfg.api_keys.as_ref(),
            ) {
                Ok(c) => c,
                Err(e) => {
                    // The default subagent provider can differ from the main one
                    // (the built-in `deepseek-v4-pro` default uses OpenRouter).
                    // If its credentials are missing, don't abort the whole program:
                    // fall back to the main agent's client and model so users on a
                    // single provider (e.g. Anthropic-only) can still start.
                    tracing::warn!(
                        "Could not initialize subagent provider '{}' ({}); \
                         falling back to main provider '{}'. \
                         Set `subagent_provider`/`subagent_model` in config, or the \
                         provider's API key, to silence this.",
                        sub_provider,
                        e,
                        provider
                    );
                    sub_model = model.clone();
                    client.clone()
                }
            }
        };

        crate::extras::subagents::init(
            sub_client,
            sub_model.to_string(),
            task_max_turns,
            cfg.clone(),
            #[cfg(feature = "archmd")]
            context.architecture.clone(),
        );
    }

    // Fetch OpenRouter pricing and context window at startup so cost tracking
    // and the context meter work from the first turn. The static catalog only
    // covers a handful of models; models like z-ai/glm-5.2 fall through to the
    // 128k default, which silently breaks compaction and the context meter.
    if provider == "openrouter" {
        let need_pricing = session.input_token_cost == 0.0 && session.output_token_cost == 0.0;
        let need_ctx = cfg.context_window.is_none()
            && config::Config::catalog_context_window("openrouter", model.as_str()).is_none();
        if (need_pricing || need_ctx)
            && let Ok(infos) = provider::fetch_openrouter_pricing(
                cli.api_key.as_deref(),
                &cfg.custom_providers_map(),
                cfg.api_keys.as_ref(),
            )
            .await
            && let Some(info) = infos.get(model.as_str())
        {
            if need_pricing {
                session.input_token_cost = info.input_cost;
                session.output_token_cost = info.output_cost;
            }
            if need_ctx && let Some(cw) = info.context_length {
                session.update_context_window(cw);
            }
        }
    }

    #[cfg(feature = "acp")]
    if cli.acp_enabled {
        return extras::acp::serve(cli, cfg, context).await;
    }

    let sandbox = sandbox::Sandbox::new(
        cli.resolve_sandbox(&cfg),
        &cli.resolve_sandbox_backend(&cfg),
    )
    .with_shell(&cli.resolve_shell(&cfg));
    if cli.resolve_sandbox(&cfg) && !sandbox.is_effectively_sandboxed() {
        tracing::warn!(
            "sandbox is enabled but backend '{}' was not found — commands will run unsandboxed",
            cli.resolve_sandbox_backend(&cfg)
        );
    }
    let edit_system = cli.resolve_edit_system(&cfg);
    tools::set_edit_system(edit_system);
    tools::set_deny_repeated_reads(cfg.deny_repeated_reads.unwrap_or(true));
    #[cfg(feature = "status-signals")]
    let status_signals = cli.status_socket.clone().map(StatusSignals::new);
    #[cfg(not(feature = "status-signals"))]
    let status_signals: Option<StatusSignals> = None;
    let (permission, ask_tx, ask_rx) = build_permission_checker(&cli, &cfg);

    #[cfg(feature = "advisor")]
    let handoff_rx = {
        let enabled = cli.resolve_advisor_enabled(&cfg);
        let human_handoff = cli.resolve_advisor_human_handoff(&cfg);
        let advisor_model_name = cli.resolve_advisor_model(&cfg);
        let max_uses = cli.resolve_advisor_max_uses(&cfg);
        let kilobytes_limit = cli.resolve_advisor_kilobytes_limit(&cfg);

        let qm = config::quick_models_map(&cfg);
        let (advisor_provider, advisor_model) = if let Some(q) = qm.get(advisor_model_name.as_str())
        {
            (q.provider.to_string(), q.model.to_string())
        } else {
            (provider.to_string(), advisor_model_name)
        };

        let advisor_client = if advisor_provider == provider {
            Some(client.clone())
        } else {
            match crate::provider::create_client(
                &advisor_provider,
                cli.api_key.as_deref(),
                &cfg.custom_providers_map(),
                cfg.api_keys.as_ref(),
            ) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!(
                        "Could not create advisor client for provider '{}' ({}); \
                         advisor disabled. Set `advisor.model` and API key in config.",
                        advisor_provider,
                        e
                    );
                    None
                }
            }
        };

        let (handoff_tx, handoff_rx) = if human_handoff && is_interactive {
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        let config = crate::extras::advisor::AdvisorToolConfig {
            client: advisor_client,
            advisor_model,
            human_handoff,
            max_uses,
            handoff_tx,
            enabled,
            kilobytes_limit,
        };
        crate::extras::advisor::init_config(config);

        handoff_rx
    };
    let completion_model = client.completion_model(model.to_string());

    // ── Interactive prompts (last thing before TUI dispatch) ──

    // Version-change prompts: defer to here so all heavy setup completes first.
    if version_changed && is_interactive && !is_first_startup {
        let prompts_dir = context::prompts::global_dir();
        let themes_dir = context::themes::global_dir();
        let mut regenerated = false;

        // Prompts: check config override, then fall back to asking or auto-regen
        match cfg.resolve_auto_update_prompts() {
            Some(true) => {
                let _ = context::prompts::regen();
                eprintln!("Prompts regenerated.");
                regenerated = true;
            }
            Some(false) => { /* skip: user explicitly denied */ }
            None => {
                if !prompts_dir.exists() {
                    let _ = context::prompts::regen();
                    eprintln!("Prompts regenerated (first launch).");
                    regenerated = true;
                } else {
                    let mut input = String::new();
                    eprint!("Regenerate prompts? [y/N] ");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    std::io::stdin().read_line(&mut input)?;
                    if matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                        let _ = context::prompts::regen();
                        eprintln!("Prompts regenerated.");
                        regenerated = true;
                    }
                }
            }
        }

        // Themes: check config override, then fall back to asking or auto-regen
        match cfg.resolve_auto_update_themes() {
            Some(true) => {
                let _ = context::themes::regen();
                eprintln!("Themes regenerated.");
                regenerated = true;
            }
            Some(false) => { /* skip: user explicitly denied */ }
            None => {
                if !themes_dir.exists() {
                    let _ = context::themes::regen();
                    eprintln!("Themes regenerated (first launch).");
                    regenerated = true;
                } else {
                    let mut input = String::new();
                    eprint!("Regenerate themes? [y/N] ");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    std::io::stdin().read_line(&mut input)?;
                    if matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                        let _ = context::themes::regen();
                        eprintln!("Themes regenerated.");
                        regenerated = true;
                    }
                }
            }
        }

        if regenerated {
            // Reload context to pick up freshly-regenerated prompts/themes
            context = context::load(cli.resolve_no_context_files(&cfg));
        }
    }

    // ── Recommended MCP prompts on first startup ──
    #[cfg(feature = "mcp")]
    if is_first_startup && is_interactive {
        let prompted = cfg.enable_context7_mcp.is_none() || cfg.enable_grepapp_mcp.is_none();
        if prompted {
            if cfg.enable_context7_mcp.is_none() {
                let mut input = String::new();
                eprint!("Enable Context7 MCP (documentation and code context lookup)? [y/N] ");
                let _ = std::io::Write::flush(&mut std::io::stderr());
                std::io::stdin().read_line(&mut input)?;
                let enable = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                cfg.enable_context7_mcp = Some(enable);
                if enable {
                    eprintln!("Context7 MCP enabled.");
                }
            }
            if cfg.enable_grepapp_mcp.is_none() {
                let mut input = String::new();
                eprint!("Enable Grep.app MCP (semantic code search across repositories)? [y/N] ");
                let _ = std::io::Write::flush(&mut std::io::stderr());
                std::io::stdin().read_line(&mut input)?;
                let enable = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
                cfg.enable_grepapp_mcp = Some(enable);
                if enable {
                    eprintln!("Grep.app MCP enabled.");
                }
            }
            config::inject_mcp_defaults(&mut cfg);
            if let Err(e) = config::save_config(&cfg) {
                tracing::warn!("Failed to save config with MCP choices: {e}");
            }
        }
    }

    // `SessionStart` fires here (dispatcher already installed above, before
    // provider/API-key resolution) once `session_resumed` is known.
    #[cfg(feature = "hooks")]
    {
        let source = if session_resumed { "resume" } else { "startup" };
        crate::extras::hooks::dispatch_session_start(source).await;
    }

    // ARCHITECTURE.md prompt: defer to here so all heavy setup completes first.
    #[cfg(feature = "archmd")]
    let arch_created = if !cli.resolve_no_context_files(&cfg) {
        let cwd = std::env::current_dir().ok();
        if let Some(ref cwd) = cwd {
            crate::extras::archmd::ask_and_create(cwd).unwrap_or_else(|e| {
                tracing::warn!("Architecture.md prompt failed: {e}");
                false
            })
        } else {
            false
        }
    } else {
        false
    };

    // Reload context after potential ARCHITECTURE.md creation
    #[cfg(feature = "archmd")]
    if arch_created {
        context.architecture = crate::context::load_architecture();
    }

    // Default prompt resolution (after prompts may have been regenerated)
    {
        let default_prompt = cfg.default_prompt.as_deref().unwrap_or("code");
        if let Some(content) = context.prompts.get(default_prompt) {
            let (mode_directive, clean_content) = crate::permission::parse_prompt_mode(content);
            let mut prompt_text = if mode_directive.is_some() {
                clean_content.to_string()
            } else {
                content.clone()
            };

            let caps: &[&str] = &[
                #[cfg(feature = "memory")]
                "- **Memory**: persistent memory across sessions (memory_read, memory_write, memory_search)",
                #[cfg(feature = "subagents")]
                "- **Subagents**: delegate specific multi-step investigations to parallel subagents via the `task` tool",
            ];

            if !caps.is_empty() {
                prompt_text.push_str("\n\n## Available Capabilities\n\n");
                prompt_text.push_str(&caps.join("\n"));
                prompt_text.push('\n');
            }

            context.current_prompt = Some(prompt_text);
            context.current_prompt_name = Some(default_prompt.to_string());

            // Apply prompt-to-model mapping
            apply_startup_prompt_model(
                default_prompt,
                &cfg,
                &mut provider,
                &mut model,
                &mut session,
            );
        }
    }

    // --load-prompt overrides the default prompt
    if let Some(ref name) = cli.load_prompt {
        if let Some(content) = context.prompts.get(name) {
            let (mode_directive, clean_content) = crate::permission::parse_prompt_mode(content);
            let mut prompt_text = if mode_directive.is_some() {
                clean_content.to_string()
            } else {
                content.clone()
            };

            let caps: &[&str] = &[
                #[cfg(feature = "memory")]
                "- **Memory**: persistent memory across sessions (memory_read, memory_write, memory_search)",
                #[cfg(feature = "subagents")]
                "- **Subagents**: delegate specific multi-step investigations to parallel subagents via the `task` tool",
            ];

            if !caps.is_empty() {
                prompt_text.push_str("\n\n## Available Capabilities\n\n");
                prompt_text.push_str(&caps.join("\n"));
                prompt_text.push('\n');
            }

            context.current_prompt = Some(prompt_text);
            context.current_prompt_name = Some(name.clone());

            // Apply prompt-to-model mapping
            apply_startup_prompt_model(name, &cfg, &mut provider, &mut model, &mut session);
        } else {
            let mut sorted: Vec<&String> = context.prompts.keys().collect();
            sorted.sort();
            eprintln!("error: unknown prompt '{}'", name);
            eprintln!("available prompts:");
            for p in &sorted {
                eprintln!("  {}", p);
            }
            anyhow::bail!("unknown prompt '{}'", name);
        }
    }

    // Rebuild client if the provider changed due to prompt-to-model mapping
    if client.provider_name() != provider.as_str() {
        match provider::create_client(
            &provider,
            cli.api_key.as_deref(),
            &cfg.custom_providers_map(),
            cfg.api_keys.as_ref(),
        ) {
            Ok(new_client) => {
                client = new_client;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to rebuild client for prompt-mapped provider '{}': {}",
                    provider,
                    e
                );
            }
        }
    }

    // Apply mode from prompt %%mode= directive (if any)
    if let Some(perm) = &permission {
        let allowlist: Vec<(String, String)> = session
            .permission_allowlist
            .iter()
            .map(|e| (e.tool.to_string(), e.pattern.to_string()))
            .collect();
        let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
        guard.load_session_allowlist(&allowlist);
        if let Some(current_prompt) = &context.current_prompt {
            let (mode_directive, _) = crate::permission::parse_prompt_mode(current_prompt);
            if let Some(mode_str) = mode_directive
                && mode_str != "last_user_mode"
                && let Some(mode) = SecurityMode::from_str(mode_str)
            {
                guard.set_prompt_mode(mode);
            }
        }
    }

    // Build the auto-trigger message for ARCHITECTURE.md creation
    #[cfg(feature = "archmd")]
    let arch_msg: Option<String> = if arch_created {
        Some(
            "I've just created an empty ARCHITECTURE.md template at the project root. \
            Explore the codebase thoroughly using the `task` tool (delegating parallel exploration to subagents) \
            and fill ARCHITECTURE.md with a high-level architecture document covering:\n\
            - Directory layout and module responsibilities\n\
            - Key types, traits, and their relationships\n\
            - Control flow (how requests/events flow through the system)\n\
            - Data flow (how data is transformed from input to output)\n\
            - Design decisions and rationale\n\
            - External dependencies and how they are used\n\
            - Entry points for different execution modes\n\n\
            Keep the document under ~300 lines of code total. Keep entries concise and reference specific source files."
                .to_string(),
        )
    } else {
        None
    };
    #[cfg(not(feature = "archmd"))]
    let arch_msg: Option<String> = None;

    if cli.print {
        let msg = cli.message.join(" ");
        if msg.starts_with('!') {
            let cmd = msg.strip_prefix('!').map(|s| s.trim()).unwrap_or("");
            if !cmd.is_empty() {
                let output = std::process::Command::new("bash")
                    .arg("-c")
                    .arg(cmd)
                    .output()?;
                let mut result = String::new();
                if !output.stdout.is_empty() {
                    result.push_str(&String::from_utf8_lossy(&output.stdout));
                }
                if !output.stderr.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&String::from_utf8_lossy(&output.stderr));
                }
                let result = result.trim().to_string();
                println!("{}", result);
                if !cli.no_session {
                    session.add_message(MessageRole::User, &msg);
                    session.add_message(MessageRole::Assistant, &result);
                    session::storage::save_session(&session)?;
                    let _ = session::chat_history::append_entry(
                        &session::chat_history::ChatHistoryEntry {
                            content: msg,
                            timestamp: session.updated_at.clone(),
                        },
                    );
                }
            } else {
                eprintln!("error: empty command after '!'");
            }
        } else {
            let temperature = config::resolve_temperature(&cli, &cfg, &model);
            let extra_body = config::resolve_extra_body(&cfg, &model);
            #[cfg(feature = "mcp")]
            let mcp_manager = connect_headless_mcp(&cfg).await;
            let agent = provider::build_agent(
                completion_model,
                &cli,
                &cfg,
                &context,
                permission,
                ask_tx,
                sandbox.clone(),
                true,
                temperature,
                extra_body,
                #[cfg(feature = "mcp")]
                mcp_manager.as_ref(),
            )
            .await;
            #[cfg(feature = "advisor")]
            {
                let mut msgs = session.messages.clone();
                msgs.push(SessionMessage {
                    role: MessageRole::User,
                    content: CompactString::new(&msg),
                    estimated_tokens: Session::estimate_tokens(&msg),
                });
                crate::extras::advisor::set_session_messages(msgs);
            }
            if let Some(ss) = status_signals.as_ref() {
                ss.send_start();
            }
            let response_result = agent
                .run_print(
                    &msg,
                    cli.pure_stdout,
                    &cfg.retry,
                    #[cfg(feature = "hooks")]
                    None,
                )
                .await;
            if let Some(ss) = status_signals.as_ref() {
                ss.send_stop();
            }
            let (response, usage) = response_result?;
            if !cli.no_session {
                session.add_message(MessageRole::User, &msg);
                session.add_message(MessageRole::Assistant, &response);
                session.total_input_tokens = session
                    .total_input_tokens
                    .saturating_add(usage.input_tokens);
                session.total_output_tokens = session
                    .total_output_tokens
                    .saturating_add(usage.output_tokens);
                session.total_cached_input_tokens = session
                    .total_cached_input_tokens
                    .saturating_add(usage.cached_input_tokens);
                session.total_cache_creation_input_tokens = session
                    .total_cache_creation_input_tokens
                    .saturating_add(usage.cache_creation_input_tokens);
                session.total_cost += crate::pricing::estimate_cost(
                    crate::pricing::billable_input_tokens(
                        cfg.is_anthropic_native(&session.provider),
                        usage.input_tokens,
                        usage.cached_input_tokens,
                        usage.cache_creation_input_tokens,
                    ),
                    usage.output_tokens,
                    session.input_token_cost,
                    session.output_token_cost,
                );
                session::storage::save_session(&session)?;
                let _ =
                    session::chat_history::append_entry(&session::chat_history::ChatHistoryEntry {
                        content: msg,
                        timestamp: session.updated_at.clone(),
                    });
            }
        }
    } else {
        #[cfg(feature = "loop")]
        if cli.loop_mode {
            let model_completion = client.completion_model(model.to_string());
            let temperature = config::resolve_temperature(&cli, &cfg, &model);
            let extra_body = config::resolve_extra_body(&cfg, &model);
            #[cfg(feature = "mcp")]
            let mcp_manager = connect_headless_mcp(&cfg).await;
            let agent = provider::build_agent(
                model_completion,
                &cli,
                &cfg,
                &context,
                permission,
                ask_tx,
                sandbox.clone(),
                true,
                temperature,
                extra_body,
                #[cfg(feature = "mcp")]
                mcp_manager.as_ref(),
            )
            .await;
            let result = run_headless_loop(agent, &cli, &cfg, &context, status_signals).await;
            #[cfg(feature = "hooks")]
            crate::extras::hooks::dispatch_session_end("exit").await;
            return result;
        }

        if !cli.resolve_no_tools(&cfg)
            && let Some(perm) = &permission
        {
            let mode = resolve_mode(&cli, &cfg);
            perm.lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_mode(mode);
        }

        let initial_msg = cli.message.join(" ");
        if !initial_msg.is_empty() {
            session.add_message(MessageRole::User, &initial_msg);
        }
        ui::run_interactive(
            client,
            None,
            &cli,
            &cfg,
            &mut session,
            &mut context,
            permission,
            ask_tx,
            ask_rx,
            sandbox,
            arch_msg,
            status_signals,
            #[cfg(feature = "advisor")]
            handoff_rx,
        )
        .await?;
    }

    #[cfg(feature = "hooks")]
    crate::extras::hooks::dispatch_session_end("exit").await;

    Ok(())
}

fn print_section(title: &str, entries: &[(&str, String)]) {
    println!("{}:", title);
    let width = entries.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (k, v) in entries {
        println!("  {k:<width$}  {v}");
    }
    println!();
}

fn print_sessions() {
    let sessions = match session::storage::find_recent_sessions(20) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error listing sessions: {e}");
            return;
        }
    };
    if sessions.is_empty() {
        println!("no saved sessions");
    } else {
        println!("recent sessions ({}):", sessions.len());
        for s in &sessions {
            let last = s
                .messages
                .last()
                .map(|m| {
                    let truncated: String = m.content.chars().take(30).collect();
                    format!("...{truncated}")
                })
                .unwrap_or_default();
            let time = crate::ui::events::format_time(&s.updated_at);
            let name_col = if s.name.is_empty() {
                String::new()
            } else {
                format!("  [{}]", s.name)
            };
            println!(
                "  {}  {}  {}msgs  {}  {}{}",
                &s.id[..8],
                time,
                s.messages.len(),
                s.model,
                last,
                name_col
            );
        }
        println!();
        println!("Use --session <id-or-name> to load a session by its ID prefix or name.");
    }
}

fn print_config(cli: &cli::Cli, cfg: &config::Config) {
    let config_dir = session::storage::config_path();
    let data_dir = session::storage::data_dir();
    let sessions_dir = data_dir.join("sessions");
    let config_file = config::config_file_path();

    let model = cli.resolve_model(cfg);
    let provider = cli.resolve_provider(cfg);
    let qm_map = config::quick_models_map(cfg);
    let max_tokens = cli.resolve_max_tokens(cfg);
    let max_agent_turns = cli.resolve_max_agent_turns(cfg);
    let context_window = cfg.resolve_context_window(&provider, &model, &qm_map);
    let temperature = config::resolve_temperature(cli, cfg, &model);
    let no_tools = cli.resolve_no_tools(cfg);
    let no_context_files = cli.resolve_no_context_files(cfg);
    let sandbox = cli.resolve_sandbox(cfg);
    let shell = cli.resolve_shell(cfg);
    let edit_system = cli.resolve_edit_system(cfg);
    let compact = cfg.resolve_compact_enabled();

    let mode = if cli.dangerously_skip_permissions {
        "dangerously-skip-permissions"
    } else if cli.yolo || cfg.yolo.unwrap_or(false) {
        "yolo"
    } else if cli.accept_all || cfg.accept_all.unwrap_or(false) {
        "standard"
    } else if cli.read_only {
        "readonly"
    } else if cli.guarded {
        "guarded"
    } else if cli.restrictive || cfg.restrictive.unwrap_or(false) {
        "restrictive"
    } else {
        cfg.default_permission_mode.as_deref().unwrap_or("standard")
    };

    print_section(
        "Directories",
        &[
            ("config", config_dir.display().to_string()),
            ("data", data_dir.display().to_string()),
            ("sessions", sessions_dir.display().to_string()),
            ("config file", config_file.display().to_string()),
        ],
    );

    let mut model_entries = vec![
        ("provider", provider.to_string()),
        ("model", model.to_string()),
    ];
    if let Some(temp) = temperature {
        model_entries.push(("temperature", temp.to_string()));
    }
    print_section("Model", &model_entries);

    let fmt_opt = |v: Option<u64>| -> String {
        match v {
            Some(n) => n.to_string(),
            None => "— (no cap)".to_string(),
        }
    };

    let mut limit_entries: Vec<(&str, String)> = vec![
        ("max-tokens", max_tokens.to_string()),
        ("max-agent-turns", max_agent_turns.to_string()),
        ("context-window", context_window.to_string()),
        (
            "reserve-tokens",
            cfg.resolve_reserve_tokens(&model, &qm_map).to_string(),
        ),
        ("max-read-lines", cfg.resolve_max_read_lines().to_string()),
        (
            "max-bash-output-lines",
            fmt_opt(cfg.resolve_max_bash_output_lines()),
        ),
        (
            "max-grep-results",
            cfg.resolve_max_grep_results().to_string(),
        ),
        (
            "max-find-results",
            cfg.resolve_max_find_results().to_string(),
        ),
        (
            "max-list-dir-entries",
            fmt_opt(cfg.resolve_max_list_dir_entries()),
        ),
    ];
    #[cfg(feature = "subagents")]
    {
        limit_entries.push((
            "subagent-max-read-lines",
            cfg.resolve_subagent_max_read_lines().to_string(),
        ));
        limit_entries.push((
            "subagent-max-grep-results",
            cfg.resolve_subagent_max_grep_results().to_string(),
        ));
        limit_entries.push((
            "subagent-max-find-results",
            cfg.resolve_subagent_max_find_results().to_string(),
        ));
        limit_entries.push((
            "subagent-max-list-dir-entries",
            fmt_opt(cfg.resolve_subagent_max_list_dir_entries()),
        ));
    }
    print_section("Limits", &limit_entries);

    print_section(
        "Behavior",
        &[
            ("permission-mode", mode.to_string()),
            ("shell", shell.to_string()),
            ("edit-system", edit_system.to_string()),
            ("sandbox", sandbox.to_string()),
            ("no-tools", no_tools.to_string()),
            ("no-context-files", no_context_files.to_string()),
            ("compact", compact.to_string()),
        ],
    );

    #[cfg(feature = "advisor")]
    {
        let advisor_enabled = cli.resolve_advisor_enabled(cfg);
        let human_handoff = cli.resolve_advisor_human_handoff(cfg);
        let advisor_model = cli.resolve_advisor_model(cfg);
        let max_uses = cli
            .resolve_advisor_max_uses(cfg)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unlimited".to_string());
        print_section(
            "Advisor",
            &[
                ("enabled", advisor_enabled.to_string()),
                ("model", advisor_model),
                ("human-handoff", human_handoff.to_string()),
                ("max-uses", max_uses),
                (
                    "context-limit",
                    format!(
                        "{} KB ({} head / {} tail)",
                        cli.resolve_advisor_kilobytes_limit(cfg),
                        cli.resolve_advisor_kilobytes_limit(cfg) * 1024 / 2,
                        cli.resolve_advisor_kilobytes_limit(cfg) * 1024 / 2,
                    ),
                ),
            ],
        );
    }
}

#[cfg(feature = "loop")]
async fn run_headless_loop(
    agent: crate::provider::AnyAgent,
    cli: &cli::Cli,
    cfg: &config::Config,
    _context: &context::ContextFiles,
    status_signals: Option<StatusSignals>,
) -> anyhow::Result<()> {
    use std::path::PathBuf;
    use uuid::Uuid;

    use crate::extras::r#loop as loop_mod;

    let prompt = cli
        .loop_prompt
        .clone()
        .or_else(|| {
            let msg = cli.message.join(" ");
            if msg.is_empty() { None } else { Some(msg) }
        })
        .ok_or_else(|| anyhow::anyhow!("No loop prompt. Use --loop-prompt or pass a message."))?;

    let plan_file = cli
        .loop_plan
        .clone()
        .unwrap_or_else(|| PathBuf::from(loop_mod::DEFAULT_PLAN_FILENAME));
    let max_iterations = cli.loop_max;
    let run_cmd = cli.loop_run.clone();
    let session_id = Uuid::new_v4().to_string();

    let use_existing = loop_mod::plan::handle_startup(&plan_file).await?;
    if !use_existing {
        // No plan exists — agent will generate one on first iteration
    }

    let mut state = loop_mod::LoopState::new(prompt, plan_file, max_iterations, run_cmd);

    loop {
        state.iteration += 1;

        if state.should_stop() {
            eprintln!(
                "[loop] max iterations ({}) reached, stopping",
                state.max_iterations.unwrap_or(0)
            );
            break;
        }

        let iteration_prompt = state.build_prompt();

        eprintln!("=== {} ===", state.iteration_label());
        eprintln!();

        if let Some(ss) = status_signals.as_ref() {
            ss.send_start();
        }
        let response = match agent
            .run_print(
                &iteration_prompt,
                cli.pure_stdout,
                &cfg.retry,
                #[cfg(feature = "hooks")]
                Some(crate::extras::hooks::LoopInfo {
                    iteration: state.iteration,
                    active: state.active,
                }),
            )
            .await
        {
            Ok((r, _usage)) => {
                if let Some(ss) = status_signals.as_ref() {
                    ss.send_stop();
                }
                r
            }
            Err(e) => {
                if let Some(ss) = status_signals.as_ref() {
                    ss.send_stop();
                }
                eprintln!("[loop] error in iteration {}: {}", state.iteration, e);
                break;
            }
        };

        let summary: String = response
            .chars()
            .take(loop_mod::SUMMARY_TRUNCATION_CHARS)
            .collect();
        state.last_summary = Some(summary.clone());

        let validation_output = if let Some(cmd) = &state.run_cmd {
            eprintln!("--- Validation: {} ---", cmd);
            let shell = if cfg!(windows) { "powershell" } else { "sh" };
            let shell_arg = if cfg!(windows) { "-Command" } else { "-c" };
            match tokio::process::Command::new(shell)
                .arg(shell_arg)
                .arg(cmd)
                .output()
                .await
            {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let combined = if stderr.is_empty() {
                        stdout
                    } else {
                        format!("{}\n{}", stdout, stderr)
                    };
                    eprintln!("{}", combined);
                    Some(combined)
                }
                Err(e) => {
                    let msg = format!("error: {}", e);
                    eprintln!("{}", msg);
                    Some(msg)
                }
            }
        } else {
            None
        };
        state.last_run_output = validation_output.clone();

        if let Err(e) = loop_mod::transcript::save_iteration(
            &session_id,
            state.iteration,
            &iteration_prompt,
            &response,
            validation_output.as_deref(),
            &summary,
        ) {
            eprintln!("[loop] warning: failed to save transcript: {}", e);
        }

        eprintln!("--- iteration {} complete, looping ---\n", state.iteration);
    }

    Ok(())
}
