mod agent;
mod auth;
mod cli;
mod config;
mod context;
mod docs;
mod event;
mod extras;
mod fs;
mod permission;
mod pricing;
mod provider;
mod sandbox;
mod session;
mod ui;

#[cfg(test)]
mod tests;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use clap::Parser;
use session::MessageRole;

use crate::agent::tools;
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,rig=off")),
        )
        .init();

    let cli = cli::Cli::parse();
    let cfg = config::load();

    if cli.print_config {
        print_config(&cli, &cfg);
        return Ok(());
    }

    let _ = docs::ensure_global();
    let mut context = context::load(cli.resolve_no_context_files(&cfg));

    #[cfg(feature = "archmd")]
    let arch_created = if !cli.resolve_no_context_files(&cfg) {
        let cwd = std::env::current_dir().ok();
        if let Some(ref cwd) = cwd {
            crate::extras::archmd::ask_and_create(cwd).unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    };

    // Reload context after potential ARCHITECTURE.md creation
    #[cfg(feature = "archmd")]
    {
        context.architecture = crate::context::load_architecture();
    }

    let default_prompt = cfg.default_prompt.as_deref().unwrap_or("code");
    let default_prompt_mode: Option<&str> = if let Some(content) =
        context.prompts.get(default_prompt)
    {
        let (mode_directive, clean_content) = crate::permission::parse_prompt_mode(content);
        let mut prompt_text = if mode_directive.is_some() {
            clean_content.to_string()
        } else {
            content.clone()
        };

        // Append available capabilities based on enabled features
        #[allow(unused_mut)]
        let mut caps: Vec<&str> = Vec::new();
        #[cfg(feature = "memory")]
            caps.push("- **Memory**: persistent memory across sessions (memory_read, memory_write, memory_search)");
        #[cfg(feature = "subagents")]
            caps.push("- **Subagents**: delegate read-only exploration to parallel subagents via the `task` tool");

        if !caps.is_empty() {
            prompt_text.push_str("\n\n## Available Capabilities\n\n");
            prompt_text.push_str(&caps.join("\n"));
            prompt_text.push('\n');
        }

        context.current_prompt = Some(prompt_text);
        context.current_prompt_name = Some(default_prompt.to_string());
        mode_directive
    } else {
        None
    };

    let mut provider = cli.resolve_provider(&cfg);
    let mut model = cli.resolve_model(&cfg);

    // --quick-model overrides provider + model
    if let Some(qm) = cli.resolve_quick_model(&cfg) {
        provider = qm.provider.clone();
        model = qm.model.clone();
    }

    // Custom provider model override (if no explicit model set)
    if let Some(custom) = cfg.custom_providers_map().get(provider.as_str())
        && cli.model.is_none()
        && cfg.model.is_none()
        && let Some(ref custom_model) = custom.model
    {
        model = custom_model.clone();
    }

    let mut session = session::Session::new(&provider, &model, cfg.resolve_context_window());

    // Resolve input/output token costs from quick models or defaults
    let qm_map = config::quick_models_map(&cfg);
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
    }

    if cli.resume && cli.session.is_none() && !cli.continue_session {
        let sessions = session::storage::find_recent_sessions(10)?;
        if sessions.is_empty() {
            eprintln!("No recent sessions found.");
        } else {
            eprintln!("Recent sessions:");
            for (i, s) in sessions.iter().enumerate() {
                let preview = s
                    .messages
                    .last()
                    .map(|m| {
                        let truncated: String = m.content.chars().take(60).collect();
                        truncated
                    })
                    .unwrap_or_default();
                eprintln!(
                    "  {}. {}  [{} msgs] {}",
                    i + 1,
                    &s.id[..8],
                    s.messages.len(),
                    preview
                );
            }
            if let Some(s) = sessions.into_iter().next() {
                session = s;
            }
        }
    }

    if cli.continue_session
        && cli.session.is_none()
        && let Ok(sessions) = session::storage::find_recent_sessions(1)
        && let Some(s) = sessions.into_iter().next()
    {
        session = s;
    }

    if let Some(session_id) = &cli.session {
        session = session::storage::load_session(session_id)?;
    }

    let client = provider::create_client(
        &provider,
        cli.api_key.as_deref(),
        &cfg.custom_providers_map(),
        cfg.api_keys.as_ref(),
    )?;

    #[cfg(feature = "subagents")]
    {
        let task_max_turns = cfg.task_max_turns.unwrap_or(15);
        let qm = config::quick_models_map(&cfg);

        // Resolve subagent model: subagent_model config > subagent_provider + model > deepseek-v4-flash quick model
        let (sub_provider, sub_model) = if let Some(sa_model) = &cfg.subagent_model {
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
        } else if let Some(dsv4) = qm.get("deepseek-v4-flash") {
            (dsv4.provider.clone(), dsv4.model.clone())
        } else {
            (provider.clone(), model.clone())
        };

        let sub_client = if sub_provider.as_str() == provider {
            client.clone()
        } else {
            crate::provider::create_client(
                &sub_provider,
                cli.api_key.as_deref(),
                &cfg.custom_providers_map(),
                cfg.api_keys.as_ref(),
            )?
        };

        crate::extras::subagents::init(
            sub_client,
            sub_model.to_string(),
            task_max_turns,
            #[cfg(feature = "archmd")]
            context.architecture.clone(),
        );
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
    let edit_system = cli.resolve_edit_system(&cfg);
    tools::set_edit_system(edit_system);
    let (permission, ask_tx, ask_rx) = build_permission_checker(&cli, &cfg);

    if let Some(perm) = &permission {
        let allowlist: Vec<(String, String)> = session
            .permission_allowlist
            .iter()
            .map(|e| (e.tool.to_string(), e.pattern.to_string()))
            .collect();
        let mut guard = perm.lock().unwrap_or_else(|e| e.into_inner());
        guard.load_session_allowlist(&allowlist);
        // Apply mode from prompt %%mode= directive (if any)
        if let Some(mode_str) = default_prompt_mode
            && mode_str != "last_user_mode"
            && let Some(mode) = SecurityMode::from_str(mode_str)
        {
            guard.set_prompt_mode(mode);
        }
    }

    let completion_model = client.completion_model(model.to_string());

    #[cfg(feature = "archmd")]
    if arch_created {
        let arch_msg = "I've just created an empty ARCHITECTURE.md template at the project root. \
            Explore the codebase thoroughly using the `task` tool (delegating parallel exploration to subagents) \
            and fill ARCHITECTURE.md with a high-level architecture document covering:\n\
            - Directory layout and module responsibilities\n\
            - Key types, traits, and their relationships\n\
            - Control flow (how requests/events flow through the system)\n\
            - Data flow (how data is transformed from input to output)\n\
            - Design decisions and rationale\n\
            - External dependencies and how they are used\n\
            - Entry points for different execution modes\n\n\
            Keep entries concise and reference specific source files.";
        session.add_message(MessageRole::User, arch_msg);
    }

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
            let agent = provider::build_agent(
                completion_model,
                &cli,
                &cfg,
                &context,
                permission,
                ask_tx,
                sandbox.clone(),
                true,
                #[cfg(feature = "mcp")]
                None,
            )
            .await;
            let response = agent
                .run_print(&msg, cli.resolve_max_agent_turns(&cfg))
                .await?;
            if !cli.no_session {
                session.add_message(MessageRole::User, &msg);
                session.add_message(MessageRole::Assistant, &response);
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
            let model = client.completion_model(model.to_string());
            let agent = provider::build_agent(
                model,
                &cli,
                &cfg,
                &context,
                permission,
                ask_tx,
                sandbox.clone(),
                true,
                #[cfg(feature = "mcp")]
                None,
            )
            .await;
            return run_headless_loop(agent, &cli, &cfg, &context).await;
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
        )
        .await?;
    }

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

