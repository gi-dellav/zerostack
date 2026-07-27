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

/// The closing lines of a fresh session's banner block. When a startup log
/// is in play they are deferred until the background prebuild finishes, so
/// they render after the live MCP lines instead of before them.
pub(crate) const READY_LINES: [&str; 2] = [
    "Ready to code; type a request or '/' for commands",
    "Run /welcome or /tutor to get started",
];

pub fn render_session(
    renderer: &mut Renderer,
    session: &Session,
    _cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
) -> anyhow::Result<()> {
    render_session_with_boot(renderer, session, _cli, cfg, context, None)
}

/// [`render_session`] plus the startup log: at initial launch the collected
/// boot lines (config source, context, provider, …) are replayed into the
/// feed between the banner and the "ready" lines, so the logo comes first
/// and the checklist reads in chronological order.
pub fn render_session_with_boot(
    renderer: &mut Renderer,
    session: &Session,
    _cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    boot_log: Option<&crate::ui::boot::BootState>,
) -> anyhow::Result<()> {
    renderer.clear_content()?;
    let feed = renderer.feed_mut();
    // With a startup log, the loaded context files are reported by its
    // "Context files" line instead of standalone system lines; without one
    // (slash-command re-renders, quiet startup) keep the system lines.
    let show_context_system_lines = !session.messages.is_empty() || boot_log.is_none();
    if context.agents.is_some() && show_context_system_lines {
        feed.push_line(BlockStyle::System, "[system] loaded AGENTS.md");
        feed.push_line(BlockStyle::Plain, "");
    }
    #[cfg(feature = "archmd")]
    if context.architecture.is_some() && show_context_system_lines {
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
    if let Some(log) = boot_log
        && !session.messages.is_empty()
    {
        // Resumed session: the startup log is fresh history, append it after
        // the replayed transcript.
        feed.push_line(BlockStyle::Welcome, "Startup");
        for (text, color) in log.render_lines() {
            feed.push_colored_line(color, text);
        }
        feed.push_line(BlockStyle::Plain, "");
    }
    if session.messages.is_empty() {
        // Minimal banner: name + version only. Model, context window,
        // prompt, branch and cwd all live in the statusline, so repeating
        // them here would be noise.
        feed.push_line(
            BlockStyle::Welcome,
            format!("zerostack v{}", env!("CARGO_PKG_VERSION")),
        );
        if let Some(log) = boot_log {
            feed.push_line(BlockStyle::Plain, "");
            feed.push_line(BlockStyle::Welcome, "Startup");
            for (text, color) in log.render_lines() {
                feed.push_colored_line(color, text);
            }
        }
        // With a startup log the ready lines are deferred: the TUI appends
        // them once the background prebuild (MCP, agent) reports ready.
        if boot_log.is_none() {
            for line in READY_LINES {
                feed.push_line(BlockStyle::Welcome, line);
            }
        }
        feed.push_line(BlockStyle::Plain, "");
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_context(prompt_name: Option<&str>) -> ContextFiles {
        ContextFiles {
            agents: None,
            prompts: HashMap::new(),
            current_prompt: None,
            current_prompt_name: prompt_name.map(str::to_string),
            themes: HashMap::new(),
            current_theme_name: None,
            extra_files: Vec::new(),
            one_shot_restore: None,
            chain_declined: Vec::new(),
            #[cfg(feature = "memory")]
            memory: None,
            #[cfg(feature = "archmd")]
            architecture: None,
        }
    }

    #[test]
    fn ready_lines_deferred_when_boot_log_present() {
        use crate::ui::boot::BootState;
        use crate::ui::renderer::Renderer;

        let cli = Cli::default();
        let cfg = Config::default();
        let context = test_context(Some("code"));
        let mut session = Session::new("anthropic", "claude-opus-4", 200_000, "");
        session.working_dir = CompactString::new("/tmp/proj");

        // Without a startup log, the ready lines close the banner directly.
        let mut renderer = Renderer::new().unwrap();
        render_session_with_boot(&mut renderer, &session, &cli, &cfg, &context, None).unwrap();
        let texts: Vec<String> = renderer
            .feed_mut()
            .lines(100)
            .iter()
            .map(|l| l.text.to_string())
            .collect();
        assert!(texts.iter().any(|l| l.contains("Ready to code")));
        assert!(texts[0].starts_with("zerostack v"));

        // With one, they are deferred (the TUI appends them after the
        // prebuild's MCP/agent-ready lines).
        let mut log = BootState::default();
        log.loaded("Configuration", "~/.zerostack/config/config.toml");
        let mut renderer = Renderer::new().unwrap();
        render_session_with_boot(&mut renderer, &session, &cli, &cfg, &context, Some(&log))
            .unwrap();
        let texts: Vec<String> = renderer
            .feed_mut()
            .lines(100)
            .iter()
            .map(|l| l.text.to_string())
            .collect();
        assert!(!texts.iter().any(|l| l.contains("Ready to code")));
        // Banner first, then the startup checklist.
        assert!(texts[0].starts_with("zerostack v"));
        let banner_pos = 0;
        let checklist_pos = texts
            .iter()
            .position(|l| l.contains("✓ Configuration"))
            .expect("checklist line present");
        assert!(checklist_pos > banner_pos);
    }
}
