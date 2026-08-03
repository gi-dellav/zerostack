use chrono::Datelike;
use compact_str::CompactString;

use crate::cli::Cli;
use crate::config::{Config, ResolvedShowToolDetails};
use crate::context::ContextFiles;
use crate::session::{MessageRole, Session};
use crate::ui::feed::BlockStyle;
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
    // Rebuild the feed from the session. Already-printed lines cannot be
    // unprinted; the watermark reconciliation in `note_feed_rebuilt` makes
    // the rebuilt feed print exactly once (full transcript at startup,
    // nothing for a rebuild over an already-printed feed).
    renderer.reset_feed_for_rebuild();
    let feed = renderer.feed_mut();
    if context.agents.is_some() {
        feed.push_line(BlockStyle::System, "[system] loaded AGENTS.md");
        feed.push_line(BlockStyle::Plain, "");
    }
    #[cfg(feature = "archmd")]
    if context.architecture.is_some() {
        feed.push_line(BlockStyle::System, "[system] loaded ARCHITECTURE.md");
        feed.push_line(BlockStyle::Plain, "");
    }
    if !session.compactions.is_empty() {
        feed.push_line(
            BlockStyle::System,
            format!(
                "compacted {} times (saved ~{} tokens)",
                session.compactions.len(),
                session
                    .compactions
                    .last()
                    .map(|c| c.token_savings)
                    .unwrap_or(0),
            ),
        );
        feed.push_line(BlockStyle::Plain, "");
    }
    for msg in &session.messages {
        match msg.role {
            MessageRole::User => {
                for line in msg.content.lines() {
                    feed.push_line(BlockStyle::User, format!("> {}", line));
                }
            }
            MessageRole::Assistant => {
                feed.push_block(BlockStyle::Agent, msg.content.to_string());
            }
            MessageRole::System => {
                for line in msg.content.lines() {
                    feed.push_line(BlockStyle::System, format!("# {}", line));
                }
            }
            MessageRole::ToolCall => {
                for line in msg.content.lines() {
                    feed.push_line(BlockStyle::Tool, format!("◈ {}", line));
                }
            }
            MessageRole::ToolResult => {
                render_tool_result_to_feed(feed, &msg.content, cfg)?;
            }
            MessageRole::SubagentToolCall => {
                for line in msg.content.lines() {
                    feed.push_line(BlockStyle::Tool, format!("⌥ {}", line));
                }
            }
        }
        feed.push_line(BlockStyle::Plain, "");
    }
    if session.messages.is_empty() {
        let cwd = std::env::current_dir().ok();
        let cwd_str = cwd
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(".");
        feed.push_line(
            BlockStyle::Welcome,
            format!(
                "[>] zerostack {} | {} | {}",
                env!("CARGO_PKG_VERSION"),
                cli.resolve_model(cfg),
                cwd_str,
            ),
        );
        feed.push_line(
            BlockStyle::Welcome,
            "──────────────────────────────────────────────────",
        );
        feed.push_line(
            BlockStyle::Welcome,
            "Ready to code; type a request or '/' for commands",
        );
        feed.push_line(BlockStyle::Welcome, "Run /welcome or /tutor to get started");
        feed.push_line(BlockStyle::Plain, "");
        feed.push_line(BlockStyle::Plain, "");
    }
    renderer.note_feed_rebuilt();
    Ok(())
}

fn render_tool_result_to_feed(
    feed: &mut crate::ui::feed::Feed,
    content: &str,
    cfg: &Config,
) -> anyhow::Result<()> {
    let output = content
        .split_once(":\n")
        .map(|(_, output)| output)
        .unwrap_or(content);
    let show_details = cfg
        .show_tool_details
        .as_ref()
        .map(|s| s.resolve())
        .unwrap_or(ResolvedShowToolDetails::Limited(3));
    match show_details {
        ResolvedShowToolDetails::Off => {
            feed.push_line(
                BlockStyle::ToolResult,
                "◈ result hidden by show_tool_details=false",
            );
        }
        ResolvedShowToolDetails::Limited(max_lines) => {
            let sanitized = sanitize_output(output);
            let char_count = sanitized.chars().count();
            let lines: Vec<&str> = sanitized.lines().collect();
            if lines.len() > max_lines {
                let shown = lines[..max_lines].join("\n");
                feed.push_line(
                    BlockStyle::ToolResult,
                    format!(
                        "◈ result ({} chars, {} lines, showing {}):\n{}",
                        char_count,
                        lines.len(),
                        max_lines,
                        shown
                    ),
                );
            } else {
                feed.push_line(
                    BlockStyle::ToolResult,
                    format!("◈ result ({} chars):\n{}", char_count, sanitized),
                );
            }
        }
        ResolvedShowToolDetails::Unlimited => {
            let sanitized = sanitize_output(output);
            let char_count = sanitized.chars().count();
            feed.push_line(
                BlockStyle::ToolResult,
                format!("◈ result ({} chars):\n{}", char_count, sanitized),
            );
        }
    }
    Ok(())
}

pub fn show_welcome(renderer: &mut Renderer) -> std::io::Result<()> {
    let feed = renderer.feed_mut();
    feed.push_line(
        BlockStyle::Welcome,
        "──────────────────────────────────────────",
    );
    feed.push_line(BlockStyle::Welcome, "  zerostack Quickstart");
    feed.push_line(
        BlockStyle::Welcome,
        "──────────────────────────────────────────",
    );
    feed.push_line(BlockStyle::Plain, "");
    feed.push_line(BlockStyle::Tool, "  Pickers:");
    feed.push_line(
        BlockStyle::Plain,
        "    @<path>     File picker / auto-complete paths",
    );
    feed.push_line(
        BlockStyle::Plain,
        "    !<command>  Run a shell command (output stored as assistant)",
    );
    feed.push_line(
        BlockStyle::Plain,
        "    .<prompt>   Switch prompt or one-shot .<prompt> <message>",
    );
    feed.push_line(BlockStyle::Plain, "");
    feed.push_line(BlockStyle::Tool, "  Slash Commands:");
    feed.push_line(BlockStyle::Plain, "    /model        Switch model");
    feed.push_line(
        BlockStyle::Plain,
        "    /prompt       List / activate prompts",
    );
    feed.push_line(
        BlockStyle::Plain,
        "    .autoconfig        Switches to auto-configurator",
    );
    feed.push_line(BlockStyle::Plain, "    /mode         Change security mode");
    feed.push_line(BlockStyle::Plain, "    /clear        Clear session");
    feed.push_line(BlockStyle::Plain, "    /undo         Undo last exchange");
    feed.push_line(
        BlockStyle::Plain,
        "    /compress     Free context window space",
    );
    feed.push_line(BlockStyle::Plain, "    /help         Show all commands");
    feed.push_line(BlockStyle::Plain, "");
    feed.push_line(BlockStyle::Tool, "  Keybindings:");
    feed.push_line(BlockStyle::Plain, "    Ctrl+G     Open input in $EDITOR");
    feed.push_line(BlockStyle::Plain, "    Ctrl+H     Launch lazygit");
    feed.push_line(BlockStyle::Plain, "    Ctrl+S     Save session");
    feed.push_line(
        BlockStyle::Plain,
        "    Tab        File picker / auto-complete",
    );
    feed.push_line(
        BlockStyle::Plain,
        "  Website: https://gi-dellav.github.io/zerostack/",
    );
    feed.push_line(BlockStyle::Plain, "");
    feed.push_line(
        BlockStyle::Welcome,
        "──────────────────────────────────────────",
    );
    feed.push_line(BlockStyle::Plain, "");
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
