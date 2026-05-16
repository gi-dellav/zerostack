use compact_str::CompactString;
use crossterm::style::Color;
use smallvec::SmallVec;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::permission::SecurityMode;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::provider::{AnyAgent, AnyClient};
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
    agent: &mut AnyAgent,
    client: &AnyClient,
    renderer: &mut Renderer,
    session: &mut Session,
    cli: &Cli,
    cfg: &Config,
    context: &mut ContextFiles,
    todo_tools_enabled: &mut bool,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
) -> anyhow::Result<()> {
    renderer.write_line("compressing...", C_AGENT)?;
    renderer.write_line("", Color::White)?;

    let reserve = cfg.resolve_reserve_tokens();
    let keep_recent = cfg.resolve_keep_recent_tokens();
    let max_tokens = session.context_window.saturating_sub(reserve);

    if session.total_estimated_tokens <= max_tokens {
        renderer.write_line("context within limits, no compression needed", C_AGENT)?;
        return Ok(());
    }

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

    let summary = client
        .compress_messages(
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

    session.compress(summary, cut_idx, tokens_before);

    let model = client.completion_model(session.model.to_string());
    *agent = crate::provider::build_agent(
        model,
        cli,
        cfg,
        context,
        *todo_tools_enabled,
        permission.clone(),
        ask_tx.clone(),
    );

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
    agent: &mut AnyAgent,
    client: &AnyClient,
    renderer: &mut Renderer,
    session: &mut Session,
    cli: &Cli,
    cfg: &Config,
    context: &mut ContextFiles,
    show_reasoning: &mut bool,
    is_running: &mut bool,
    input: &mut InputEditor,
    todo_tools_enabled: &mut bool,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    #[cfg(feature = "loop")] loop_state: &mut Option<crate::extras::r#loop::LoopState>,
) -> anyhow::Result<()> {
    let parts: SmallVec<[&str; 3]> = text.trim().splitn(3, ' ').collect();
    match parts[0] {
        "/model" => {
            if parts.len() < 2 {
                renderer.write_line(&format!("current model: {}", session.model), C_AGENT)?;
            } else {
                let new_model = CompactString::new(parts[1].trim());
                let model = client.completion_model(new_model.to_string());
                *agent = crate::provider::build_agent(
                    model,
                    cli,
                    cfg,
                    context,
                    *todo_tools_enabled,
                    permission.clone(),
                    ask_tx.clone(),
                );
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
        "/mode" => {
            let current_mode = permission
                .as_ref()
                .map(|p| p.lock().unwrap().mode())
                .unwrap_or(SecurityMode::Standard);

            if parts.len() < 2 {
                renderer.write_line("security mode:", C_AGENT)?;
                renderer.write_line(&format!("  current: {}", current_mode), C_RESULT)?;
                renderer.write_line("", C_AGENT)?;
                renderer.write_line(
                    "  /mode standard      use configured permission rules",
                    C_RESULT,
                )?;
                renderer.write_line("  /mode restrictive   default all tools to ask", C_RESULT)?;
                renderer.write_line(
                    "  /mode accept        auto-accept within working directory",
                    C_RESULT,
                )?;
                renderer
                    .write_line("  /mode yolo          auto-accept ALL operations", C_RESULT)?;
                renderer.write_line("", C_AGENT)?;
                renderer.write_line("  /mode todo [on|off] toggle todo tools", C_RESULT)?;
            } else {
                match parts[1] {
                    "standard" => {
                        if let Some(p) = permission {
                            p.lock().unwrap().set_mode(SecurityMode::Standard);
                            renderer.write_line("security mode: standard", C_AGENT)?;
                        } else {
                            renderer.write_line("permission system not active", C_ERROR)?;
                        }
                    }
                    "restrictive" => {
                        if let Some(p) = permission {
                            p.lock().unwrap().set_mode(SecurityMode::Restrictive);
                            renderer.write_line("security mode: restrictive", C_AGENT)?;
                        } else {
                            renderer.write_line("permission system not active", C_ERROR)?;
                        }
                    }
                    "accept" => {
                        if let Some(p) = permission {
                            p.lock().unwrap().set_mode(SecurityMode::Accept);
                            renderer.write_line(
                                "security mode: accept (auto-allow within CWD)",
                                C_AGENT,
                            )?;
                        } else {
                            renderer.write_line("permission system not active", C_ERROR)?;
                        }
                    }
                    "yolo" => {
                        if let Some(p) = permission {
                            p.lock().unwrap().set_mode(SecurityMode::Yolo);
                            renderer.write_line(
                                "security mode: YOLO (all operations allowed)",
                                C_AGENT,
                            )?;
                        } else {
                            renderer.write_line("permission system not active", C_ERROR)?;
                        }
                    }
                    "todo" => {
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
                                *agent = crate::provider::build_agent(
                                    model,
                                    cli,
                                    cfg,
                                    context,
                                    *todo_tools_enabled,
                                    permission.clone(),
                                    ask_tx.clone(),
                                );
                                renderer.write_line(
                                    &format!(
                                        "todo tools: {}",
                                        if *todo_tools_enabled { "on" } else { "off" }
                                    ),
                                    C_AGENT,
                                )?;
                            }
                        }
                    }
                    _ => {
                        renderer.write_line(&format!("unknown mode: {}", parts[1]), C_ERROR)?;
                    }
                }
            }
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
                        *agent = crate::provider::build_agent(
                            model,
                            cli,
                            cfg,
                            context,
                            *todo_tools_enabled,
                            permission.clone(),
                            ask_tx.clone(),
                        );
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
        "/loop" => {
            #[cfg(feature = "loop")]
            {
                if parts.len() < 2 || (parts.len() >= 2 && parts[1] == "status") {
                    if let Some(ls) = loop_state {
                        let status = if ls.active { "active" } else { "stopped" };
                        renderer.write_line(
                            &format!(
                                "loop {}: {} ({})",
                                status,
                                ls.iteration_label(),
                                ls.plan_file.display()
                            ),
                            C_AGENT,
                        )?;
                    } else {
                        renderer.write_line("no active loop", C_AGENT)?;
                        renderer.write_line("usage: /loop <prompt>  |  /loop stop", C_RESULT)?;
                    }
                } else if parts[1] == "stop" {
                    if let Some(ls) = loop_state {
                        ls.active = false;
                        renderer.write_line("loop stopped", C_AGENT)?;
                    } else {
                        renderer.write_line("no active loop", C_AGENT)?;
                    }
                } else {
                    let prompt = parts[1..].join(" ");
                    if prompt.is_empty() {
                        renderer.write_line("usage: /loop <prompt>", C_ERROR)?;
                        return Ok(());
                    }
                    let plan_file = std::path::PathBuf::from("LOOP_PLAN.md");
                    let ls = crate::extras::r#loop::LoopState::new(
                        prompt, plan_file, None, None,
                    );
                    *loop_state = Some(ls);
                    *is_running = true;
                    renderer.write_line(
                        "loop started — iteration 1 will run after this message",
                        C_AGENT,
                    )?;
                }
            }
            #[cfg(not(feature = "loop"))]
            {
                renderer.write_line(
                    "/loop requires the 'loop' feature: cargo build --features loop",
                    C_ERROR,
                )?;
            }
        }
        "/prompt" => {
            let mut sorted: Vec<&String> = context.prompts.keys().collect();
            sorted.sort();
            if parts.len() < 2 {
                if sorted.is_empty() {
                    renderer.write_line("no prompts available", C_AGENT)?;
                } else {
                    let current = context.current_prompt.as_deref().unwrap_or("(none)");
                    renderer.write_line(
                        &format!("available prompts (current: {}):", current),
                        C_AGENT,
                    )?;
                    for name in &sorted {
                        renderer.write_line(&format!("  {}", name), C_RESULT)?;
                    }
                    renderer.write_line("", C_AGENT)?;
                    renderer.write_line("usage: /prompt <name>  |  /prompt default", C_RESULT)?;
                }
            } else if parts[1] == "default" {
                if context.current_prompt.is_none() {
                    renderer.write_line("no active prompt to clear", C_AGENT)?;
                } else {
                    context.current_prompt = None;
                    let model = client.completion_model(session.model.to_string());
                    *agent = crate::provider::build_agent(
                        model,
                        cli,
                        cfg,
                        context,
                        *todo_tools_enabled,
                        permission.clone(),
                        ask_tx.clone(),
                    );
                    renderer.write_line("prompt cleared (back to default behavior)", C_AGENT)?;
                }
            } else {
                let name = parts[1].trim();
                if let Some(content) = context.prompts.get(name) {
                    context.current_prompt = Some(content.clone());
                    let model = client.completion_model(session.model.to_string());
                    *agent = crate::provider::build_agent(
                        model,
                        cli,
                        cfg,
                        context,
                        *todo_tools_enabled,
                        permission.clone(),
                        ask_tx.clone(),
                    );
                    renderer.write_line(
                        &format!("active prompt: {}", name),
                        C_AGENT,
                    )?;
                } else {
                    renderer.write_line(
                        &format!("unknown prompt: '{}'", name),
                        C_ERROR,
                    )?;
                    if !sorted.is_empty() {
                        renderer.write_line("available prompts:", C_AGENT)?;
                        for p in &sorted {
                            renderer.write_line(&format!("  {}", p), C_RESULT)?;
                        }
                    }
                }
            }
        }
        "/regen-prompts" => {
            match crate::context::prompts::regen() {
                Ok(()) => {
                    context.prompts = crate::context::prompts::load();
                    renderer.write_line("default prompts regenerated", C_AGENT)?;
                }
                Err(e) => {
                    renderer.write_line(&format!("failed to regenerate prompts: {}", e), C_ERROR)?;
                }
            }
        }
        "/quit" => {
            *is_running = false;
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "quit").into());
        }
        "/clear" => {
            session.messages.clear();
            session.total_estimated_tokens = 0;
            session.compactions.clear();
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
            renderer.write_line(
                "  /mode                  show/change security mode",
                C_RESULT,
            )?;
            renderer.write_line(
                "  /mode <mode>           set mode (standard|restrictive|accept|yolo)",
                C_RESULT,
            )?;
            renderer.write_line("  /toggle <f> [on|off]   toggle features (todo)", C_RESULT)?;
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
            #[cfg(feature = "loop")]
            {
                let _ = renderer.write_line("  /loop [prompt]         start iterative coding loop", C_RESULT);
                let _ = renderer.write_line("  /loop stop             stop the loop", C_RESULT);
            }
            #[cfg(not(feature = "loop"))]
            {
                let _ = renderer.write_line("  /loop [prompt]         start iterative coding loop (req. 'loop' feature)", C_RESULT);
            }
            renderer.write_line("  /prompt                list available prompts", C_RESULT)?;
            renderer.write_line("  /prompt <name>         activate a prompt", C_RESULT)?;
            renderer.write_line("  /prompt default        clear active prompt", C_RESULT)?;
            renderer.write_line(
                "  /regen-prompts        restore built-in prompts to global dir",
                C_RESULT,
            )?;
            renderer.write_line("  /quit                  exit zerostack", C_RESULT)?;
            renderer.write_line("  /help                  show this message", C_RESULT)?;
            renderer.write_line("", C_AGENT)?;
            renderer.write_line("keys:", C_AGENT)?;
            renderer.write_line("  PgUp/PgDn             scroll chat history", C_RESULT)?;
            renderer.write_line("  Home/End               jump to top/bottom", C_RESULT)?;
            renderer.write_line(
                "  @<query>               file picker (Tab/Enter select, Esc cancel)",
                C_RESULT,
            )?;
            renderer.write_line(
                "  mouse drag             select text (copies to clipboard on release)",
                C_RESULT,
            )?;
            renderer.write_line("  Esc (while selected)   clear selection (no copy)", C_RESULT)?;
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
