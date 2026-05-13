mod input;
mod renderer;
mod status;

use std::io::Write;

use chrono::Datelike;
use compact_str::CompactString;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyCode, KeyModifiers, MouseEventKind,
};
use crossterm::style::Color;
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{ExecutableCommand, event};
use rig::client::CompletionClient;
use rig::providers::openrouter;
use smallvec::SmallVec;
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
const C_REASONING: Color = Color::DarkMagenta;

struct TerminalGuard;

impl TerminalGuard {
    fn new() -> std::io::Result<Self> {
        let mut stdout = std::io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(Clear(ClearType::All))?;
        stdout.execute(EnableMouseCapture)?;
        terminal::enable_raw_mode()?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = stdout.execute(DisableMouseCapture);
        let _ = stdout.execute(LeaveAlternateScreen);
        let _ = stdout.flush();
    }
}

fn format_time(rfc3339: &str) -> CompactString {
    let dt = chrono::DateTime::parse_from_rfc3339(rfc3339).ok();
    let dt = match dt {
        Some(dt) => dt,
        None => return CompactString::new(rfc3339),
    };
    let local = dt.with_timezone(&chrono::Local);
    let now = chrono::Local::now();
    if local.date_naive() == now.date_naive() {
        CompactString::new(local.format("%H:%M").to_string())
    } else if local.year() == now.year() {
        CompactString::new(local.format("%b %d %H:%M").to_string())
    } else {
        CompactString::new(local.format("%Y-%m-%d %H:%M").to_string())
    }
}

fn render_session(
    renderer: &mut Renderer,
    session: &Session,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
) -> anyhow::Result<()> {
    renderer.clear_content()?;
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
        let (prefix, c) = match msg.role {
            crate::session::MessageRole::User => (">", C_USER),
            crate::session::MessageRole::Assistant => ("<", C_AGENT),
        };
        for line in msg.content.lines() {
            renderer.write_line(&format!("{} {}", prefix, line), c)?;
        }
        renderer.write_line("", Color::White)?;
    }
    Ok(())
}

fn sanitize_output(text: &str) -> CompactString {
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
    CompactString::from(result)
}


