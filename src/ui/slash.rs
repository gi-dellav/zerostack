use compact_str::CompactString;
use crossterm::style::Color;
use rig::client::CompletionClient;
use rig::providers::openrouter;
use smallvec::SmallVec;

use crate::agent;
use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::session::{MessageRole, Session};
use crate::ui::events::{format_time, render_session};
use crate::ui::input::InputEditor;
use crate::ui::renderer::Renderer;

const C_AGENT: Color = Color::White;
const C_RESULT: Color = Color::DarkGrey;
const C_ERROR: Color = Color::Red;

pub fn undo_last(session: &mut Session) -> usize {
    let len = session.messages.len();
    if len == 0 {
        return 0;
    }
    if session.messages[len - 1].role == MessageRole::Assistant {
        session.messages.pop();
        if session
            .messages
            .last()
            .is_some_and(|m| m.role == MessageRole::User)
        {
            session.messages.pop();
            return 2;
        }
        return 1;
    }
    if session.messages[len - 1].role == MessageRole::User {
        session.messages.pop();
        return 1;
    }
    0
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_compress(
    instructions: Option<&str>,
    agent: &mut agent::ZAgent,
    client: &openrouter::Client,
    renderer: &mut Renderer,
    session: &mut Session,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    todo_tools_enabled: &mut bool,
) -> anyhow::Result<()> {
    renderer.write_line("compressing...", C_AGENT)?;
    renderer.write_line("", Color::White)?;

    // Find messages to summarize (messages before the reserve window)
    let reserve = cfg.resolve_reserve_tokens();
    let keep_recent = cfg.resolve_keep_recent_tokens();
    let max_tokens = session.context_window.saturating_sub(reserve);

    if session.total_estimated_tokens <= max_tokens {
        renderer.write_line("context within limits, no compression needed", C_AGENT)?;
        return Ok(());
    }

    // Walk backwards to find cut point
    let mut accumulated = 0u64;
    let mut cut_idx = session.messages.len();
    for (i, msg) in session.messages.iter().enumerate().rev() {
        if accumulated >= keep_recent {
            cut_idx = i + 1;
            break;
        }
        accumulated = accumulated.saturating_add(msg.estimated_tokens);
    }

    if cut_idx == 0 {
        renderer.write_line("nothing to compress (entire context is recent)", C_AGENT)?;
        return Ok(());
    }

    let messages_to_summarize = &session.messages[..cut_idx];
    let previous_summary = session.compactions.last().map(|c| c.summary.as_str());

    let summary = agent::compress::compress_messages(
        client,
        &session.model,
        messages_to_summarize,
        previous_summary,
        instructions,
    )
    .await?;

    let tokens_before: u64 = messages_to_summarize
        .iter()
        .map(|m| m.estimated_tokens)
        .sum();

    // Create compaction entry
    session.compress(summary, cut_idx, tokens_before);

    // Rebuild agent with potentially new context
    let model = client.completion_model(session.model.to_string());
    *agent = agent::build_agent(model, cli, cfg, context, *todo_tools_enabled);

    render_session(renderer, session, cli, cfg, context)?;
    renderer.write_line(
        &format!(
            "compressed {} messages (saved ~{} tokens)",
            cut_idx, tokens_before,
        ),
        C_AGENT,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn handle_slash(
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
                    renderer
                        .write_line(&format!("recent sessions ({}):", sessions.len()), C_AGENT)?;
                    for s in &sessions {
                        let last = s
                            .messages
                            .last()
                            .map(|m| {
                                format!("...{}", &m.content.chars().take(30).collect::<String>())
                            })
                            .unwrap_or_default();
                        let time = format_time(&s.updated_at);
                        renderer.write_line(
                            &format!(
                                "  {}  {}  {}msgs  {}  {}",
                                &s.id[..8],
                                time,
                                s.messages.len(),
                                s.model,
                                last
                            ),
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
                        let preview = s
                            .messages
                            .last()
                            .map(|m| {
                                format!("...{}", &m.content.chars().take(40).collect::<String>())
                            })
                            .unwrap_or_default();
                        if let Err(e) = crate::session::storage::delete_session(&id) {
                            renderer.write_line(&format!("failed to delete: {}", e), C_ERROR)?;
                        } else {
                            renderer.write_line(
                                &format!("deleted session {} {}", &id[..8], preview),
                                C_AGENT,
                            )?;
                        }
                    }
                } else {
                    renderer.write_line(
                        &format!("multiple sessions match '{}', be more specific", prefix),
                        C_AGENT,
                    )?;
                    for s in &sessions {
                        let last = s
                            .messages
                            .last()
                            .map(|m| {
                                format!("...{}", &m.content.chars().take(30).collect::<String>())
                            })
                            .unwrap_or_default();
                        let time = format_time(&s.updated_at);
                        renderer.write_line(
                            &format!(
                                "  {}  {}  {}msgs  {}  {}",
                                &s.id[..8],
                                time,
                                s.messages.len(),
                                s.model,
                                last
                            ),
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
                        renderer
                            .write_line(&format!("loaded session ({} msgs)", msg_count), C_AGENT)?;
                    }
                } else {
                    renderer
                        .write_line(&format!("multiple sessions match '{}':", prefix), C_AGENT)?;
                    for s in &sessions {
                        let last = s
                            .messages
                            .last()
                            .map(|m| {
                                format!("...{}", &m.content.chars().take(30).collect::<String>())
                            })
                            .unwrap_or_default();
                        let time = format_time(&s.updated_at);
                        renderer.write_line(
                            &format!(
                                "  {}  {}  {}msgs  {}  {}",
                                &s.id[..8],
                                time,
                                s.messages.len(),
                                s.model,
                                last
                            ),
                            C_RESULT,
                        )?;
                    }
                }
            }
        }
        "/reasoning" => {
            *show_reasoning = !*show_reasoning;
            renderer.write_line(
                &format!(
                    "reasoning visibility: {}",
                    if *show_reasoning { "on" } else { "off" }
                ),
                C_AGENT,
            )?;
        }
        "/toggle" => {
            if parts.len() < 2 {
                renderer.write_line("usage: /toggle <feature> [on|off]", C_AGENT)?;
                renderer.write_line("features:", C_AGENT)?;
                renderer.write_line(
                    &format!("  todo  {}", if *todo_tools_enabled { "on" } else { "off" }),
                    C_RESULT,
                )?;
            } else if parts[1] == "todo" {
                if parts.len() < 3 {
                    renderer.write_line(
                        &format!(
                            "todo tools: {}",
                            if *todo_tools_enabled { "on" } else { "off" }
                        ),
                        C_AGENT,
                    )?;
                } else {
                    let new_state = match parts[2] {
                        "on" => true,
                        "off" => false,
                        other => {
                            renderer.write_line(
                                &format!("invalid: '{}', use on or off", other),
                                C_ERROR,
                            )?;
                            return Ok(());
                        }
                    };
                    if new_state == *todo_tools_enabled {
                        renderer.write_line(
                            &format!(
                                "todo tools already {}",
                                if new_state { "on" } else { "off" }
                            ),
                            C_AGENT,
                        )?;
                    } else {
                        *todo_tools_enabled = new_state;
                        let model = client.completion_model(session.model.to_string());
                        *agent = agent::build_agent(model, cli, cfg, context, *todo_tools_enabled);
                        renderer.write_line(
                            &format!(
                                "todo tools: {}",
                                if *todo_tools_enabled { "on" } else { "off" }
                            ),
                            C_AGENT,
                        )?;
                    }
                }
            } else {
                renderer.write_line(&format!("unknown feature: {}", parts[1]), C_ERROR)?;
            }
        }
        "/compress" | "/compact" => {
            let instructions = if parts.len() > 1 {
                Some(parts[1..].join(" "))
            } else {
                None
            };
            let instr_str = instructions.clone().unwrap_or_default();
            return Err(anyhow::anyhow!("DEFER_COMPRESS:{}", instr_str));
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
            let last_user = session
                .messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::User)
                .cloned();
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
            renderer.write_line(
                "  /sessions <id>         load a session (by ID prefix)",
                C_RESULT,
            )?;
            renderer.write_line("  /sessions delete <id>  delete a session", C_RESULT)?;
            renderer.write_line(
                "  /reasoning             toggle reasoning visibility",
                C_RESULT,
            )?;
            renderer.write_line("  /toggle <f> [on|off]  toggle features (todo)", C_RESULT)?;
            renderer.write_line("  /clear                 clear screen", C_RESULT)?;
            renderer.write_line("  /undo                  undo last exchange", C_RESULT)?;
            renderer.write_line("  /retry                 retry last prompt", C_RESULT)?;
            renderer.write_line(
                "  /compress [/compact]   compress conversation history",
                C_RESULT,
            )?;
            renderer.write_line(
                "  /compress [instr]      compress with custom instructions",
                C_RESULT,
            )?;
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
            renderer.write_line(
                &format!("unknown command: {} (try /help)", parts[0]),
                C_ERROR,
            )?;
        }
    }
    Ok(())
}
