use compact_str::CompactString;
use crossterm::style::Color;

use crate::agent::tools::todo::TODO_LIST;
use crate::cli::Cli;
use crate::config::ResolvedShowToolDetails;
use crate::event::AgentEvent;
use crate::provider::AnyAgent;
use crate::session::storage::save_session;
use crate::session::{MessageRole, Session};
use crate::ui::events::sanitize_output;
use crate::ui::feed::BlockStyle;
use crate::ui::renderer::Renderer;
use crate::ui::slash::handle_compress;
use crate::ui::state::{AgentRunState, ChainState, SlashState, TurnUsage, UiContext};

#[cfg(any(feature = "git-worktree", feature = "loop"))]
use super::C_AGENT;
#[cfg(feature = "git-worktree")]
use super::apply_current_prompt_mode;
use super::{C_ERROR, C_TOOL};

/// Build the main agent on first use, lazily connecting MCP as well. Callers
/// only reach the build when `agent` is `None`, so MCP connects at most once.
pub async fn ensure_agent(
    agent: &mut Option<AnyAgent>,
    ui: &mut UiContext<'_>,
    reasoning_enabled: bool,
) {
    if agent.is_some() {
        return;
    }
    let model = ui.client.completion_model(ui.session.model.to_string());
    let temperature = crate::config::resolve_temperature(ui.cli, ui.cfg, &ui.session.model);
    let extra_body = crate::config::resolve_extra_body(ui.cfg, &ui.session.model);
    #[cfg(feature = "mcp")]
    let mcp_ref = crate::ui::ensure_mcp_manager(&mut ui.mcp_manager, ui.cfg).await;
    *agent = Some(
        crate::provider::build_agent(
            model,
            ui.cli,
            ui.cfg,
            ui.context,
            ui.permission.clone(),
            ui.ask_tx.clone(),
            ui.sandbox.clone(),
            reasoning_enabled,
            temperature,
            extra_body,
            #[cfg(feature = "mcp")]
            mcp_ref,
        )
        .await,
    );
    // Keep the pre-calibration context estimate in sync with the preamble we
    // just built (system prompt + tools + context files).
    ui.session.overhead_tokens =
        crate::agent::builder::estimate_overhead(ui.context, reasoning_enabled);
}

