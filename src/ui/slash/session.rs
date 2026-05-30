use std::io::Read;

use crate::ui::events::render_session;
use crate::ui::slash::{SlashCtx, undo_last, write_error, write_ok, write_result};

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match parts[0] {
        "/sessions" => handle_sessions(parts, ctx).await,
        "/clear" | "/new" => handle_clear(ctx).await,
        "/undo" => handle_undo(ctx).await,
        "/retry" => handle_retry(ctx).await,
        "/quit" | "/exit" => handle_quit(ctx).await,
        "/history" => handle_history(ctx).await,
        _ => Ok(()),
    }
}

async fn handle_sessions(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 {
        let sessions = crate::session::storage::find_recent_sessions(20)?;
        if sessions.is_empty() {
            write_ok(ctx.renderer, "no saved sessions");
        } else {
            write_ok(
                ctx.renderer,
                format!("recent sessions ({}):", sessions.len()),
            );
            for s in &sessions {
                let last = s
                    .messages
                    .last()
                    .map(|m| format!("...{}", &m.content.chars().take(30).collect::<String>()))
                    .unwrap_or_default();
                let time = crate::ui::events::format_time(&s.updated_at);
                write_result(
                    ctx.renderer,
                    format!(
                        "  {}  {}  {}msgs  {}  {}",
                        &s.id[..8],
                        time,
                        s.messages.len(),
                        s.model,
                        last
                    ),
                );
            }
        }
    } else if parts[1] == "delete" && parts.len() >= 3 {
        let prefix = parts[2].trim();
        let sessions = crate::session::storage::find_sessions_by_prefix(prefix)?;
        if sessions.is_empty() {
            write_ok(ctx.renderer, format!("no session matching '{}'", prefix));
        } else if sessions.len() == 1 {
            if let Some(s) = sessions.into_iter().next() {
                let id = s.id.clone();
                let preview = s
                    .messages
                    .last()
                    .map(|m| format!("...{}", &m.content.chars().take(40).collect::<String>()))
                    .unwrap_or_default();
                if let Err(e) = crate::session::storage::delete_session(&id) {
                    write_error(ctx.renderer, format!("failed to delete: {}", e));
                } else {
                    write_ok(
                        ctx.renderer,
                        format!("deleted session {} {}", &id[..8], preview),
                    );
                }
            }
        } else {
            write_ok(
                ctx.renderer,
                format!("multiple sessions match '{}', be more specific", prefix),
            );
            for s in &sessions {
                let last = s
                    .messages
                    .last()
                    .map(|m| format!("...{}", &m.content.chars().take(30).collect::<String>()))
                    .unwrap_or_default();
                let time = crate::ui::events::format_time(&s.updated_at);
                write_result(
                    ctx.renderer,
                    format!(
                        "  {}  {}  {}msgs  {}  {}",
                        &s.id[..8],
                        time,
                        s.messages.len(),
                        s.model,
                        last
                    ),
                );
            }
        }
    } else {
        let prefix = parts[1].trim();
        let sessions = crate::session::storage::find_sessions_by_prefix(prefix)?;
        if sessions.is_empty() {
            write_ok(ctx.renderer, format!("no session matching '{}'", prefix));
        } else if sessions.len() == 1 {
            if let Some(s) = sessions.into_iter().next() {
                let msg_count = s.messages.len();
                *ctx.session = s;
                render_session(ctx.renderer, ctx.session, ctx.cli, ctx.cfg, ctx.context)?;
                write_ok(ctx.renderer, format!("loaded session ({} msgs)", msg_count));
            }
        } else {
            write_ok(
                ctx.renderer,
                format!("multiple sessions match '{}':", prefix),
            );
            for s in &sessions {
                let last = s
                    .messages
                    .last()
                    .map(|m| format!("...{}", &m.content.chars().take(30).collect::<String>()))
                    .unwrap_or_default();
                let time = crate::ui::events::format_time(&s.updated_at);
                write_result(
                    ctx.renderer,
                    format!(
                        "  {}  {}  {}msgs  {}  {}",
                        &s.id[..8],
                        time,
                        s.messages.len(),
                        s.model,
                        last
                    ),
                );
            }
        }
    }
    Ok(())
}

async fn handle_clear(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    ctx.session.messages.clear();
    ctx.session.total_estimated_tokens = 0;
    ctx.session.compactions.clear();
    render_session(ctx.renderer, ctx.session, ctx.cli, ctx.cfg, ctx.context)?;
    Ok(())
}

async fn handle_undo(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let removed = undo_last(ctx.session);
    if removed == 0 {
        write_ok(ctx.renderer, "nothing to undo");
        return Ok(());
    }

    render_session(ctx.renderer, ctx.session, ctx.cli, ctx.cfg, ctx.context)?;
    write_ok(ctx.renderer, format!("removed {} message(s)", removed));

    write_ok(ctx.renderer, "  git stash working changes? [y/N] ");

    let mut buf = [0u8; 1];
    let do_stash =
        std::io::stdin().read_exact(&mut buf).is_ok() && (buf[0] == b'y' || buf[0] == b'Y');

    if do_stash {
        match std::process::Command::new("git").args(["stash"]).output() {
            Ok(out) if out.status.success() => {
                write_ok(ctx.renderer, "git stash done");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                write_error(ctx.renderer, format!("git stash failed: {}", stderr.trim()));
            }
            Err(e) => {
                write_error(ctx.renderer, format!("git stash failed: {}", e));
            }
        }
    }

    Ok(())
}

async fn handle_retry(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let last_user = ctx
        .session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == crate::session::MessageRole::User)
        .cloned();
    match last_user {
        Some(msg) => {
            ctx.input.buffer = msg.content.clone();
            ctx.input.cursor = msg.content.len();
            write_ok(ctx.renderer, "edit last message and press Enter to retry");
        }
        None => {
            write_ok(ctx.renderer, "no previous message to retry");
        }
    }
    Ok(())
}

async fn handle_quit(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    *ctx.is_running = false;
    Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "quit").into())
}

async fn handle_history(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match crate::session::chat_history::load_history() {
        Ok(entries) => {
            if entries.is_empty() {
                write_ok(ctx.renderer, "no chat history");
            } else {
                write_ok(
                    ctx.renderer,
                    format!("global chat history ({} entries):", entries.len()),
                );
                for entry in entries.iter().rev().take(10).rev() {
                    let preview: String = entry.content.chars().take(80).collect();
                    write_result(ctx.renderer, format!("  {}", preview));
                }
                if entries.len() > 10 {
                    write_ok(ctx.renderer, "  ... (showing last 10)");
                }
            }
        }
        Err(e) => {
            write_error(ctx.renderer, format!("failed to load chat history: {}", e));
        }
    }
    Ok(())
}
