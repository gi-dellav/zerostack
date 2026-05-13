mod events;
mod input;
mod renderer;
mod slash;
mod status;
mod terminal;

use crossterm::event;
use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};
use crossterm::style::Color;
use rig::providers::openrouter;
use tokio::sync::mpsc;

use crate::agent;
use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::event::{AgentEvent, UserEvent};
use crate::session::MessageRole;
use crate::session::Session;
use crate::ui::events::{render_session, sanitize_output};
use crate::ui::input::InputEditor;
use crate::ui::renderer::Renderer;
use crate::ui::slash::{handle_compress, handle_slash};
use crate::ui::status::StatusLine;
use crate::ui::terminal::TerminalGuard;

const C_AGENT: Color = Color::White;
const C_ERROR: Color = Color::Red;
const C_TOOL: Color = Color::Yellow;

pub async fn run_interactive(
    client: openrouter::Client,
    mut agent: agent::ZAgent,
    cli: &Cli,
    cfg: &Config,
    session: &mut Session,
    context: &ContextFiles,
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
    let mut answer_tx: Option<tokio::sync::oneshot::Sender<String>> = None;

    render_session(&mut renderer, session, cli, cfg, context)?;
    renderer.draw_bottom("", 0, &StatusLine::render(session, false, 0), false)?;

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
                            &StatusLine::render(session, is_running, 0),
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
                            &StatusLine::render(session, is_running, 0),
                            is_running,
                        )?;
                        continue;
                    }
                    UserEvent::Key(key) => {
                        let is_ctrl_c = key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        if is_ctrl_c {
                            if is_running {
                                is_running = false;
                                agent_rx = None;
                                answer_tx.take();
                                let _ = agent::tools::PENDING_QUESTION.lock().unwrap().take();
                                renderer.write_line("interrupted", C_ERROR)?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running, 0),
                                    is_running,
                                )?;
                            } else {
                                break;
                            }
                            continue;
                        }

                        if answer_tx.is_some() {
                            match key.code {
                                KeyCode::Enter => {
                                    let answer = input.buffer.clone();
                                    if !answer.is_empty() {
                                        if let Some(tx) = answer_tx.take() {
                                            let _ = tx.send(answer.to_string());
                                        }
                                        input.buffer.clear();
                                        input.cursor = 0;
                                        renderer.write_line(&format!("[Answer] {}", answer), Color::Green)?;
                                        renderer.write_line("", Color::White)?;
                                    }
                                }
                                KeyCode::Esc => {
                                    answer_tx.take();
                                    let _ = agent::tools::PENDING_QUESTION.lock().unwrap().take();
                                    renderer.write_line("[Cancelled]", C_ERROR)?;
                                    input.buffer.clear();
                                    input.cursor = 0;
                                }
                                KeyCode::Char(c) => {
                                    input.buffer.insert(input.cursor, c);
                                    input.cursor += 1;
                                }
                                KeyCode::Backspace if input.cursor > 0 => {
                                    input.cursor -= 1;
                                    input.buffer.remove(input.cursor);
                                }
                                KeyCode::Delete if input.cursor < input.buffer.len() => {
                                    input.buffer.remove(input.cursor);
                                }
                                KeyCode::Left if input.cursor > 0 => {
                                    input.cursor -= 1;
                                }
                                KeyCode::Right if input.cursor < input.buffer.len() => {
                                    input.cursor += 1;
                                }
                                KeyCode::Home => input.cursor = 0,
                                KeyCode::End => input.cursor = input.buffer.len(),
                                _ => {}
                            }
                            renderer.draw_bottom(
                                &input.buffer,
                                input.cursor,
                                &StatusLine::render(session, is_running, 0),
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
                                &StatusLine::render(session, is_running, 0),
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
                                    &StatusLine::render(session, is_running, 0),
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
                                    &StatusLine::render(session, is_running, 0),
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
                                    &StatusLine::render(session, is_running, 0),
                                    is_running,
                                )?;
                                continue;
                            }
                            KeyCode::End => {
                                renderer.scroll_to_bottom()?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running, 0),
                                    is_running,
                                )?;
                                continue;
                            }
                            _ => {}
                        }

                        if let Some(text) = input.handle_key(key) {
                            if renderer.is_scrolling() {
                                renderer.scroll_to_bottom()?;
                            }
                            if text.starts_with('/') {
                                for line in text.lines() {
                                    let safe_line = sanitize_output(line);
                                    renderer.write_line(&format!("> {}", safe_line), Color::Green)?;
                                }
                                renderer.write_line("", Color::White)?;
                                let result = handle_slash(&text, &mut agent, &client, &mut renderer, session, cli, cfg, context, &mut show_reasoning, &mut is_running, &mut input, &mut todo_tools_enabled);
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
                                            &mut todo_tools_enabled,
                                        ).await;
                                        if let Err(e) = compress_result {
                                            renderer.write_line(&format!("compress error: {}", e), C_ERROR)?;
                                        }
                                        let _ = crate::session::storage::save_session(session);
                                    }
                                    Err(e) => {
                                        if e.downcast_ref::<std::io::Error>().is_some_and(|e: &std::io::Error| e.kind() == std::io::ErrorKind::Interrupted) {
                                            break;
                                        }
                                        renderer.write_line(&format!("error: {}", e), C_ERROR)?;
                                    }
                                    Ok(_) => {}
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

                                let history = agent::runner::convert_history(session);
                                let runner = agent::runner::spawn_agent(
                                    agent.clone(),
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
                            &StatusLine::render(session, is_running, 0),
                            is_running,
                        )?;
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
                        renderer.write_line("", Color::White)?;
                        let args_str = serde_json::to_string(&args).unwrap_or_default();
                        let safe = sanitize_output(&format!("[{} {}]", name, args_str));
                        renderer.write_line(&safe, C_TOOL)?;
                        renderer.write_line("", Color::White)?;
                    }
                    AgentEvent::ToolResult { output } => {
                        renderer.write_line("", Color::White)?;
                        let sanitized = sanitize_output(&output);
                        let preview: String = sanitized.chars().take(2000).collect();
                        renderer.write_line(&preview, Color::DarkGrey)?;
                        renderer.write_line("", Color::White)?;
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

                        // Auto-compaction check
                        if cfg.resolve_compact_enabled()
                            && session.needs_compaction(cfg.resolve_reserve_tokens())
                            && !cli.no_session
                        {
                            renderer.write_line("auto-compacting...", Color::DarkGrey)?;
                            // Use the non-streaming compress (defer compress handles it)
                            let instructions: Option<&str> = None;
                            let compress_result = handle_compress(
                                instructions,
                                &mut agent, &client, &mut renderer, session, cli, cfg, context,
                                &mut todo_tools_enabled,
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
                    &StatusLine::render(session, is_running, 0),
                    is_running,
                )?;
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(200)), if is_running => {
                if answer_tx.is_none() {
                    let mut pq = agent::tools::PENDING_QUESTION.lock().unwrap();
                    if let Some(req) = pq.take() {
                        renderer.write_line(&format!("[Question] {}", req.question), C_TOOL)?;
                        renderer.write_line("", Color::White)?;
                        input.buffer.clear();
                        input.cursor = 0;
                        answer_tx = Some(req.answer_tx);
                    }
                }
                renderer.draw_bottom(
                    &input.buffer,
                    input.cursor,
                    &StatusLine::render(session, is_running, 0),
                    is_running,
                )?;
            }
            else => {
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
        }
    }

    Ok(())
}