pub async fn handle_agent_event(
    event: AgentEvent,
    renderer: &mut Renderer,
    run: &mut AgentRunState,
    ui: &mut UiContext<'_>,
    slash: &SlashState,
    chain: &mut ChainState,
) -> anyhow::Result<()> {
    match event {
        AgentEvent::Reasoning(text) => {
            if !slash.show_reasoning {
                return Ok(());
            }
            if !run.agent_line_started {
                renderer.write("< ", Color::DarkMagenta)?;
                run.agent_line_started = true;
            }
            let safe = sanitize_output(&text);
            renderer.write(&safe, Color::DarkMagenta)?;
            run.was_reasoning = true;
        }
        AgentEvent::Token(text) => {
            if run.was_reasoning {
                renderer.write_line("", Color::White)?;
                run.agent_line_started = false;
                run.was_reasoning = false;
                run.response_buf.clear();
                run.response_start_block = None;
            }
            let safe = sanitize_output(&text);
            run.response_buf.push_str(&safe);

            if run.response_buf.is_empty() {
                return Ok(());
            }

            // Throttle markdown re-parsing: only re-parse when the buffer
            // ends with a newline (markdown structure changes at line
            // boundaries) or when the buffer is small. This avoids O(n²)
            // re-parsing on every token. The final parse happens in
            // handle_agent_done.
            if run.response_buf.len() >= 200 && !run.response_buf.ends_with('\n') {
                return Ok(());
            }

            if run.response_start_block.is_none() {
                renderer.feed_mut().push_block(BlockStyle::Agent, "");
                run.response_start_block = Some(renderer.feed().block_count() - 1);
            }
            renderer
                .feed_mut()
                .replace_last(BlockStyle::Agent, run.response_buf.as_str());
            renderer.render_viewport()?;
            run.agent_line_started = true;
        }
        AgentEvent::ToolCall { name, args } => {
            run.was_reasoning = false;
            if run.agent_line_started {
                renderer.write_line("", Color::White)?;
                run.agent_line_started = false;
            }
            run.response_buf.clear();
            run.response_start_block = None;
            ui.session.add_tool_call(&name, &args);
            save_session_if_enabled(ui.session, ui.cli, renderer)?;
            let line = format!(
                "◈ {}",
                crate::ui::utils::format_tool_call_summary(&name, &args)
            );
            renderer.write_line(&sanitize_output(&line), C_TOOL)?;
        }
        #[cfg(any(feature = "subagents", feature = "acp"))]
        AgentEvent::SubagentToolCall { name, args } => {
            ui.session.add_subagent_tool_call(&name, &args);
            save_session_if_enabled(ui.session, ui.cli, renderer)?;
            let line = format!(
                "⌥ {}",
                crate::ui::utils::format_tool_call_summary(&name, &args)
            );
            renderer.write_line(&sanitize_output(&line), C_TOOL)?;
        }
        AgentEvent::ToolResult { name, output } => {
            ui.session.add_tool_result(&name, &output);
            save_session_if_enabled(ui.session, ui.cli, renderer)?;
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
                let show_details = ui
                    .cfg
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
                TurnUsage {
                    input_tokens,
                    output_tokens,
                    cached_input_tokens,
                    cache_creation_input_tokens,
                },
                renderer,
                run,
                ui,
                chain,
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
                ui.cfg.is_anthropic_native(&ui.session.provider),
                input_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
            )
            .saturating_add(output_tokens);
            if real > ui.session.total_estimated_tokens {
                ui.session.total_estimated_tokens = real;
            }
            // Accumulate cost for intermediate calls (tool-use turns). The Done
            // event only carries the final call's usage, so without this every
            // tool-call round-trip would go uncosted.
            ui.session.total_input_tokens =
                ui.session.total_input_tokens.saturating_add(input_tokens);
            ui.session.total_output_tokens =
                ui.session.total_output_tokens.saturating_add(output_tokens);
            ui.session.total_cost += crate::pricing::estimate_cost(
                crate::pricing::billable_input_tokens(
                    ui.cfg.is_anthropic_native(&ui.session.provider),
                    input_tokens,
                    cached_input_tokens,
                    cache_creation_input_tokens,
                ),
                output_tokens,
                ui.session.input_token_cost,
                ui.session.output_token_cost,
            );
        }
        AgentEvent::Retrying { attempt, max } => {
            run.was_reasoning = false;
            if run.agent_line_started {
                renderer.write_line("", Color::White)?;
                run.agent_line_started = false;
            }
            run.response_buf.clear();
            run.response_start_block = None;
            renderer.write_line(&format!("retrying... ({}/{})", attempt, max), Color::Yellow)?;
        }
        AgentEvent::Error(e) => {
            run.was_reasoning = false;
            let safe = sanitize_output(&e);
            renderer.write_line(&format!("error: {}", safe), C_ERROR)?;
            run.is_running = false;
            if let Some(ss) = ui.status_signals.as_ref() {
                ss.send_stop();
            }
            run.agent_rx = None;
            run.agent_line_started = false;
            run.response_buf.clear();
            run.response_start_block = None;
            save_session_if_enabled(ui.session, ui.cli, renderer)?;
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

async fn handle_agent_done(
    response: CompactString,
    usage: TurnUsage,
    renderer: &mut Renderer,
    run: &mut AgentRunState,
    ui: &mut UiContext<'_>,
    chain: &mut ChainState,
) -> anyhow::Result<()> {
    // `chain` is only read by the /loop-respawn and worktree-return paths.
    #[cfg(not(any(feature = "loop", feature = "git-worktree")))]
    let _ = &chain;
    run.was_reasoning = false;

    if !run.response_buf.is_empty() {
        if let Some(start) = run.response_start_block {
            renderer.feed_mut().truncate_blocks(start + 1);
        }
        renderer
            .feed_mut()
            .replace_last(BlockStyle::Agent, run.response_buf.as_str());
        renderer.render_viewport()?;
    } else if !run.agent_line_started {
        renderer.feed_mut().push_line(BlockStyle::Agent, "< ");
    }

    renderer.write_line("", Color::White)?;
    renderer.write_line("", Color::White)?;
    ui.session.add_message(MessageRole::Assistant, &response);
    // `total_input_tokens`/`total_output_tokens` keep the raw provider-reported
    // counts (that's what those fields mean), but cost prices the *billable*
    // input — for Anthropic that folds in cache reads/writes, which the raw
    // `input_tokens` excludes yet are still billed (see `billable_input_tokens`).
    ui.session.total_input_tokens = ui
        .session
        .total_input_tokens
        .saturating_add(usage.input_tokens);
    ui.session.total_output_tokens = ui
        .session
        .total_output_tokens
        .saturating_add(usage.output_tokens);
    ui.session.total_cost += crate::pricing::estimate_cost(
        crate::pricing::billable_input_tokens(
            ui.cfg.is_anthropic_native(&ui.session.provider),
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.cache_creation_input_tokens,
        ),
        usage.output_tokens,
        ui.session.input_token_cost,
        ui.session.output_token_cost,
    );
    // Anchor context-size accounting to the provider's real usage. Context
    // measurement needs the full prompt size, so use the cache-inclusive count
    // (Anthropic reports input_tokens excluding cached/cache-creation tokens,
    // which would otherwise collapse the context meter to ~0 on cache hits).
    // Must come after add_message so the anchor includes the just-appended response.
    let context_input_tokens = Session::real_input_tokens(
        ui.cfg.is_anthropic_native(&ui.session.provider),
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.cache_creation_input_tokens,
    );
    ui.session
        .set_calibration(context_input_tokens, usage.output_tokens);
    run.agent_line_started = false;
    run.response_buf.clear();
    run.response_start_block = None;

    #[cfg(feature = "loop")]
    let loop_running = chain.loop_state.as_ref().is_some_and(|ls| ls.active);
    #[cfg(not(feature = "loop"))]
    let loop_running = false;

    let qm = crate::config::quick_models_map(ui.cfg);

    #[cfg(feature = "memory")]
    let reserve = crate::extras::memory::effective_reserve(
        ui.cfg.resolve_reserve_tokens(&ui.session.model, &qm),
        ui.context.memory.as_deref(),
    );
    #[cfg(not(feature = "memory"))]
    let reserve = ui.cfg.resolve_reserve_tokens(&ui.session.model, &qm);

    if !loop_running
        && ui.cfg.resolve_compact_enabled()
        && ui.session.needs_compaction(reserve)
        && !ui.cli.no_session
    {
        let compress_result = handle_compress(None, true, &mut run.agent, renderer, ui, true).await;
        if let Err(e) = compress_result {
            renderer.write_line(&format!("auto-compact error: {}", e), C_ERROR)?;
        }
    }

    if !ui.cli.no_session
        && let Err(e) = save_session(ui.session)
    {
        renderer.write_line(&format!("warning: failed to save session: {}", e), C_ERROR)?;
    }
    run.is_running = false;
    if let Some(ss) = ui.status_signals.as_ref() {
        ss.send_stop();
    }
    run.agent_rx = None;

    #[cfg(feature = "loop")]
    if let Some(ls) = chain.loop_state.as_mut()
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
            &ui.session.id,
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
            chain.loop_label = None;
        } else {
            let prompt = ls.build_prompt();
            run.agent = Some({
                let model = ui.client.completion_model(ui.session.model.to_string());
                let temperature =
                    crate::config::resolve_temperature(ui.cli, ui.cfg, &ui.session.model);
                let extra_body = crate::config::resolve_extra_body(ui.cfg, &ui.session.model);
                crate::provider::build_agent(
                    model,
                    ui.cli,
                    ui.cfg,
                    ui.context,
                    ui.permission.clone(),
                    ui.ask_tx.clone(),
                    ui.sandbox.clone(),
                    true,
                    temperature,
                    extra_body,
                    #[cfg(feature = "mcp")]
                    ui.mcp_manager.as_ref(),
                )
                .await
            });
            let runner = run
                .agent
                .as_ref()
                .unwrap()
                .clone()
                .spawn_runner(
                    prompt,
                    Vec::new(),
                    ui.cfg.retry.clone(),
                    #[cfg(feature = "hooks")]
                    Some(crate::extras::hooks::LoopInfo {
                        iteration: ls.iteration,
                        active: ls.active,
                    }),
                )
                .await;
            run.agent_rx = Some(runner.event_rx);
            run.is_running = true;
            if let Some(ss) = ui.status_signals.as_ref() {
                ss.send_start();
            }
            chain.loop_label = Some(ls.iteration_label());
            renderer.write_line(
                &format!("[loop] launching {}", ls.iteration_label()),
                C_AGENT,
            )?;
        }
    }

    #[cfg(feature = "git-worktree")]
    if let Some((main_path, wt_path, branch, force)) = chain.wt_return_path.take() {
        crate::extras::git_worktree::cleanup_worktree(&wt_path, &branch, &main_path, force);
        match std::env::set_current_dir(&main_path) {
            Ok(()) => {
                ui.session.working_dir = compact_str::CompactString::new(&main_path);
                ui.context.reload();
                apply_current_prompt_mode(ui.context, &ui.permission);
                run.agent = Some({
                    let model = ui.client.completion_model(ui.session.model.to_string());
                    let temperature =
                        crate::config::resolve_temperature(ui.cli, ui.cfg, &ui.session.model);
                    let extra_body = crate::config::resolve_extra_body(ui.cfg, &ui.session.model);
                    crate::provider::build_agent(
                        model,
                        ui.cli,
                        ui.cfg,
                        ui.context,
                        ui.permission.clone(),
                        ui.ask_tx.clone(),
                        ui.sandbox.clone(),
                        true,
                        temperature,
                        extra_body,
                        #[cfg(feature = "mcp")]
                        ui.mcp_manager.as_ref(),
                    )
                    .await
                });
                crate::ui::events::render_session(
                    renderer, ui.session, ui.cli, ui.cfg, ui.context,
                )?;
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
