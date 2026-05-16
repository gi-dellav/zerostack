mod events;
mod input;
mod picker;
mod renderer;
mod slash;
mod status;
mod terminal;

use crossterm::event;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::style::Color;
use tokio::sync::mpsc;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::{AgentEvent, UserEvent};
use crate::permission::ask::{AskReceiver, AskSender, UserDecision};
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
use crate::session::{MessageRole, PermissionAllowEntry, Session};
use crate::ui::events::{render_session, sanitize_output};
use crate::ui::input::InputEditor;
use crate::ui::renderer::{Renderer, copy_to_clipboard};
use crate::ui::slash::{handle_compress, handle_slash};
use crate::ui::status::StatusLine;
use crate::ui::terminal::TerminalGuard;
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;

const C_AGENT: Color = Color::White;
const C_ERROR: Color = Color::Red;
const C_TOOL: Color = Color::Yellow;
const C_PERM: Color = Color::Magenta;

/// Formats a tool call showing only the primary file/command parameter.
/// - read/write/edit → path
/// - grep → pattern (and path if both present)
/// - find_files → pattern
/// - list_dir → path
/// - bash → command (truncated to 60 chars)
/// - others → first string arg or nothing
fn format_tool_call_summary(name: &str, args: &serde_json::Value) -> String {
    let obj = match args {
        serde_json::Value::Object(map) => map,
        _ => return name.to_string(),
    };

    // Determine which key(s) to show based on tool name
    let primary_keys: &[&str] = match name {
        "read" | "write" | "edit" | "list_dir" => &["path"],
        "grep" => &["pattern", "path"],
        "find_files" => &["pattern"],
        "bash" => &["command"],
        _ => &[],
    };

    let mut shown = Vec::new();
    for key in primary_keys {
        if let Some(serde_json::Value::String(val)) = obj.get(*key) {
            let truncated = if val.len() > 60 {
                format!("\"{}...\"", &val[..57])
            } else {
                format!("\"{}\"", val)
            };
            shown.push(truncated);
        }
    }

    if shown.is_empty() {
        // fallback: show first string value if any
        if let Some((_, serde_json::Value::String(val))) = obj.iter().next() {
            let truncated = if val.len() > 60 {
                format!("\"{}...\"", &val[..57])
            } else {
                format!("\"{}\"", val)
            };
            format!("{} {}", name, truncated)
        } else {
            name.to_string()
        }
    } else {
        format!("{} {}", name, shown.join(" "))
    }
}

