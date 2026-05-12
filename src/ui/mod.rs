mod input;
mod renderer;
mod status;

use std::io::Write;

use crossterm::style::Color;
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{ExecutableCommand, event};
use tokio::sync::mpsc;

use crate::agent;
use crate::cli::Cli;
use crate::context::ContextFiles;
use crate::event::{AgentEvent, UserEvent};
use crate::session::Session;
use crate::ui::input::InputEditor;
use crate::ui::renderer::Renderer;
use crate::ui::status::StatusLine;

const C_USER: Color = Color::Green;
const C_AGENT: Color = Color::White;
const C_TOOL: Color = Color::Yellow;
const C_RESULT: Color = Color::DarkGrey;
const C_ERROR: Color = Color::Red;

struct TerminalGuard;

impl TerminalGuard {
    fn new() -> std::io::Result<Self> {
        let mut stdout = std::io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(Clear(ClearType::All))?;
        terminal::enable_raw_mode()?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = stdout.execute(LeaveAlternateScreen);
        let _ = stdout.flush();
    }
}

fn sanitize_output(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') | Some(']') => {
                    for next in &mut chars {
                        if next.is_ascii_alphabetic() || next == '~' {
                            break;
                        }
                    }
                }
                Some(_) => {}
                None => break,
            }
        } else if c.is_ascii_control() && c != '\n' && c != '\t' && c != '\r' {
            continue;
        } else {
            result.push(c);
        }
    }
    result
}

pub async fn run_interactive<M, P>(
    agent: &rig::agent::Agent<M, P>,
    cli: &Cli,
    session: &mut Session,
    context: &ContextFiles,
) -> anyhow::Result<()>
where
    M: rig::completion::CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    let _guard = TerminalGuard::new()?;

    let mut renderer = Renderer::new()?;
    let mut input = InputEditor::new();
    let mut is_running = false;
    let mut agent_rx: Option<mpsc::Receiver<AgentEvent>> = None;
    let mut agent_line_started = false;

    let welcome = format!(
        "zerostack {}  {}  {}",
        cli.provider, cli.model, env!("CARGO_PKG_VERSION")
    );
    renderer.write_line(&welcome, Color::Cyan)?;
    renderer.write_line("", Color::White)?;

    if context.agents.is_some() {
        renderer.write_line("loaded AGENTS.md", Color::DarkGrey)?;
        renderer.write_line("", Color::White)?;
    }

    for msg in &session.messages {
        let (prefix, c) = match msg.role.as_str() {
            "user" => (">", C_USER),
            _ => ("<", C_AGENT),
        };
        for line in msg.content.lines() {
            renderer.write_line(&format!("{} {}", prefix, line), c)?;
        }
        renderer.write_line("", Color::White)?;
    }

    renderer.draw_bottom("", 0, &StatusLine::render(session, false), false)?;

    let (user_tx, mut user_rx) = mpsc::channel::<UserEvent>(64);
    let user_tx_clone = user_tx.clone();
    std::thread::spawn(move || loop {
        if let Ok(event::Event::Key(key)) = event::read() {
            let is_ctrl_c = key.code == event::KeyCode::Char('c')
                && key.modifiers.contains(event::KeyModifiers::CONTROL);
            let ev = if is_ctrl_c {
                UserEvent::Quit
            } else {
                UserEvent::Key(key)
            };
            if user_tx_clone.blocking_send(ev).is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            Some(ev) = user_rx.recv() => {
                match ev {
                    UserEvent::Quit => break,
                    UserEvent::Key(key) => {
                        if let Some(text) = input.handle_key(key) {
                            for line in text.lines() {
                                let safe_line = sanitize_output(line);
                                renderer.write_line(&format!("> {}", safe_line), C_USER)?;
                            }
                            renderer.write_line("", Color::White)?;

                            let history = agent::convert_history(&session.messages);

                            let runner = agent::spawn_agent(
                                agent.clone(),
                                text.clone(),
                                history,
                            );
                            agent_rx = Some(runner.event_rx);
                            is_running = true;

                            session.add_message("user", &text);
                        }
                        renderer.draw_bottom(
                            &input.buffer,
                            input.cursor,
                            &StatusLine::render(session, is_running),
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
                    AgentEvent::Token(text) => {
                        if !agent_line_started {
                            renderer.write("< ", C_AGENT)?;
                            agent_line_started = true;
                        }
                        let safe = sanitize_output(&text);
                        renderer.write(&safe, C_AGENT)?;
                    }
                    AgentEvent::ToolCall { name, args } => {
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
                        let preview: String = sanitized.chars().take(200).collect();
                        renderer.write_line(&preview, C_RESULT)?;
                        renderer.write_line("", Color::White)?;
                    }
                    AgentEvent::Done { response } => {
                        if !agent_line_started {
                            renderer.write("< ", C_AGENT)?;
                        }
                        renderer.write_line("", Color::White)?;
                        renderer.write_line("", Color::White)?;
                        session.add_message("assistant", &response);
                        agent_line_started = false;
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
                    &StatusLine::render(session, is_running),
                    is_running,
                )?;
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(200)), if is_running => {
                renderer.draw_bottom(
                    &input.buffer,
                    input.cursor,
                    &StatusLine::render(session, is_running),
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
