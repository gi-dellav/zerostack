use std::path::PathBuf;

use crate::ui::slash::{SlashCtx, write_error, write_ok, write_result};

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match parts[0] {
        "/image" => handle_image(parts, ctx).await,
        "/image-drop" => handle_image_drop(parts, ctx).await,
        _ => Ok(()),
    }
}

async fn handle_image(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 {
        if ctx.input.image_attachments.is_empty() {
            ctx.input.start_image_picker();
            write_ok(ctx.renderer, "opening file picker to select an image");
        } else {
            write_ok(ctx.renderer, "attached images:");
            for (i, path) in ctx.input.image_attachments.iter().enumerate() {
                write_result(ctx.renderer, format!("  {}. {}", i + 1, path.display()));
            }
            write_ok(
                ctx.renderer,
                "type /image-drop <n> to remove, or /image <path> to add",
            );
        }
        return Ok(());
    }

    let path = PathBuf::from(parts[1]);
    if !path.exists() {
        write_error(ctx.renderer, format!("file not found: {}", path.display()));
        return Ok(());
    }
    if !path.is_file() {
        write_error(ctx.renderer, format!("not a file: {}", path.display()));
        return Ok(());
    }

    let canonical = path.canonicalize().unwrap_or(path);
    if !super::is_image_path(&canonical) {
        write_error(
            ctx.renderer,
            format!(
                "not a supported image format: {} (supported: png, jpg, jpeg, gif, webp, svg, heic, heif)",
                canonical.display()
            ),
        );
        return Ok(());
    }

    if ctx.input.image_attachments.contains(&canonical) {
        write_ok(
            ctx.renderer,
            format!("already attached: {}", canonical.display()),
        );
        return Ok(());
    }

    ctx.input.image_attachments.push(canonical.clone());
    write_ok(ctx.renderer, format!("attached: {}", canonical.display()));
    Ok(())
}

async fn handle_image_drop(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 {
        write_error(ctx.renderer, "usage: /image-drop <n>");
        return Ok(());
    }

    let idx: usize = match parts[1].parse::<usize>() {
        Ok(n) if n > 0 => n - 1,
        _ => {
            write_error(ctx.renderer, "expected a number (e.g. /image-drop 1)");
            return Ok(());
        }
    };

    if idx >= ctx.input.image_attachments.len() {
        write_error(
            ctx.renderer,
            format!(
                "index out of range (1-{})",
                ctx.input.image_attachments.len()
            ),
        );
        return Ok(());
    }

    let removed = ctx.input.image_attachments.remove(idx);
    write_ok(ctx.renderer, format!("removed: {}", removed.display()));
    Ok(())
}
