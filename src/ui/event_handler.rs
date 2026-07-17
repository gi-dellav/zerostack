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
use crate::session::storage::save_session;
use crate::session::{MessageRole, Session};
use crate::ui::events::sanitize_output;
use crate::ui::feed::BlockStyle;
use crate::ui::renderer::Renderer;
use crate::ui::slash::handle_compress;

#[cfg(any(feature = "git-worktree", feature = "loop"))]
use super::C_AGENT;
#[cfg(feature = "git-worktree")]
use super::apply_current_prompt_mode;
use super::{C_ERROR, C_TOOL};

#[cfg(feature = "mcp")]
#[allow(clippy::too_many_arguments)]
pub async fn ensure_agent(
    agent: &mut Option<AnyAgent>,
    client: &AnyClient,
    session: &mut Session,
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
    let temperature = crate::config::resolve_temperature(cli, cfg, &session.model);
    let extra_body = crate::config::resolve_extra_body(cfg, &session.model);
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
            temperature,
            extra_body,
            mcp_manager,
        )
        .await,
    );
    // Keep the pre-calibration context estimate in sync with the preamble we
    // just built (system prompt + tools + context files).
    session.overhead_tokens = crate::agent::builder::estimate_overhead(context, reasoning_enabled);
}

#[cfg(not(feature = "mcp"))]
#[allow(clippy::too_many_arguments)]
pub async fn ensure_agent(
    agent: &mut Option<AnyAgent>,
    client: &AnyClient,
    session: &mut Session,
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
    let temperature = crate::config::resolve_temperature(cli, cfg, &session.model);
    let extra_body = crate::config::resolve_extra_body(cfg, &session.model);
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
            temperature,
            extra_body,
        )
        .await,
    );
    // Keep the pre-calibration context estimate in sync with the preamble we
    // just built (system prompt + tools + context files).
    session.overhead_tokens = crate::agent::builder::estimate_overhead(context, reasoning_enabled);
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
    response_start_block: &mut Option<usize>,
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
    #[cfg(feature = "git-worktree")] wt_return_path: &mut Option<(String, String, String, bool)>,
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
                *response_start_block = None;
            }
            let safe = sanitize_output(&text);
            response_buf.push_str(&safe);

            if response_buf.is_empty() {
                return Ok(());
            }

            // Throttle markdown re-parsing: only re-parse when the buffer
            // ends with a newline (markdown structure changes at line
            // boundaries) or when the buffer is small. This avoids O(n²)
            // re-parsing on every token. The final parse happens in
            // handle_agent_done.
            if response_buf.len() >= 200 && !response_buf.ends_with('\n') {
                return Ok(());
            }

            if response_start_block.is_none() {
                renderer.feed_mut().push_block(BlockStyle::Agent, "");
                *response_start_block = Some(renderer.feed().block_count() - 1);
            }
            renderer
                .feed_mut()
                .replace_last(BlockStyle::Agent, response_buf.as_str());
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
            *response_start_block = None;
            session.add_tool_call(&name, &args);
            save_session_if_enabled(session, cli, renderer)?;
            let line = format!(
                "◈ {}",
                crate::ui::utils::format_tool_call_summary(&name, &args)
            );
            renderer.write_line(&sanitize_output(&line), C_TOOL)?;
        }
        #[cfg(any(feature = "subagents", feature = "acp"))]
        AgentEvent::SubagentToolCall { name, args } => {
            session.add_subagent_tool_call(&name, &args);
            save_session_if_enabled(session, cli, renderer)?;
            let line = format!(
                "⌥ {}",
                crate::ui::utils::format_tool_call_summary(&name, &args)
            );
            renderer.write_line(&sanitize_output(&line), C_TOOL)?;
        }
        AgentEvent::ToolResult { name, output } => {
            session.add_tool_result(&name, &output);
            save_session_if_enabled(session, cli, renderer)?;
            if name == "todo_write" {
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
            cached_input_tokens,
            cache_creation_input_tokens,
        } => {
            handle_agent_done(
                response,
                input_tokens,
                output_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
                renderer,
                session,
                cfg,
                cli,
                context,
                is_running,
                agent_rx,
                agent_line_started,
                response_buf,
                response_start_block,
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
        AgentEvent::CompletionCall {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
        } => {
            // Real provider-reported usage for the call that just finished.
            // The local len()/4 heuristic in session.total_estimated_tokens
            // undercounts code-heavy turns; trust the real number as a floor
            // so the status bar's x/y/% reflects what the provider actually saw.
            // Use the cache-inclusive prompt size so Anthropic cache hits (which
            // report input_tokens excluding cached tokens) don't deflate it.
            let real = Session::real_input_tokens(
                cfg.is_anthropic_native(&session.provider),
                input_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
            )
            .saturating_add(output_tokens);
            if real > session.total_estimated_tokens {
                session.total_estimated_tokens = real;
            }
            // Accumulate cost for intermediate calls (tool-use turns). The Done
            // event only carries the final call's usage, so without this every
            // tool-call round-trip would go uncosted.
            session.total_input_tokens = session.total_input_tokens.saturating_add(input_tokens);
            session.total_output_tokens = session.total_output_tokens.saturating_add(output_tokens);
            session.total_cost += crate::pricing::estimate_cost(
                crate::pricing::billable_input_tokens(
                    cfg.is_anthropic_native(&session.provider),
                    input_tokens,
                    cached_input_tokens,
                    cache_creation_input_tokens,
                ),
                output_tokens,
                session.input_token_cost,
                session.output_token_cost,
            );
        }
        AgentEvent::Retrying { attempt, max } => {
            *was_reasoning = false;
            if *agent_line_started {
                renderer.write_line("", Color::White)?;
                *agent_line_started = false;
            }
            response_buf.clear();
            *response_start_block = None;
            renderer.write_line(&format!("retrying... ({}/{})", attempt, max), Color::Yellow)?;
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
            *response_start_block = None;
            save_session_if_enabled(session, cli, renderer)?;
        }
    }
    Ok(())
}

fn save_session_if_enabled(
    session: &Session,
    cli: &Cli,
    renderer: &mut Renderer,
) -> anyhow::Result<()> {
    if !cli.no_session
        && let Err(e) = save_session(session)
    {
        renderer.write_line(&format!("warning: failed to save session: {}", e), C_ERROR)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_agent_done(
    response: CompactString,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_input_tokens: u64,
    renderer: &mut Renderer,
    session: &mut Session,
    cfg: &Config,
    cli: &Cli,
    context: &mut ContextFiles,
    is_running: &mut bool,
    agent_rx: &mut Option<mpsc::Receiver<AgentEvent>>,
    agent_line_started: &mut bool,
    response_buf: &mut String,
    response_start_block: &mut Option<usize>,
    was_reasoning: &mut bool,
    agent: &mut Option<AnyAgent>,
    client: &mut AnyClient,
    _loop_label: &mut Option<String>,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    sandbox: &Sandbox,
    status_signals: &Option<StatusSignals>,
    #[cfg(feature = "loop")] loop_state: &mut Option<crate::extras::r#loop::LoopState>,
    #[cfg(feature = "git-worktree")] wt_return_path: &mut Option<(String, String, String, bool)>,
    #[cfg(feature = "mcp")] mcp_manager: Option<&crate::extras::mcp::McpClientManager>,
) -> anyhow::Result<()> {
    *was_reasoning = false;

    if !response_buf.is_empty() {
        if let Some(start) = *response_start_block {
            renderer.feed_mut().truncate_blocks(start + 1);
        }
        renderer
            .feed_mut()
            .replace_last(BlockStyle::Agent, response_buf.as_str());
        renderer.render_viewport()?;
    } else if !*agent_line_started {
        renderer.feed_mut().push_line(BlockStyle::Agent, "< ");
    }

    renderer.write_line("", Color::White)?;
    renderer.write_line("", Color::White)?;
    session.add_message(MessageRole::Assistant, &response);
    // `total_input_tokens`/`total_output_tokens` keep the raw provider-reported
    // counts (that's what those fields mean), but cost prices the *billable*
    // input — for Anthropic that folds in cache reads/writes, which the raw
    // `input_tokens` excludes yet are still billed (see `billable_input_tokens`).
    session.total_input_tokens = session.total_input_tokens.saturating_add(input_tokens);
    session.total_output_tokens = session.total_output_tokens.saturating_add(output_tokens);
    session.total_cost += crate::pricing::estimate_cost(
        crate::pricing::billable_input_tokens(
            cfg.is_anthropic_native(&session.provider),
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
        ),
        output_tokens,
        session.input_token_cost,
        session.output_token_cost,
    );
    // Anchor context-size accounting to the provider's real usage. Context
    // measurement needs the full prompt size, so use the cache-inclusive count
    // (Anthropic reports input_tokens excluding cached/cache-creation tokens,
    // which would otherwise collapse the context meter to ~0 on cache hits).
    // Must come after add_message so the anchor includes the just-appended response.
    let context_input_tokens = Session::real_input_tokens(
        cfg.is_anthropic_native(&session.provider),
        input_tokens,
        cached_input_tokens,
        cache_creation_input_tokens,
    );
    session.set_calibration(context_input_tokens, output_tokens);
    *agent_line_started = false;
    response_buf.clear();
    *response_start_block = None;

    #[cfg(feature = "loop")]
    let loop_running = loop_state.as_ref().is_some_and(|ls| ls.active);
    #[cfg(not(feature = "loop"))]
    let loop_running = false;

    let qm = crate::config::quick_models_map(cfg);

    #[cfg(feature = "memory")]
    let reserve = crate::extras::memory::effective_reserve(
        cfg.resolve_reserve_tokens(&session.model, &qm),
        context.memory.as_deref(),
    );
    #[cfg(not(feature = "memory"))]
    let reserve = cfg.resolve_reserve_tokens(&session.model, &qm);

    if !loop_running
        && cfg.resolve_compact_enabled()
        && session.needs_compaction(reserve)
        && !cli.no_session
    {
        let compress_result = handle_compress(
            None,
            true,
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
        && let Err(e) = save_session(session)
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
        let summary: String = response
            .chars()
            .take(crate::extras::r#loop::SUMMARY_TRUNCATION_CHARS)
            .collect();
        ls.last_summary = Some(summary.clone());

        let validation_output = if let Some(cmd) = &ls.run_cmd {
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
                    Some(combined)
                }
                Err(e) => {
                    let msg = format!("error: {}", e);
                    Some(msg)
                }
            }
        } else {
            None
        };
        ls.last_run_output = validation_output.clone();

        let _ = crate::extras::r#loop::transcript::save_iteration(
            &session.id,
            ls.iteration,
            &ls.build_prompt(),
            &response,
            validation_output.as_deref(),
            &summary,
        );

        ls.iteration += 1;

        if ls.should_stop() {
            renderer.write_line(
                &format!(
                    "[loop] max iterations ({}) reached, stopping",
                    ls.iteration - 1
                ),
                C_AGENT,
            )?;
            ls.active = false;
            *_loop_label = None;
        } else {
            let prompt = ls.build_prompt();
            *agent = Some({
                let model = client.completion_model(session.model.to_string());
                let temperature = crate::config::resolve_temperature(cli, cfg, &session.model);
                let extra_body = crate::config::resolve_extra_body(cfg, &session.model);
                crate::provider::build_agent(
                    model,
                    cli,
                    cfg,
                    context,
                    permission.clone(),
                    ask_tx.clone(),
                    sandbox.clone(),
                    true,
                    temperature,
                    extra_body,
                    #[cfg(feature = "mcp")]
                    mcp_manager,
                )
                .await
            });
            let runner = agent
                .as_ref()
                .unwrap()
                .clone()
                .spawn_runner(
                    prompt,
                    Vec::new(),
                    cfg.retry.clone(),
                    #[cfg(feature = "hooks")]
                    Some(crate::extras::hooks::LoopInfo {
                        iteration: ls.iteration,
                        active: ls.active,
                    }),
                )
                .await;
            *agent_rx = Some(runner.event_rx);
            *is_running = true;
            if let Some(ss) = status_signals.as_ref() {
                ss.send_start();
            }
            *_loop_label = Some(ls.iteration_label());
            renderer.write_line(
                &format!("[loop] launching {}", ls.iteration_label()),
                C_AGENT,
            )?;
        }
    }

    #[cfg(feature = "git-worktree")]
    if let Some((main_path, wt_path, branch, force)) = wt_return_path.take() {
        crate::extras::git_worktree::cleanup_worktree(&wt_path, &branch, &main_path, force);
        match std::env::set_current_dir(&main_path) {
            Ok(()) => {
                session.working_dir = compact_str::CompactString::new(&main_path);
                context.reload();
                apply_current_prompt_mode(context, permission);
                *agent = Some({
                    let model = client.completion_model(session.model.to_string());
                    let temperature = crate::config::resolve_temperature(cli, cfg, &session.model);
                    let extra_body = crate::config::resolve_extra_body(cfg, &session.model);
                    crate::provider::build_agent(
                        model,
                        cli,
                        cfg,
                        context,
                        permission.clone(),
                        ask_tx.clone(),
                        sandbox.clone(),
                        true,
                        temperature,
                        extra_body,
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