#[allow(clippy::too_many_arguments)]
fn handle_slash(
    text: &str,
    agent: &mut agent::ZAgent,
    client: &openrouter::Client,
    renderer: &mut Renderer,
    session: &mut Session,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    show_reasoning: &mut bool,
    is_running: &mut bool,
    input: &mut InputEditor,
    todo_tools_enabled: &mut bool,
) -> anyhow::Result<()> {
    let parts: SmallVec<[&str; 3]> = text.trim().splitn(3, ' ').collect();
    match parts[0] {
        "/model" => {
            if parts.len() < 2 {
                renderer.write_line(&format!("current model: {}", session.model), C_AGENT)?;
            } else {
                let new_model = CompactString::new(parts[1].trim());
                let model = client.completion_model(new_model.to_string());
                *agent = agent::build_agent(model, cli, cfg, context, *todo_tools_enabled);
                session.model = new_model.clone();
                session.provider = cli.resolve_provider(cfg);
                renderer.write_line(&format!("switched to model: {}", new_model), C_AGENT)?;
            }
        }
        "/sessions" => {
            if parts.len() < 2 {
                let sessions = crate::session::storage::find_recent_sessions(20)?;
                if sessions.is_empty() {
                    renderer.write_line("no saved sessions", C_AGENT)?;
                } else {
                    renderer.write_line(&format!("recent sessions ({}):", sessions.len()), C_AGENT)?;
                    for s in &sessions {
                        let last = s.messages.last()
                            .map(|m| format!("...{}", &m.content.chars().take(30).collect::<String>()))
                            .unwrap_or_default();
                        let time = format_time(&s.updated_at);
                        renderer.write_line(
                            &format!("  {}  {}  {}msgs  {}  {}",
                                &s.id[..8], time, s.messages.len(), s.model, last),
                            C_RESULT,
                        )?;
                    }
                }
            } else if parts[1] == "delete" && parts.len() >= 3 {
                let prefix = parts[2].trim();
                let sessions = crate::session::storage::find_sessions_by_prefix(prefix)?;
                if sessions.is_empty() {
                    renderer.write_line(&format!("no session matching '{}'", prefix), C_AGENT)?;
                } else if sessions.len() == 1 {
                    if let Some(s) = sessions.into_iter().next() {
                        let id = s.id.clone();
                        let preview = s.messages.last()
                            .map(|m| format!("...{}", &m.content.chars().take(40).collect::<String>()))
                            .unwrap_or_default();
                        if let Err(e) = crate::session::storage::delete_session(&id) {
                            renderer.write_line(&format!("failed to delete: {}", e), C_ERROR)?;
                        } else {
                            renderer.write_line(&format!("deleted session {} {}", &id[..8], preview), C_AGENT)?;
                        }
                    }
                } else {
                    renderer.write_line(&format!("multiple sessions match '{}', be more specific", prefix), C_AGENT)?;
                    for s in &sessions {
                        let last = s.messages.last()
                            .map(|m| format!("...{}", &m.content.chars().take(30).collect::<String>()))
                            .unwrap_or_default();
                        let time = format_time(&s.updated_at);
                        renderer.write_line(
                            &format!("  {}  {}  {}msgs  {}  {}",
                                &s.id[..8], time, s.messages.len(), s.model, last),
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
                    if let Some(s) = sessions.into_iter().next() {
                        let msg_count = s.messages.len();
                        *session = s;
                        render_session(renderer, session, cli, cfg, context)?;
                        renderer.write_line(&format!("loaded session ({} msgs)", msg_count), C_AGENT)?;
                    }
                } else {
                    renderer.write_line(&format!("multiple sessions match '{}':", prefix), C_AGENT)?;
                    for s in &sessions {
                        let last = s.messages.last()
                            .map(|m| format!("...{}", &m.content.chars().take(30).collect::<String>()))
                            .unwrap_or_default();
                        let time = format_time(&s.updated_at);
                        renderer.write_line(
                            &format!("  {}  {}  {}msgs  {}  {}",
                                &s.id[..8], time, s.messages.len(), s.model, last),
                            C_RESULT,
                        )?;
                    }
                }
            }
        }
        "/reasoning" => {
            *show_reasoning = !*show_reasoning;
            renderer.write_line(
                &format!("reasoning visibility: {}", if *show_reasoning { "on" } else { "off" }),
                C_AGENT,
            )?;
        }
        "/toggle" => {
            if parts.len() < 2 {
                renderer.write_line("usage: /toggle <feature> [on|off]", C_AGENT)?;
                renderer.write_line("features:", C_AGENT)?;
                renderer.write_line(&format!("  todo  {}", if *todo_tools_enabled { "on" } else { "off" }), C_RESULT)?;
            } else if parts[1] == "todo" {
                if parts.len() < 3 {
                    renderer.write_line(&format!("todo tools: {}", if *todo_tools_enabled { "on" } else { "off" }), C_AGENT)?;
                } else {
                    let new_state = match parts[2] {
                        "on" => true,
                        "off" => false,
                        other => {
                            renderer.write_line(&format!("invalid: '{}', use on or off", other), C_ERROR)?;
                            return Ok(());
                        }
                    };
                    if new_state == *todo_tools_enabled {
                        renderer.write_line(&format!("todo tools already {}", if new_state { "on" } else { "off" }), C_AGENT)?;
                    } else {
                        *todo_tools_enabled = new_state;
                        let model = client.completion_model(session.model.to_string());
                        *agent = agent::build_agent(model, cli, cfg, context, *todo_tools_enabled);
                        renderer.write_line(&format!("todo tools: {}", if *todo_tools_enabled { "on" } else { "off" }), C_AGENT)?;
                    }
                }
            } else {
                renderer.write_line(&format!("unknown feature: {}", parts[1]), C_ERROR)?;
            }
        }
        "/quit" => {
            *is_running = false;
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "quit").into());
        }
        "/clear" => {
            render_session(renderer, session, cli, cfg, context)?;
        }
        "/undo" => {
            let removed = undo_last(session);
            if removed > 0 {
                render_session(renderer, session, cli, cfg, context)?;
                renderer.write_line(&format!("removed {} message(s)", removed), C_AGENT)?;
            } else {
                renderer.write_line("nothing to undo", C_AGENT)?;
            }
        }
        "/retry" => {
            let last_user = session.messages.iter().rev().find(|m| m.role == crate::session::MessageRole::User).cloned();
            match last_user {
                Some(msg) => {
                    input.buffer = msg.content.clone();
                    input.cursor = msg.content.len();
                    renderer.write_line("edit last message and press Enter to retry", C_AGENT)?;
                }
                None => {
                    renderer.write_line("no previous message to retry", C_AGENT)?;
                }
            }
        }
        "/help" => {
            renderer.write_line("commands:", C_AGENT)?;
            renderer.write_line("  /model [name]          show or switch model", C_RESULT)?;
            renderer.write_line("  /sessions              list recent sessions", C_RESULT)?;
            renderer.write_line("  /sessions <id>         load a session (by ID prefix)", C_RESULT)?;
            renderer.write_line("  /sessions delete <id>  delete a session", C_RESULT)?;
            renderer.write_line("  /reasoning             toggle reasoning visibility", C_RESULT)?;
            renderer.write_line("  /toggle <f> [on|off]  toggle features (todo)", C_RESULT)?;
            renderer.write_line("  /clear                 clear screen", C_RESULT)?;
            renderer.write_line("  /undo                  undo last exchange", C_RESULT)?;
            renderer.write_line("  /retry                 retry last prompt", C_RESULT)?;
            renderer.write_line("  /quit                  exit zerostack", C_RESULT)?;
            renderer.write_line("  /help                  show this message", C_RESULT)?;
            renderer.write_line("", C_AGENT)?;
            renderer.write_line("keys:", C_AGENT)?;
            renderer.write_line("  PgUp/PgDn             scroll chat history", C_RESULT)?;
            renderer.write_line("  Home/End               jump to top/bottom", C_RESULT)?;
            renderer.write_line("  Ctrl+R                 toggle reasoning", C_RESULT)?;
            renderer.write_line("  Ctrl+C                 interrupt/quit", C_RESULT)?;
            renderer.write_line("  mouse scroll           scroll chat", C_RESULT)?;
        }
        _ => {
            renderer.write_line(&format!("unknown command: {} (try /help)", parts[0]), C_ERROR)?;
        }
    }
    Ok(())
}

