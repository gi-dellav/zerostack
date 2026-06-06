use compact_str::CompactString;
use crossterm::style::Color;
use tokio::sync::mpsc;

use crate::agent::tools::todo::TODO_LIST;
use crate::cli::Cli;
use crate::config::{Config, ResolvedShowToolDetails};
use crate::context::ContextFiles;
use crate::event::AgentEvent;
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::extras::status_signals::StatusSignals;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
use crate::sandbox::Sandbox;
use crate::session::{MessageRole, Session};
use crate::ui::events::sanitize_output;
use crate::ui::renderer::Renderer;
use crate::ui::slash::handle_compress;

use super::{C_AGENT, C_ERROR, C_TOOL, apply_current_prompt_mode};

#[cfg(feature = "mcp")]
#[allow(clippy::too_many_arguments)]
pub async fn ensure_agent(
    agent: &mut Option<AnyAgent>,
    client: &AnyClient,
    session: &Session,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    sandbox: &Sandbox,
    reasoning_enabled: bool,
    mcp_manager: Option<&McpClientManager>,
) {
    if agent.is_some() {
        return;
    }
    let model = client.completion_model(session.model.to_string());
    *agent = Some(
        crate::provider::build_agent(
            model,
            cli,
            cfg,
            context,
            permission.clone(),
            ask_tx.clone(),
            sandbox.clone(),
            reasoning_enabled,
            mcp_manager,
        )
        .await,
    );
}

