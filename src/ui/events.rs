use chrono::Datelike;
use compact_str::CompactString;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::session::{MessageRole, Session};
use crate::ui::markdown;
use crate::ui::renderer::{LineColor, Renderer};

pub fn format_time(rfc3339: &str) -> CompactString {
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

pub fn render_session(
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
    renderer.write_line(&welcome, LineColor::Heading)?;
    renderer.write_line("", LineColor::AgentText)?;
    if context.agents.is_some() {
        renderer.write_line("loaded AGENTS.md", LineColor::Secondary)?;
        renderer.write_line("", LineColor::AgentText)?;
    }
    #[cfg(feature = "archmd")]
    if context.architecture.is_some() {
        renderer.write_line("loaded ARCHITECTURE.md", LineColor::Secondary)?;
        renderer.write_line("", LineColor::AgentText)?;
    }
    if !session.compactions.is_empty() {
        renderer.write_line(
            &format!(
                "compacted {} times (saved ~{} tokens)",
                session.compactions.len(),
                session
                    .compactions
                    .last()
                    .map(|c| c.token_savings)
                    .unwrap_or(0),
            ),
            LineColor::Secondary,
        )?;
        renderer.write_line("", LineColor::AgentText)?;
    }
    for msg in &session.messages {
        let (prefix, _c) = match msg.role {
            MessageRole::User => (">", LineColor::PromptMarker),
            MessageRole::Assistant => ("<", LineColor::AgentText),
            MessageRole::System => ("#", LineColor::Secondary),
        };
        if msg.role == MessageRole::Assistant {
            let max_width = renderer.line_width();
            let mut styled = markdown::markdown_to_styled(&msg.content, max_width);
            if !styled.is_empty() {
                styled[0].text = CompactString::from(format!("{} {}", prefix, styled[0].text));
            }
            for entry in styled {
                renderer.write_line(&entry.text, entry.color)?;
            }
        } else {
            for line in msg.content.lines() {
                renderer.write_line(&format!("{} {}", prefix, line), _c)?;
            }
        }
        renderer.write_line("", LineColor::AgentText)?;
    }
    Ok(())
}

pub fn show_welcome(renderer: &mut Renderer) -> std::io::Result<()> {
    renderer.write_line(
        "──────────────────────────────────────────",
        LineColor::Heading,
    )?;
    renderer.write_line("  zerostack Quickstart", LineColor::Heading)?;
    renderer.write_line(
        "──────────────────────────────────────────",
        LineColor::Heading,
    )?;
    renderer.write_line("", LineColor::AgentText)?;
    renderer.write_line("  Pickers:", LineColor::ToolCall)?;
    renderer.write_line(
        "    @<path>     File picker / auto-complete paths",
        LineColor::AgentText,
    )?;
    renderer.write_line(
        "    !<command>  Run a shell command (output stored as assistant)",
        LineColor::AgentText,
    )?;
    renderer.write_line(
        "    .<prompt>   Switch prompt or one-shot .<prompt> <message>",
        LineColor::AgentText,
    )?;
    renderer.write_line("", LineColor::AgentText)?;
    renderer.write_line("  Slash Commands:", LineColor::ToolCall)?;
    renderer.write_line("    /model        Switch model", LineColor::AgentText)?;
    renderer.write_line(
        "    /prompt       List / activate prompts",
        LineColor::AgentText,
    )?;
    renderer.write_line(
        "    .autoconfig        Switches to auto-configurator",
        LineColor::AgentText,
    )?;
    renderer.write_line(
        "    /mode         Change security mode",
        LineColor::AgentText,
    )?;
    renderer.write_line("    /clear        Clear session", LineColor::AgentText)?;
    renderer.write_line("    /undo         Undo last exchange", LineColor::AgentText)?;
    renderer.write_line(
        "    /compress     Free context window space",
        LineColor::AgentText,
    )?;
    renderer.write_line("    /help         Show all commands", LineColor::AgentText)?;
    renderer.write_line("", LineColor::AgentText)?;
    renderer.write_line("  Keybindings:", LineColor::ToolCall)?;
    renderer.write_line("    Ctrl+G     Open input in $EDITOR", LineColor::AgentText)?;
    renderer.write_line("    Ctrl+H     Launch lazygit", LineColor::AgentText)?;
    renderer.write_line("    Ctrl+S     Save session", LineColor::AgentText)?;
    renderer.write_line(
        "    Tab        File picker / auto-complete",
        LineColor::AgentText,
    )?;
    renderer.write_line(
        "  Website: https://gi-dellav.github.io/zerostack/",
        LineColor::AgentText,
    )?;
    renderer.write_line("", LineColor::AgentText)?;
    renderer.write_line(
        "──────────────────────────────────────────",
        LineColor::Heading,
    )?;
    renderer.write_line("", LineColor::AgentText)?;
    Ok(())
}

pub fn sanitize_output(text: &str) -> CompactString {
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
