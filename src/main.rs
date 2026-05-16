mod agent;
mod cli;
mod config;
mod context;
mod event;
mod extras;
mod permission;
mod provider;
mod session;
mod ui;

use clap::Parser;
use session::MessageRole;

use crate::permission::ask::AskSender;
use crate::permission::checker::{PermCheck, PermissionChecker};
use crate::permission::{PermissionConfig, SecurityMode};

fn resolve_mode(cli: &cli::Cli, cfg: &config::Config) -> SecurityMode {
    if cli.yolo || cfg.yolo.unwrap_or(false) {
        SecurityMode::Yolo
    } else if cli.accept_all || cfg.accept_all.unwrap_or(false) {
        SecurityMode::Accept
    } else if cli.restrictive || cfg.restrictive.unwrap_or(false) {
        SecurityMode::Restrictive
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

    let perm_config: PermissionConfig = cfg
        .permission
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let mode = resolve_mode(cli, cfg);
    let checker = PermissionChecker::new(&perm_config, mode, None);
    let perm: PermCheck = std::sync::Arc::new(std::sync::Mutex::new(checker));

    let (ask_tx, ask_rx) = tokio::sync::mpsc::channel(64);
    (Some(perm), Some(ask_tx), Some(ask_rx))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = cli::Cli::parse();
    let cfg = config::load();
    let mut context = context::load(cli.resolve_no_context_files(&cfg));

    let default_prompt = cfg.default_prompt.as_deref().unwrap_or("code");
    if let Some(content) = context.prompts.get(default_prompt) {
        context.current_prompt = Some(content.clone());
        context.current_prompt_name = Some(default_prompt.to_string());
    }

    let provider = cli.resolve_provider(&cfg);
    let model = cli.resolve_model(&cfg);

    let mut session = session::Session::new(&provider, &model, cfg.resolve_context_window());

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
    )?;

    let (permission, ask_tx, ask_rx) = build_permission_checker(&cli, &cfg);

    if let Some(perm) = &permission {
        let allowlist: Vec<(String, String)> = session
            .permission_allowlist
            .iter()
            .map(|e| (e.tool.clone(), e.pattern.clone()))
            .collect();
        perm.lock().unwrap().load_session_allowlist(&allowlist);
    }

    let completion_model = client.completion_model(model.to_string());

    if cli.print {
        let agent =
            provider::build_agent(completion_model, &cli, &cfg, &context, permission, ask_tx);
        let msg = cli.message.join(" ");
        let response = agent.run_print(&msg).await?;
        if !cli.no_session {
            session.add_message(MessageRole::User, &msg);
            session.add_message(MessageRole::Assistant, &response);
            session::storage::save_session(&session)?;
        }
    } else {
        #[cfg(feature = "loop")]
        if cli.loop_mode {
            let model = client.completion_model(model.to_string());
            let agent = provider::build_agent(model, &cli, &cfg, &context, permission, ask_tx);
            return run_headless_loop(agent, &cli, &cfg, &context).await;
        }

        let agent = provider::build_agent(
            completion_model,
            &cli,
            &cfg,
            &context,
            permission.clone(),
            ask_tx.clone(),
        );

        if !cli.resolve_no_tools(&cfg)
            && let Some(perm) = &permission {
                let mode = resolve_mode(&cli, &cfg);
                perm.lock().unwrap().set_mode(mode);
            }

        let initial_msg = cli.message.join(" ");
        if !initial_msg.is_empty() {
            session.add_message(MessageRole::User, &initial_msg);
        }
        ui::run_interactive(
            client,
            agent,
            &cli,
            &cfg,
            &mut session,
            &mut context,
            permission,
            ask_tx,
            ask_rx,
        )
        .await?;
    }

    Ok(())
}

#[cfg(feature = "loop")]
async fn run_headless_loop(
    agent: crate::provider::AnyAgent,
    cli: &cli::Cli,
    _cfg: &config::Config,
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

        let response = match agent.run_print(&iteration_prompt).await {
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
            match tokio::process::Command::new("sh")
                .arg("-c")
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

        let _ = loop_mod::transcript::save_iteration(
            &session_id,
            state.iteration,
            &iteration_prompt,
            &response,
            validation_output.as_deref(),
            &summary,
        );

        eprintln!("--- iteration {} complete, looping ---\n", state.iteration);
    }

    Ok(())
}