#[cfg(not(feature = "mcp"))]
#[allow(clippy::too_many_arguments)]
pub async fn ensure_agent(
    agent: &mut Option<AnyAgent>,
    client: &AnyClient,
    session: &Session,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    sandbox: &Sandbox,
    reasoning_enabled: bool,
) {
    if agent.is_some() {
        return;
    }
    let model = client.completion_model(session.model.to_string());
    *agent = Some(
        crate::provider::build_agent(
            model,
            cli,
            cfg,
            context,
            permission.clone(),
            ask_tx.clone(),
            sandbox.clone(),
            reasoning_enabled,
        )
        .await,
    );
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_agent_event(
    event: AgentEvent,
    renderer: &mut Renderer,
    session: &mut Session,
    cfg: &Config,
    cli: &Cli,
    context: &mut ContextFiles,
    is_running: &mut bool,
    agent_rx: &mut Option<mpsc::Receiver<AgentEvent>>,
    agent_line_started: &mut bool,
    response_buf: &mut String,
    response_start_line: &mut Option<usize>,
    was_reasoning: &mut bool,
    show_reasoning: bool,
    agent: &mut Option<AnyAgent>,
    client: &mut AnyClient,
    loop_label: &mut Option<String>,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    sandbox: &Sandbox,
    status_signals: &Option<StatusSignals>,
    #[cfg(feature = "loop")] loop_state: &mut Option<crate::extras::r#loop::LoopState>,
    #[cfg(feature = "git-worktree")] wt_return_path: &mut Option<String>,
    #[cfg(feature = "mcp")] mcp_manager: Option<&crate::extras::mcp::McpClientManager>,
) -> anyhow::Result<()> {
    match event {
        AgentEvent::Reasoning(text) => {
            if !show_reasoning {
                return Ok(());
            }
            if !*agent_line_started {
                renderer.write("< ", Color::DarkMagenta)?;
                *agent_line_started = true;
            }
            let safe = sanitize_output(&text);
            renderer.write(&safe, Color::DarkMagenta)?;
            *was_reasoning = true;
        }
        AgentEvent::Token(text) => {
            if *was_reasoning {
                renderer.write_line("", Color::White)?;
                *agent_line_started = false;
                *was_reasoning = false;
                response_buf.clear();
                *response_start_line = None;
            }
            let safe = sanitize_output(&text);
            response_buf.push_str(&safe);

            if response_buf.is_empty() {
                return Ok(());
            }

            let max_width = renderer.line_width();
            let mut styled = crate::ui::markdown::markdown_to_styled(response_buf, max_width);

            if !styled.is_empty() {
                styled[0].text = CompactString::from(format!("< {}", styled[0].text));
            }

            if let Some(start) = *response_start_line {
                renderer.replace_from(start, styled);
            } else {
                let start = renderer.buffer_len();
                *response_start_line = Some(start);
                renderer.replace_from(start, styled);
            }
            renderer.render_viewport()?;
            *agent_line_started = true;
        }
        AgentEvent::ToolCall { name, args } => {
            *was_reasoning = false;
            if *agent_line_started {
                renderer.write_line("", Color::White)?;
                *agent_line_started = false;
            }
            response_buf.clear();
            *response_start_line = None;
            let line = format!(
                "◈ {}",
                crate::ui::utils::format_tool_call_summary(&name, &args)
            );
            renderer.write_line(&sanitize_output(&line), C_TOOL)?;
        }
        AgentEvent::SubagentToolCall { name, args } => {
            let line = format!(
                "⌥ {}",
                crate::ui::utils::format_tool_call_summary(&name, &args)
            );
            renderer.write_line(&sanitize_output(&line), C_TOOL)?;
        }
        AgentEvent::ToolResult { name, output } => {
            if name == "write_todo_list" {
                let list = TODO_LIST.lock().unwrap_or_else(|e| e.into_inner());
                if list.is_empty() {
                    renderer.write_line("tasks cleared", Color::DarkGrey)?;
                } else {
                    let total = list.len();
                    let completed = list.iter().filter(|t| t.status == "completed").count();
                    renderer.write_line(
                        &format!("tasks  {} done / {} total", completed, total),
                        C_TOOL,
                    )?;
                    for item in list.iter() {
                        let icon = match item.status.as_str() {
                            "completed" => "[x]",
                            "in_progress" => "[>]",
                            "cancelled" => "[-]",
                            _ => "[ ]",
                        };
                        let status_color = match item.status.as_str() {
                            "completed" => Color::Green,
                            "in_progress" => C_TOOL,
                            "cancelled" => Color::DarkGrey,
                            _ => Color::DarkGrey,
                        };
                        let priority_mark = match item.priority.as_str() {
                            "high" => "!!",
                            "medium" => "! ",
                            _ => "  ",
                        };
                        renderer.write_line(
                            &format!("  {} {} {}", icon, priority_mark, item.content),
                            status_color,
                        )?;
                    }
                }
            } else {
                let show_details = cfg
                    .show_tool_details
                    .as_ref()
                    .map(|s| s.resolve())
                    .unwrap_or(ResolvedShowToolDetails::Limited(3));
                match show_details {
                    ResolvedShowToolDetails::Off => {}
                    ResolvedShowToolDetails::Limited(max_lines) => {
                        let sanitized = sanitize_output(&output);
                        let char_count = sanitized.chars().count();
                        let lines: Vec<&str> = sanitized.lines().collect();
                        if lines.len() > max_lines {
                            let shown = lines[..max_lines].join("\n");
                            let summary = format!(
                                "◈ result ({} chars, {} lines, showing {}):\n{}",
                                char_count,
                                lines.len(),
                                max_lines,
                                shown
                            );
                            renderer.write_line(&summary, Color::DarkGrey)?;
                        } else {
                            let summary =
                                format!("◈ result ({} chars):\n{}", char_count, sanitized);
                            renderer.write_line(&summary, Color::DarkGrey)?;
                        }
                    }
                    ResolvedShowToolDetails::Unlimited => {
                        let sanitized = sanitize_output(&output);
                        let char_count = sanitized.chars().count();
                        let summary = format!("◈ result ({} chars):\n{}", char_count, sanitized);
                        renderer.write_line(&summary, Color::DarkGrey)?;
                    }
                }
            }
        }
        AgentEvent::Done {
            response,
            input_tokens,
            output_tokens,
        } => {
            handle_agent_done(
                response,
                input_tokens,
                output_tokens,
                renderer,
                session,
                cfg,
                cli,
                context,
                is_running,
                agent_rx,
                agent_line_started,
                response_buf,
                response_start_line,
                was_reasoning,
                agent,
                client,
                loop_label,
                permission,
                ask_tx,
                sandbox,
                status_signals,
                #[cfg(feature = "loop")]
                loop_state,
                #[cfg(feature = "git-worktree")]
                wt_return_path,
                #[cfg(feature = "mcp")]
                mcp_manager,
            )
            .await?;
        }
        AgentEvent::Error(e) => {
            *was_reasoning = false;
            let safe = sanitize_output(&e);
            renderer.write_line(&format!("error: {}", safe), C_ERROR)?;
            *is_running = false;
            if let Some(ss) = status_signals.as_ref() {
                ss.send_stop();
            }
            *agent_rx = None;
            *agent_line_started = false;
            response_buf.clear();
            *response_start_line = None;
        }
        #[cfg(feature = "advisor")]
        AgentEvent::AdviserConsulting => {
            *was_reasoning = false;
            if *agent_line_started {
                renderer.write_line("", Color::White)?;
                *agent_line_started = false;
            }
            response_buf.clear();
            *response_start_line = None;
            renderer.write_line("⬢ consulting adviser...", C_TOOL)?;
        }
        #[cfg(feature = "advisor")]
        AgentEvent::AdviserResult { text } => {
            renderer.write_line(&sanitize_output(&text), Color::DarkMagenta)?;
        }
        #[cfg(feature = "advisor")]
        AgentEvent::AdviserError { error } => {
            renderer.write_line(
                &format!("adviser error: {}", sanitize_output(&error)),
                C_ERROR,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_agent_done(
    response: CompactString,
    input_tokens: u64,
    output_tokens: u64,
    renderer: &mut Renderer,
    session: &mut Session,
    cfg: &Config,
    cli: &Cli,
    context: &mut ContextFiles,
    is_running: &mut bool,
    agent_rx: &mut Option<mpsc::Receiver<AgentEvent>>,
    agent_line_started: &mut bool,
    response_buf: &mut String,
    response_start_line: &mut Option<usize>,
    was_reasoning: &mut bool,
    agent: &mut Option<AnyAgent>,
    client: &mut AnyClient,
    loop_label: &mut Option<String>,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    sandbox: &Sandbox,
    status_signals: &Option<StatusSignals>,
    #[cfg(feature = "loop")] loop_state: &mut Option<crate::extras::r#loop::LoopState>,
    #[cfg(feature = "git-worktree")] wt_return_path: &mut Option<String>,
    #[cfg(feature = "mcp")] mcp_manager: Option<&crate::extras::mcp::McpClientManager>,
) -> anyhow::Result<()> {
    *was_reasoning = false;

    if !response_buf.is_empty() {
        let max_width = renderer.line_width();
        let mut styled = crate::ui::markdown::markdown_to_styled(response_buf, max_width);
        if !styled.is_empty() {
            styled[0].text = CompactString::from(format!("< {}", styled[0].text));
        }
        if let Some(start) = *response_start_line {
            renderer.replace_from(start, styled);
            renderer.render_viewport()?;
        }
    } else if !*agent_line_started {
        renderer.write("< ", C_AGENT)?;
    }

    renderer.write_line("", Color::White)?;
    renderer.write_line("", Color::White)?;
    session.add_message(MessageRole::Assistant, &response);
    #[cfg(feature = "advisor")]
    crate::extras::adviser::update_session_snapshot(&session.messages);
    session.total_input_tokens = session.total_input_tokens.saturating_add(input_tokens);
    session.total_output_tokens = session.total_output_tokens.saturating_add(output_tokens);
    session.total_cost += crate::pricing::estimate_cost(
        input_tokens,
        output_tokens,
        session.input_token_cost,
        session.output_token_cost,
    );
    *agent_line_started = false;
    response_buf.clear();
    *response_start_line = None;

    #[cfg(feature = "loop")]
    let loop_running = loop_state.as_ref().is_some_and(|ls| ls.active);
    #[cfg(not(feature = "loop"))]
    let loop_running = false;

    #[cfg(feature = "memory")]
    let reserve = crate::extras::memory::effective_reserve(
        cfg.resolve_reserve_tokens(),
        context.memory.as_deref(),
    );
    #[cfg(not(feature = "memory"))]
    let reserve = cfg.resolve_reserve_tokens();

    if !loop_running
        && cfg.resolve_compact_enabled()
        && session.needs_compaction(reserve)
        && !cli.no_session
    {
        renderer.write_line("auto-compacting...", Color::DarkGrey)?;
        let compress_result = handle_compress(
            None,
            agent,
            client,
            renderer,
            session,
            cli,
            cfg,
            context,
            true,
            permission,
            ask_tx,
            sandbox,
            #[cfg(feature = "mcp")]
            mcp_manager,
        )
        .await;
        if let Err(e) = compress_result {
            renderer.write_line(&format!("auto-compact error: {}", e), C_ERROR)?;
        }
    }

    if !cli.no_session
        && let Err(e) = crate::session::storage::save_session(session)
    {
        renderer.write_line(&format!("warning: failed to save session: {}", e), C_ERROR)?;
    }
    *is_running = false;
    if let Some(ss) = status_signals.as_ref() {
        ss.send_stop();
    }
    *agent_rx = None;

    #[cfg(feature = "loop")]
    if let Some(ls) = loop_state
        && ls.active
    {
        if ls.should_stop() {
            renderer.write_line(
                &format!("[loop] max iterations ({}) reached, stopping", ls.iteration),
                C_AGENT,
            )?;
            ls.active = false;
            *loop_label = None;
        } else {
            let summary: String = response.chars().take(200).collect();
            ls.last_summary = Some(summary);
            ls.iteration += 1;
            let prompt = ls.build_prompt();
            *agent = Some({
                let model = client.completion_model(session.model.to_string());
                crate::provider::build_agent(
                    model,
                    cli,
                    cfg,
                    context,
                    permission.clone(),
                    ask_tx.clone(),
                    sandbox.clone(),
                    true,
                    #[cfg(feature = "mcp")]
                    mcp_manager,
                )
                .await
            });
            let runner = agent
                .as_ref()
                .unwrap()
                .clone()
                .spawn_runner(prompt, Vec::new());
            *agent_rx = Some(runner.event_rx);
            *is_running = true;
            if let Some(ss) = status_signals.as_ref() {
                ss.send_start();
            }
            *loop_label = Some(ls.iteration_label());
            renderer.write_line(
                &format!("[loop] launching {}", ls.iteration_label()),
                C_AGENT,
            )?;
        }
    }

    // Drop the agent after each response – it will be rebuilt on the next user input.
    // The git-worktree path below (wt_return_path) may reassign it, which is fine.
    *agent = None;

    #[cfg(feature = "git-worktree")]
    if let Some(main_path) = wt_return_path.take() {
        match std::env::set_current_dir(&main_path) {
            Ok(()) => {
                session.working_dir = compact_str::CompactString::new(&main_path);
                context.reload();
                apply_current_prompt_mode(context, permission);
                *agent = Some({
                    let model = client.completion_model(session.model.to_string());
                    crate::provider::build_agent(
                        model,
                        cli,
                        cfg,
                        context,
                        permission.clone(),
                        ask_tx.clone(),
                        sandbox.clone(),
                        true,
                        #[cfg(feature = "mcp")]
                        mcp_manager,
                    )
                    .await
                });
                crate::ui::events::render_session(renderer, session, cli, cfg, context)?;
                renderer.write_line(
                    &format!("merged and returned to main repo at {}", main_path),
                    C_AGENT,
                )?;
            }
            Err(e) => {
                renderer.write_line(
                    &format!("warning: failed to change back to main repo: {}", e),
                    C_ERROR,
                )?;
            }
        }
    }

    Ok(())
}