fn print_config(cli: &cli::Cli, cfg: &config::Config) {
    let config_dir = session::storage::config_path();
    let data_dir = session::storage::data_dir();
    let sessions_dir = data_dir.join("sessions");
    let config_file = config::config_file_path();

    let model = cli.resolve_model(cfg);
    let provider = cli.resolve_provider(cfg);
    let max_tokens = cli.resolve_max_tokens(cfg);
    let max_agent_turns = cli.resolve_max_agent_turns(cfg);
    let context_window = cfg.resolve_context_window();
    let temperature = cli.temperature.or(cfg.temperature);
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

    print_section(
        "Limits",
        &[
            ("max-tokens", max_tokens.to_string()),
            ("max-agent-turns", max_agent_turns.to_string()),
            ("context-window", context_window.to_string()),
            ("reserve-tokens", cfg.resolve_reserve_tokens().to_string()),
        ],
    );

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
}

#[cfg(feature = "loop")]
async fn run_headless_loop(
    agent: crate::provider::AnyAgent,
    cli: &cli::Cli,
    cfg: &config::Config,
    _context: &context::ContextFiles,
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
        .unwrap_or_else(|| PathBuf::from("LOOP_PLAN.md"));
    let max_iterations = cli.loop_max;
    let run_cmd = cli.loop_run.clone();
    let session_id = Uuid::new_v4().to_string();

    let use_existing = loop_mod::plan::handle_startup(&plan_file)?;
    if !use_existing {
        // No plan exists — agent will generate one on first iteration
    }

    let mut state = loop_mod::LoopState::new(prompt, plan_file, max_iterations, run_cmd);

    loop {
        state.iteration += 1;

        if state.should_stop() {
            eprintln!(
                "[loop] max iterations ({}) reached, stopping",
                state.iteration
            );
            break;
        }

        let iteration_prompt = state.build_prompt();

        eprintln!("=== {} ===", state.iteration_label());
        eprintln!();

        let response = match agent
            .run_print(&iteration_prompt, cli.resolve_max_agent_turns(cfg))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[loop] error in iteration {}: {}", state.iteration, e);
                break;
            }
        };

        let summary: String = response.chars().take(300).collect();
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