pub async fn run_interactive(
    client: AnyClient,
    mut agent: AnyAgent,
    cli: &Cli,
    cfg: &Config,
    session: &mut Session,
    context: &mut ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    mut ask_rx: Option<AskReceiver>,
    #[cfg(feature = "mcp")] mcp_manager: Option<&McpClientManager>,
) -> anyhow::Result<()> {
    let _guard = TerminalGuard::new()?;

    let mut renderer = Renderer::new()?;
    let mut input = InputEditor::new();
    let mut is_running = false;
    let mut agent_rx: Option<mpsc::Receiver<AgentEvent>> = None;
    let mut agent_line_started = false;
    let mut show_reasoning = true;
    let mut was_reasoning = false;
    let mut todo_tools_enabled = false;
    #[allow(unused_mut)]
    let mut loop_label: Option<String> = None;
    #[cfg(feature = "loop")]
    let mut loop_state: Option<crate::extras::r#loop::LoopState> = None;
    #[cfg(feature = "git-worktree")]
    let mut wt_return_path: Option<String> = None;

    render_session(&mut renderer, session, cli, cfg, context)?;
    renderer.draw_bottom("", 0, &StatusLine::render(session, false, 0, None, context.current_prompt_name.as_deref()), false)?;

    let (user_tx, mut user_rx) = mpsc::channel::<UserEvent>(64);
    let user_tx_clone = user_tx.clone();
    std::thread::spawn(move || {
        loop {
            match event::read() {
                Ok(event::Event::Key(key)) => {
                    if user_tx_clone.blocking_send(UserEvent::Key(key)).is_err() {
                        break;
                    }
                }
                Ok(event::Event::Mouse(m)) => match m.kind {
                    MouseEventKind::ScrollUp => {
                        if user_tx_clone.blocking_send(UserEvent::ScrollUp).is_err() {
                            break;
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        if user_tx_clone.blocking_send(UserEvent::ScrollDown).is_err() {
                            break;
                        }
                    }
                    MouseEventKind::Down(btn) if btn == MouseButton::Left => {
                        let _ = user_tx_clone.blocking_send(UserEvent::MouseDown {
                            row: m.row,
                            col: m.column,
                        });
                    }
                    MouseEventKind::Drag(btn) if btn == MouseButton::Left => {
                        let _ = user_tx_clone.blocking_send(UserEvent::MouseDrag {
                            row: m.row,
                            col: m.column,
                        });
                    }
                    MouseEventKind::Up(btn) if btn == MouseButton::Left => {
                        let _ = user_tx_clone.blocking_send(UserEvent::MouseUp {
                            row: m.row,
                            col: m.column,
                        });
                    }
                    _ => {}
                },
                Ok(event::Event::Resize(_, _)) => {}
                Err(_) => break,
                _ => {}
            }
        }
    });

    loop {
        tokio::select! {
            Some(ev) = user_rx.recv() => {
                match ev {
                    UserEvent::ScrollUp => {
                        renderer.scroll_line_up();
                        renderer.render_viewport()?;
                        renderer.draw_bottom(
                            &input.buffer,
                            input.cursor,
                            &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                            is_running,
                        )?;
                        continue;
                    }
                    UserEvent::ScrollDown => {
                        renderer.scroll_line_down();
                        renderer.render_viewport()?;
                        renderer.draw_bottom(
                            &input.buffer,
                            input.cursor,
                            &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                            is_running,
                        )?;
                        continue;
                    }
                    UserEvent::MouseDown { row, col: _ } => {
                        if row < renderer.visible_lines() as u16
                            && let Some(idx) = renderer.buffer_line_at_row(row) {
                                renderer.selection_active = true;
                                renderer.selection_start = Some(idx);
                                renderer.selection_end = Some(idx);
                                renderer.render_viewport()?;
                                renderer.draw_bottom(
                                    &input.buffer, input.cursor,
                                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                    is_running,
                                )?;
                            }
                        continue;
                    }
                    UserEvent::MouseDrag { row, col: _ } => {
                        if renderer.selection_active
                            && let Some(idx) = renderer.buffer_line_at_row(row) {
                                renderer.selection_end = Some(idx);
                                renderer.render_viewport()?;
                                renderer.draw_bottom(
                                    &input.buffer, input.cursor,
                                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                    is_running,
                                )?;
                            }
                        continue;
                    }
                    UserEvent::MouseUp { row, col: _ } => {
                        if renderer.selection_active {
                            if let Some(idx) = renderer.buffer_line_at_row(row) {
                                renderer.selection_end = Some(idx);
                            }
                            if let Some(text) = renderer.selected_text() {
                                copy_to_clipboard(&text);
                            }
                            renderer.clear_selection();
                            renderer.render_viewport()?;
                            renderer.draw_bottom(
                                &input.buffer, input.cursor,
                                &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                is_running,
                            )?;
                        }
                        continue;
                    }
                    UserEvent::Key(key) => {
                        let is_ctrl_c = key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        if is_ctrl_c {
                            if is_running {
                                is_running = false;
                                agent_rx = None;
                                #[cfg(feature = "loop")]
                                if let Some(ref mut ls) = loop_state {
                                    ls.active = false;
                                    loop_label = None;
                                }
                                renderer.write_line("interrupted", C_ERROR)?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                    is_running,
                                )?;
                            } else {
                                break;
                            }
                            continue;
                        }

                        if renderer.selection_active && key.code == KeyCode::Char('y') {
                            if let Some(text) = renderer.selected_text() {
                                copy_to_clipboard(&text);
                                renderer.write_line("copied selection", Color::Green)?;
                            }
                            renderer.clear_selection();
                            renderer.render_viewport()?;
                            renderer.draw_bottom(
                                &input.buffer, input.cursor,
                                &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                is_running,
                            )?;
                            continue;
                        }
                        if renderer.selection_active && key.code == KeyCode::Esc {
                            renderer.clear_selection();
                            renderer.render_viewport()?;
                            renderer.draw_bottom(
                                &input.buffer, input.cursor,
                                &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                is_running,
                            )?;
                            continue;
                        }

                        let ctrl_r = key.code == KeyCode::Char('r')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        if ctrl_r {
                            show_reasoning = !show_reasoning;
                            renderer.write_line(
                                &format!("reasoning visibility: {}", if show_reasoning { "on" } else { "off" }),
                                Color::White,
                            )?;
                            renderer.draw_bottom(
                                &input.buffer,
                                input.cursor,
                                &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                is_running,
                            )?;
                            continue;
                        }

                        match key.code {
                            KeyCode::PageUp => {
                                renderer.scroll_page_up();
                                renderer.render_viewport()?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                    is_running,
                                )?;
                                continue;
                            }
                            KeyCode::PageDown => {
                                renderer.scroll_page_down();
                                renderer.render_viewport()?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                    is_running,
                                )?;
                                continue;
                            }
                            KeyCode::Home => {
                                renderer.scroll_to_top();
                                renderer.render_viewport()?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                    is_running,
                                )?;
                                continue;
                            }
                            KeyCode::End => {
                                renderer.scroll_to_bottom()?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                    is_running,
                                )?;
                                continue;
                            }
                            _ => {}
                        }

                        if input.picker.as_ref().is_some_and(|p| p.active)
                            && input.handle_picker_key(key) {
                                renderer.render_viewport()?;
                                renderer.draw_bottom(
                                    &input.buffer, input.cursor,
                                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                    is_running,
                                )?;
                                if let Some(ref picker) = input.picker {
                                    picker.draw()?;
                                }
                                continue;
                            }

                        if let Some(text) = input.handle_key(key) {
                            #[cfg(feature = "loop")]
                            if loop_state.as_ref().is_some_and(|ls| ls.active) && !text.starts_with('/') {
                                renderer.write_line("loop active: /loop stop to cancel", C_ERROR)?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                                    is_running,
                                )?;
                                continue;
                            }
                            if renderer.is_scrolling() {
                                renderer.scroll_to_bottom()?;
                            }
                            if text.starts_with('/') {
                                for line in text.lines() {
                                    let safe_line = sanitize_output(line);
                                    renderer.write_line(&format!("> {}", safe_line), Color::Green)?;
                                }
                                renderer.write_line("", Color::White)?;
                                let result = handle_slash(&text, &mut agent, &client, &mut renderer, session, cli, cfg, context, &mut show_reasoning, &mut is_running, &mut input, &permission, &ask_tx, &mut todo_tools_enabled, #[cfg(feature = "loop")] &mut loop_state, #[cfg(feature = "mcp")] mcp_manager).await;
                                match result {
                                Err(e) if e.to_string().starts_with("DEFER_COMPRESS:") => {
                                    let err_msg = e.to_string();
                                    let instructions = err_msg.strip_prefix("DEFER_COMPRESS:").and_then(|s| {
                                        let s = s.trim();
                                        if s.is_empty() || s == "(none)" { None } else { Some(s.to_string()) }
                                    });
                                        let compress_result = handle_compress(
                                            instructions.as_deref(),
                                            &mut agent, &client, &mut renderer, session, cli, cfg, context,
                                            &permission, &ask_tx,
                                            #[cfg(feature = "mcp")] mcp_manager,
                                        ).await;
                                        if let Err(e) = compress_result {
                                            renderer.write_line(&format!("compress error: {}", e), C_ERROR)?;
                                        }
                                        let _ = crate::session::storage::save_session(session);
                                    }
                                    #[cfg(feature = "git-worktree")]
                                    Err(e) if e.to_string().starts_with("DEFER_WT_MERGE:") => {
                                        let err_msg = e.to_string();
                                        let parts: Vec<&str> = err_msg.strip_prefix("DEFER_WT_MERGE:").unwrap_or("").splitn(5, ':').collect();
                                        if parts.len() == 5 {
                                            let branch = parts[0];
                                            let target = parts[1];
                                            let main_path = parts[2].to_string();
                                            let wt_path = parts[3];
                                            let _repo_name = parts[4];
                                            let prompt = format!(
                                                "I'm in a git worktree on branch '{}' at '{}'. \
                                                 Please merge branch '{}' into '{}' in the main repo at '{}', \
                                                 push the changes, and delete the worktree at '{}'. \
                                                 After merging, go back to the main repo at '{}'.",
                                                branch, wt_path, branch, target, main_path, wt_path, main_path
                                            );
                                            session.add_message(MessageRole::User, &prompt);
                                            let history = crate::agent::runner::convert_history(session);
                                            let runner = agent.clone().spawn_runner(prompt, history);
                                            agent_rx = Some(runner.event_rx);
                                            is_running = true;
                                            wt_return_path = Some(main_path);
                                        }
                                    }
                                    #[cfg(feature = "git-worktree")]
                                    Err(e) if e.to_string().starts_with("DEFER_WT_EXIT:") => {
                                        let err_msg = e.to_string();
                                        let parts: Vec<&str> = err_msg.strip_prefix("DEFER_WT_EXIT:").unwrap_or("").splitn(2, ':').collect();
                                        if parts.len() == 2 {
                                            let main_path = parts[0];
                                            std::env::set_current_dir(main_path)
                                                .map_err(|e| anyhow::anyhow!("failed to change directory: {}", e))?;
                                            session.working_dir = compact_str::CompactString::new(main_path);
                                            context.reload();
                                            let model = client.completion_model(session.model.to_string());
                                            agent = crate::provider::build_agent(
                                                model,
                                                cli,
                                                cfg,
                                                context,
                                                permission.clone(),
                                                ask_tx.clone(),
                                                #[cfg(feature = "mcp")] mcp_manager,
                                            ).await;
                                            render_session(&mut renderer, session, cli, cfg, context)?;
                                            renderer.write_line(
                                                &format!("returned to main repo at {}", main_path),
                                                C_AGENT,
                                            )?;
                                        }
                                    }
                                    Err(e) => {
                                        if e.downcast_ref::<std::io::Error>().is_some_and(|e: &std::io::Error| e.kind() == std::io::ErrorKind::Interrupted) {
                                            break;
                                        }
                                        renderer.write_line(&format!("error: {}", e), C_ERROR)?;
                                    }
                                    Ok(_) => {
                                        let _ = crate::session::storage::save_session(session);
                                        #[cfg(feature = "loop")]
                                        if let Some(ref mut ls) = loop_state
                                            && ls.active && ls.iteration == 0 && !is_running
                                        {
                                            ls.iteration = 1;
                                            let prompt = ls.build_prompt();
                                            let runner = agent.clone().spawn_runner(prompt, Vec::new());
                                            agent_rx = Some(runner.event_rx);
                                            is_running = true;
                                            loop_label = Some(ls.iteration_label());
                                        }
                                    }
                                }
                                if !cli.no_session {
                                    let _ = crate::session::storage::save_session(session);
                                }
                            } else {
                                for line in text.lines() {
                                    let safe_line = sanitize_output(line);
                                    renderer.write_line(&format!("> {}", safe_line), Color::Green)?;
                                }
                                renderer.write_line("", Color::White)?;

                                let history = crate::agent::runner::convert_history(session);
                                let runner = agent.clone().spawn_runner(
                                    text.to_string(),
                                    history,
                                );
                                agent_rx = Some(runner.event_rx);
                                is_running = true;

                                session.add_message(MessageRole::User, &text);
                            }
                        }
                        renderer.draw_bottom(
                            &input.buffer,
                            input.cursor,
                            &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                            is_running,
                        )?;
                        if let Some(ref picker) = input.picker {
                            picker.draw()?;
                        }
                    }
                }
            }
            Some(event) = async {
                if let Some(rx) = &mut agent_rx {
                    rx.recv().await
                } else {
                    std::future::pending().await
                }
            } => {
                match event {
                    AgentEvent::Reasoning(text) => {
                        if !show_reasoning {
                            continue;
                        }
                        if !agent_line_started {
                            renderer.write("< ", Color::DarkMagenta)?;
                            agent_line_started = true;
                        }
                        let safe = sanitize_output(&text);
                        renderer.write(&safe, Color::DarkMagenta)?;
                        was_reasoning = true;
                    }
                    AgentEvent::Token(text) => {
                        if was_reasoning {
                            renderer.write_line("", Color::White)?;
                            agent_line_started = false;
                            was_reasoning = false;
                        }
                        if !agent_line_started {
                            renderer.write("< ", C_AGENT)?;
                            agent_line_started = true;
                        }
                        let safe = sanitize_output(&text);
                        renderer.write(&safe, C_AGENT)?;
                    }
                    AgentEvent::ToolCall { name, args } => {
                        was_reasoning = false;
                        if agent_line_started {
                            renderer.write_line("", Color::White)?;
                            agent_line_started = false;
                        }
                        let show_details = cfg.show_tool_details.unwrap_or(false);
                        let line = if show_details {
                            format_tool_call_summary(&name, &args)
                        } else {
                            format!("◈ {}", name)
                        };
                        renderer.write_line(&sanitize_output(&line), C_TOOL)?;
                    }
                    AgentEvent::ToolResult { output } => {
                        let show_details = cfg.show_tool_details.unwrap_or(false);
                        if show_details {
                            let sanitized = sanitize_output(&output);
                            let char_count = sanitized.chars().count();
                            let preview: String = sanitized.chars().take(120).collect();
                            let preview_trimmed = if char_count > 120 {
                                format!("{}...", preview)
                            } else {
                                preview
                            };
                            let summary = if char_count > 120 {
                                format!("◈ result ({} chars): {}", char_count, preview_trimmed)
                            } else {
                                preview_trimmed
                            };
                            renderer.write_line(&summary, Color::DarkGrey)?;
                        }
                    }
                    AgentEvent::Done { response, tokens, cost } => {
                        was_reasoning = false;
                        if !agent_line_started {
                            renderer.write("< ", C_AGENT)?;
                        }
                        renderer.write_line("", Color::White)?;
                        renderer.write_line("", Color::White)?;
                        session.add_message(MessageRole::Assistant, &response);
                        session.total_tokens = session.total_tokens.saturating_add(tokens);
                        session.total_cost += cost;
                        agent_line_started = false;

                        #[cfg(feature = "loop")]
                        let loop_running = loop_state.as_ref().is_some_and(|ls| ls.active);
                        #[cfg(not(feature = "loop"))]
                        let loop_running = false;

                        if !loop_running
                            && cfg.resolve_compact_enabled()
                            && session.needs_compaction(cfg.resolve_reserve_tokens())
                            && !cli.no_session
                        {
                            renderer.write_line("auto-compacting...", Color::DarkGrey)?;
                            let compress_result = handle_compress(
                                None,
                                &mut agent, &client, &mut renderer, session, cli, cfg, context,
                                &permission, &ask_tx,
                                #[cfg(feature = "mcp")] mcp_manager,
                            ).await;
                            if let Err(e) = compress_result {
                                renderer.write_line(&format!("auto-compact error: {}", e), C_ERROR)?;
                            }
                        }

                        if !cli.no_session
                            && let Err(e) = crate::session::storage::save_session(session)
                        {
                            renderer.write_line(
                                &format!("warning: failed to save session: {}", e),
                                C_ERROR,
                            )?;
                        }
                        is_running = false;
                        agent_rx = None;

                        #[cfg(feature = "loop")]
                        if let Some(ref mut ls) = loop_state
                            && ls.active
                        {
                            if ls.should_stop() {
                                renderer.write_line(
                                    &format!(
                                        "[loop] max iterations ({}) reached, stopping",
                                        ls.iteration
                                    ),
                                    C_AGENT,
                                )?;
                                ls.active = false;
                                loop_label = None;
                            } else {
                                let summary: String = response.chars().take(200).collect();
                                ls.last_summary = Some(summary);
                                ls.iteration += 1;
                                let prompt = ls.build_prompt();
                                let runner = agent.clone().spawn_runner(prompt, Vec::new());
                                agent_rx = Some(runner.event_rx);
                                is_running = true;
                                loop_label = Some(ls.iteration_label());
                                renderer.write_line(
                                    &format!("[loop] launching {}", ls.iteration_label()),
                                    C_AGENT,
                                )?;
                            }
                        }

                        #[cfg(feature = "git-worktree")]
                        if let Some(main_path) = wt_return_path.take() {
                            match std::env::set_current_dir(&main_path) {
                                Ok(()) => {
                                    session.working_dir = compact_str::CompactString::new(&main_path);
                                    context.reload();
                                    let model = client.completion_model(session.model.to_string());
                                    agent = crate::provider::build_agent(
                                        model,
                                        cli,
                                        cfg,
                                        context,
                                        permission.clone(),
                                        ask_tx.clone(),
                                        #[cfg(feature = "mcp")] mcp_manager,
                                    ).await;
                                    render_session(&mut renderer, session, cli, cfg, context)?;
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
                    }
                    AgentEvent::Error(e) => {
                        was_reasoning = false;
                        let safe = sanitize_output(&e);
                        renderer.write_line(&format!("error: {}", safe), C_ERROR)?;
                        is_running = false;
                        agent_rx = None;
                        agent_line_started = false;
                    }
                }
                renderer.draw_bottom(
                    &input.buffer,
                    input.cursor,
                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                    is_running,
                )?;
                if let Some(ref picker) = input.picker {
                    picker.draw()?;
                }
            }
            Some(ask_req) = async {
                if let Some(rx) = &mut ask_rx {
                    rx.recv().await
                } else {
                    std::future::pending().await
                }
            } => {
                was_reasoning = false;
                if agent_line_started {
                    renderer.write_line("", Color::White)?;
                    agent_line_started = false;
                }

                renderer.write_line(
                    &format!("[permission] {}: {}", ask_req.tool, ask_req.input),
                    C_PERM,
                )?;
                renderer.write_line(
                    "  (y) allow once  (a) allow always  (n) deny  (ESC) abort",
                    C_PERM,
                )?;

                let decision = loop {
                    tokio::select! {
                        Some(ev) = user_rx.recv() => {
                            if let UserEvent::Key(key) = ev {
                                match key.code {
                                    KeyCode::Char('y') => break UserDecision::AllowOnce,
                                    KeyCode::Char('a') => {
                                        let pattern = suggest_pattern(&ask_req.tool, &ask_req.input);
                                        renderer.write_line(
                                            &format!("  -> will allow: {}", pattern),
                                            Color::Green,
                                        )?;
                                        break UserDecision::AllowAlways(pattern);
                                    }
                                    KeyCode::Char('n') | KeyCode::Esc => break UserDecision::Deny,
                                    _ => {}
                                }
                            }
                        }
                    }
                };

                let allow_pattern = match &decision {
                    UserDecision::AllowAlways(p) => Some(p.clone()),
                    _ => None,
                };
                let _ = ask_req.reply.send(decision);

                if let Some(pattern) = allow_pattern {
                    session.permission_allowlist.push(PermissionAllowEntry {
                        tool: ask_req.tool.clone(),
                        pattern: pattern.clone(),
                    });
                    if !cli.no_session {
                        let _ = crate::session::storage::save_session(session);
                    }
                    renderer.write_line(
                        &format!("  allowed {} {} (saved to session)", ask_req.tool, pattern),
                        Color::Green,
                    )?;
                }

                renderer.render_viewport()?;
                renderer.draw_bottom(
                    &input.buffer,
                    input.cursor,
                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                    is_running,
                )?;
                if let Some(ref picker) = input.picker {
                    picker.draw()?;
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(200)), if is_running => {
                renderer.draw_bottom(
                    &input.buffer,
                    input.cursor,
                    &StatusLine::render(session, is_running, 0, loop_label.as_deref(), context.current_prompt_name.as_deref()),
                    is_running,
                )?;
                if let Some(ref picker) = input.picker {
                    picker.draw()?;
                }
            }
            else => {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
        }
    }

    Ok(())
}

fn suggest_pattern(tool: &str, input: &str) -> String {
    match tool {
        "bash" => {
            let first = input.split_whitespace().next().unwrap_or("*");
            format!("{} *", first)
        }
        "read" | "write" | "edit" | "list_dir" => {
            let path = std::path::Path::new(input);
            let parent = path
                .parent()
                .map(|p| p.to_string_lossy())
                .unwrap_or(std::borrow::Cow::Borrowed("*"));
            if parent.is_empty() {
                "**".to_string()
            } else {
                format!("{}/*", parent)
            }
        }
        "grep" | "find_files" => {
            let first = input.split_whitespace().next().unwrap_or("*");
            format!("{}*", first)
        }
        _ => "*".to_string(),
    }
}
