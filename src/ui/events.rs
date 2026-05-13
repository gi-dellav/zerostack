use chrono::Datelike;
use compact_str::CompactString;
use crossterm::style::Color;

use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::session::{MessageRole, Session};
use crate::ui::renderer::Renderer;

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
    renderer.write_line(&welcome, Color::Cyan)?;
    renderer.write_line("", Color::White)?;
    if context.agents.is_some() {
        renderer.write_line("loaded AGENTS.md", Color::DarkGrey)?;
        renderer.write_line("", Color::White)?;
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
            Color::DarkGrey,
        )?;
        renderer.write_line("", Color::White)?;
    }
    for msg in &session.messages {
        let (prefix, c) = match msg.role {
            MessageRole::User => (">", Color::Green),
            MessageRole::Assistant => ("<", Color::White),
            MessageRole::System => ("#", Color::DarkGrey),
        };
        for line in msg.content.lines() {
            renderer.write_line(&format!("{} {}", prefix, line), c)?;
        }
        renderer.write_line("", Color::White)?;
    }
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
