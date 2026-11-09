use std::path::PathBuf;

use crate::ui::slash::{SlashCtx, write_error, write_ok, write_result};

pub(crate) fn resolve_path(s: &str) -> PathBuf {
    let p = PathBuf::from(s);
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    }
}

#[cfg(feature = "multimodal")]
fn has_pending_media(ctx: &SlashCtx<'_>) -> bool {
    !ctx.session.pending_media.is_empty()
}

#[cfg(not(feature = "multimodal"))]
fn has_pending_media(_ctx: &SlashCtx<'_>) -> bool {
    false
}

/// Whether any attachment is recorded in the transcript.
#[cfg(feature = "multimodal")]
fn has_transcript_media(ctx: &SlashCtx<'_>) -> bool {
    ctx.session.messages.iter().any(|m| !m.media.is_empty())
}

#[cfg(not(feature = "multimodal"))]
fn has_transcript_media(_ctx: &SlashCtx<'_>) -> bool {
    false
}

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match parts[0] {
        "/add" => handle_add(parts, ctx).await,
        "/drop" => handle_drop(parts, ctx).await,
        "/drop-all" => handle_drop_all(ctx).await,
        _ => Ok(()),
    }
}

async fn handle_add(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 {
        if ctx.context.extra_files.is_empty()
            && !has_pending_media(ctx)
            && !has_transcript_media(ctx)
        {
            write_ok(ctx.renderer, "no files added (use /add <path>)");
        } else {
            write_ok(ctx.renderer, "added files:");
            for f in &ctx.context.extra_files {
                let size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
                write_result(ctx.renderer, format!("  {} ({size}B)", f.display()));
            }
            #[cfg(feature = "multimodal")]
            for m in &ctx.session.pending_media {
                let kind = m.kind().map(|k| k.label()).unwrap_or("media");
                let size = std::fs::metadata(&m.path).map(|m| m.len()).unwrap_or(0);
                write_result(
                    ctx.renderer,
                    format!("  [{kind}] {} ({size}B, pending)", m.path.display()),
                );
            }
            // The attachments already in the transcript: they are sent with
            // every later turn, so they are worth showing (and dropping).
            #[cfg(feature = "multimodal")]
            for msg in &ctx.session.messages {
                for m in &msg.media {
                    let kind = m.kind().map(|k| k.label()).unwrap_or("media");
                    write_result(
                        ctx.renderer,
                        format!("  [{kind}] {} (in transcript)", m.path.display()),
                    );
                }
            }
        }
        return Ok(());
    }

    // The rest of the line is the path, so a path with spaces works.
    let path = resolve_path(&parts[1..].join(" "));

    if !path.exists() {
        write_error(ctx.renderer, format!("file not found: {}", path.display()));
        return Ok(());
    }
    if !path.is_file() {
        write_error(ctx.renderer, format!("not a file: {}", path.display()));
        return Ok(());
    }

    #[cfg(feature = "multimodal")]
    if crate::extras::multimodal::detect_media(&path).is_some() {
        match crate::extras::multimodal::attach(&path) {
            Ok(media) => {
                let support = crate::extras::multimodal::ProviderSupport {
                    name: ctx.client.provider_name(),
                    audio: ctx
                        .client
                        .completion_model(ctx.session.model.to_string())
                        .supports_audio(),
                };
                if let Err(e) = crate::extras::multimodal::validate_for_provider(support, &media) {
                    write_error(ctx.renderer, format!("cannot attach: {e}"));
                    return Ok(());
                }
                let size = std::fs::metadata(&media.path).map(|m| m.len()).unwrap_or(0);
                let mime_str = media.mime.clone();
                ctx.session.pending_media.push(media);
                write_ok(
                    ctx.renderer,
                    format!("attached: {} ({mime_str}, {size}B)", path.display()),
                );
            }
            Err(e) => {
                write_error(ctx.renderer, format!("failed to load media: {e}"));
            }
        }
        return Ok(());
    }

    let canonical = path.canonicalize().unwrap_or(path);
    if ctx.context.extra_files.contains(&canonical) {
        write_ok(
            ctx.renderer,
            format!("already added: {}", canonical.display()),
        );
        return Ok(());
    }

    let size = std::fs::metadata(&canonical).map(|m| m.len()).unwrap_or(0);
    ctx.context.extra_files.push(canonical.clone());
    ctx.rebuild_agent().await;
    write_ok(
        ctx.renderer,
        format!("added: {} ({size}B)", canonical.display()),
    );
    Ok(())
}

async fn handle_drop(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 {
        write_error(ctx.renderer, "usage: /drop <path-or-index>");
        return Ok(());
    }

    // A bare number is an index into the pending attachments; anything else
    // is a path, and the rest of the line belongs to it.
    #[cfg(feature = "multimodal")]
    if let Ok(idx) = parts[1].parse::<usize>()
        && idx < ctx.session.pending_media.len()
    {
        let removed = ctx.session.pending_media.remove(idx);
        write_ok(
            ctx.renderer,
            format!("dropped media: {}", removed.path.display()),
        );
        return Ok(());
    }

    let path = resolve_path(&parts[1..].join(" "));
    let canonical = path.canonicalize().unwrap_or(path);

    // Try extra_files first.
    if let Some(i) = ctx.context.extra_files.iter().position(|f| f == &canonical) {
        ctx.context.extra_files.remove(i);
        ctx.rebuild_agent().await;
        write_ok(ctx.renderer, format!("dropped: {}", canonical.display()));
        return Ok(());
    }

    // Try pending_media by path match.
    #[cfg(feature = "multimodal")]
    {
        let canonical_str = canonical.to_string_lossy();
        if let Some(i) = ctx
            .session
            .pending_media
            .iter()
            .position(|m| m.path.to_string_lossy() == canonical_str)
        {
            ctx.session.pending_media.remove(i);
            write_ok(
                ctx.renderer,
                format!("dropped media: {}", canonical.display()),
            );
            return Ok(());
        }
        // The attachment may already be part of the conversation: drop it
        // from the recorded messages too.
        let dropped = ctx.session.drop_media_path(&canonical);
        if dropped > 0 {
            write_ok(
                ctx.renderer,
                format!(
                    "dropped media: {} ({dropped} in transcript)",
                    canonical.display()
                ),
            );
            return Ok(());
        }
    }

    write_error(
        ctx.renderer,
        format!("not in context: {} (use /add to see)", canonical.display()),
    );
    Ok(())
}

async fn handle_drop_all(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let file_count = ctx.context.extra_files.len();
    #[cfg(feature = "multimodal")]
    // `drop_all_media` clears the transcript attachments as it counts them.
    let media_count = ctx.session.pending_media.len() + ctx.session.drop_all_media();
    #[cfg(not(feature = "multimodal"))]
    let media_count = 0;

    if file_count == 0 && media_count == 0 {
        write_ok(ctx.renderer, "no files or media to drop");
        return Ok(());
    }

    if file_count > 0 {
        ctx.context.extra_files.clear();
        ctx.rebuild_agent().await;
    }
    #[cfg(feature = "multimodal")]
    {
        ctx.session.pending_media.clear();
    }

    let mut parts = vec![];
    if file_count > 0 {
        parts.push(format!("{file_count} file(s)"));
    }
    if media_count > 0 {
        parts.push(format!("{media_count} media"));
    }
    write_ok(ctx.renderer, format!("dropped {}", parts.join(", ")));
    Ok(())
}