fn undo_last(session: &mut Session) -> usize {
    let len = session.messages.len();
    if len == 0 {
        return 0;
    }
    if session.messages[len - 1].role == crate::session::MessageRole::Assistant {
        session.messages.pop();
        if session.messages.last().is_some_and(|m| m.role == crate::session::MessageRole::User) {
            session.messages.pop();
            return 2;
        }
        return 1;
    }
    if session.messages[len - 1].role == crate::session::MessageRole::User {
        session.messages.pop();
        return 1;
    }
    0
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
    let mut show_reasoning = true;
    let mut was_reasoning = false;
    let mut todo_tools_enabled = false;
    let mut answer_tx: Option<tokio::sync::oneshot::Sender<String>> = None;

    render_session(&mut renderer, session, cli, cfg, context)?;
    renderer.draw_bottom("", 0, &StatusLine::render(session, false), false)?;

    let (user_tx, mut user_rx) = mpsc::channel::<UserEvent>(64);
    let user_tx_clone = user_tx.clone();
    std::thread::spawn(move || loop {
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
                            &StatusLine::render(session, is_running),
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
                            &StatusLine::render(session, is_running),
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
                                let _ = crate::agent::tools::PENDING_QUESTION.lock().unwrap().take();
                                renderer.write_line("interrupted", C_ERROR)?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running),
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
                                        renderer.write_line(&format!("[Answer] {}", answer), C_USER)?;
                                        renderer.write_line("", Color::White)?;
                                    }
                                }
                                KeyCode::Esc => {
                                    answer_tx.take();
                                    let _ = crate::agent::tools::PENDING_QUESTION.lock().unwrap().take();
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
                                &StatusLine::render(session, is_running),
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
                                C_AGENT,
                            )?;
                            renderer.draw_bottom(
                                &input.buffer,
                                input.cursor,
                                &StatusLine::render(session, is_running),
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
                                    &StatusLine::render(session, is_running),
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
                                    &StatusLine::render(session, is_running),
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
                                    &StatusLine::render(session, is_running),
                                    is_running,
                                )?;
                                continue;
                            }
                            KeyCode::End => {
                                renderer.scroll_to_bottom()?;
                                renderer.draw_bottom(
                                    &input.buffer,
                                    input.cursor,
                                    &StatusLine::render(session, is_running),
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
                                    renderer.write_line(&format!("> {}", safe_line), C_USER)?;
                                }
                                renderer.write_line("", Color::White)?;
                                let result = handle_slash(&text, &mut agent, &client, &mut renderer, session, cli, cfg, context, &mut show_reasoning, &mut is_running, &mut input, &mut todo_tools_enabled);
                                if let Err(e) = result {
                                    if e.downcast_ref::<std::io::Error>().is_some_and(|e: &std::io::Error| e.kind() == std::io::ErrorKind::Interrupted) {
                                        break;
                                    }
                                    renderer.write_line(&format!("error: {}", e), C_ERROR)?;
                                }
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
                                    text.to_string(),
                                    history,
                                );
                                agent_rx = Some(runner.event_rx);
                                is_running = true;

                                session.add_message(crate::session::MessageRole::User, &text);
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
                    AgentEvent::Reasoning(text) => {
                        if !show_reasoning {
                            continue;
                        }
                        if !agent_line_started {
                            renderer.write("< ", C_REASONING)?;
                            agent_line_started = true;
                        }
                        let safe = sanitize_output(&text);
                        renderer.write(&safe, C_REASONING)?;
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
                        renderer.write_line(&preview, C_RESULT)?;
                        renderer.write_line("", Color::White)?;
                    }
                    AgentEvent::Done { response, tokens, cost } => {
                        was_reasoning = false;
                        if !agent_line_started {
                            renderer.write("< ", C_AGENT)?;
                        }
                        renderer.write_line("", Color::White)?;
                        renderer.write_line("", Color::White)?;
                        session.add_message(crate::session::MessageRole::Assistant, &response);
                        session.total_tokens = session.total_tokens.saturating_add(tokens);
                        session.total_cost += cost;
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
                    &StatusLine::render(session, is_running),
                    is_running,
                )?;
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(200)), if is_running => {
                if answer_tx.is_none() {
                    let mut pq = crate::agent::tools::PENDING_QUESTION.lock().unwrap();
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
