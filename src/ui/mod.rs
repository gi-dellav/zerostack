mod input;
mod renderer;
mod status;

use std::io::Write;

use crossterm::style::Color;
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{ExecutableCommand, event};
use rig::client::CompletionClient;
use rig::providers::openrouter;
use tokio::sync::mpsc;

use crate::agent;
use crate::cli::Cli;
use crate::config::Config;
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

fn handle_slash(
    text: &str,
    agent: &mut agent::ZAgent,
    client: &openrouter::Client,
    renderer: &mut Renderer,
    session: &mut Session,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
) -> anyhow::Result<()> {
    let parts: Vec<&str> = text.trim().splitn(2, ' ').collect();
    match parts[0] {
        "/model" => {
            if parts.len() < 2 {
                renderer.write_line(&format!("current model: {}", session.model), C_AGENT)?;
            } else {
                let new_model = parts[1].trim().to_string();
                let model = client.completion_model(&new_model);
                *agent = agent::build_agent(model, cli, cfg, context);
                session.model = new_model.clone();
                session.provider = cli.resolve_provider(cfg);
                renderer.write_line(&format!("switched to model: {}", new_model), C_AGENT)?;
            }
        }
        "/sessions" => {
            let parts: Vec<&str> = text.trim().splitn(2, ' ').collect();
            if parts.len() < 2 {
                let sessions = crate::session::storage::find_recent_sessions(20)?;
                if sessions.is_empty() {
                    renderer.write_line("no saved sessions", C_AGENT)?;
                } else {
                    renderer.write_line(&format!("recent sessions ({}):", sessions.len()), C_AGENT)?;
                    for s in &sessions {
                        let last = s.messages.last()
                            .map(|m| format!("...{}", &m.content.chars().take(40).collect::<String>()))
                            .unwrap_or_default();
                        renderer.write_line(
                            &format!("  {}  {}  {}msgs  {}", &s.id[..8], s.model, s.messages.len(), last),
                            C_RESULT,
                        )?;
                    }
                }
            } else {
                let prefix = parts[1].trim();
                let sessions = crate::session::storage::find_sessions_by_prefix(prefix)?;
                if sessions.is_empty() {
                    renderer.write_line(&format!("no session matching '{}'", prefix), C_AGENT)?;
                } else if sessions.len() == 1 {
                    let s = sessions.into_iter().next().unwrap();
                    let msg_count = s.messages.len();
                    *session = s;
                    renderer.write_line(&format!("loaded session ({} msgs)", msg_count), C_AGENT)?;
                } else {
                    renderer.write_line(&format!("multiple sessions match '{}':", prefix), C_AGENT)?;
                    for s in &sessions {
                        let last = s.messages.last()
                            .map(|m| format!("...{}", &m.content.chars().take(40).collect::<String>()))
                            .unwrap_or_default();
                        renderer.write_line(
                            &format!("  {}  {}  {}msgs  {}", &s.id[..8], s.model, s.messages.len(), last),
                            C_RESULT,
                        )?;
                    }
                }
            }
        }
        "/help" => {
            renderer.write_line("commands:", C_AGENT)?;
            renderer.write_line("  /model [name]       show or switch model", C_RESULT)?;
            renderer.write_line("  /sessions [id]      list or load a session", C_RESULT)?;
            renderer.write_line("  /help               show this message", C_RESULT)?;
        }
        _ => {
            renderer.write_line(&format!("unknown command: {} (try /help)", parts[0]), C_ERROR)?;
        }
    }
    Ok(())
}

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

    let welcome = format!(
        "zerostack {}  {}  {}",
        cli.resolve_provider(cfg),
        cli.resolve_model(cfg),
        env!("CARGO_PKG_VERSION")
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
                            if text.starts_with('/') {
                                for line in text.lines() {
                                    let safe_line = sanitize_output(line);
                                    renderer.write_line(&format!("> {}", safe_line), C_USER)?;
                                }
                                renderer.write_line("", Color::White)?;
                                handle_slash(&text, &mut agent, &client, &mut renderer, session, cli, cfg, context)?;
                                if !cli.no_session {
                                    let _ = crate::session::storage::save_session(session);
                                }
                            } else {
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
