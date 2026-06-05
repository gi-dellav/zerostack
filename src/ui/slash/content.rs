use crate::context;
use crate::permission::{self, SecurityMode};
use crate::session::storage;
use crate::ui::slash::{SlashCtx, write_error, write_ok, write_result};

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match parts[0] {
        "/prompt" => handle_prompt(parts, ctx).await,
        "/theme" => handle_theme(parts, ctx).await,
        "/regen-prompts" => handle_regen_prompts(ctx).await,
        "/regen-themes" => handle_regen_themes(ctx).await,
        _ => Ok(()),
    }
}

async fn handle_prompt(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let mut sorted: Vec<&String> = ctx.context.prompts.keys().collect();
    sorted.sort();
    if parts.len() < 2 {
        if sorted.is_empty() {
            write_ok(ctx.renderer, "no prompts available");
        } else {
            let current = ctx
                .context
                .current_prompt_name
                .as_deref()
                .unwrap_or("(none)");
            write_ok(
                ctx.renderer,
                format!("available prompts (current: {}):", current),
            );
            for name in &sorted {
                write_result(ctx.renderer, format!("  {}", name));
            }
            write_result(ctx.renderer, "usage: /prompt <name>  |  /prompt default");
        }
    } else if parts[1] == "default" {
        if ctx.context.current_prompt.is_none() {
            write_ok(ctx.renderer, "no active prompt to clear");
        } else {
            ctx.context.current_prompt = None;
            ctx.context.current_prompt_name = None;
            ctx.rebuild_agent().await;
            write_ok(ctx.renderer, "prompt cleared (back to default)");
        }
    } else {
        let name = parts[1].trim();
        if let Some(content) = ctx.context.prompts.get(name) {
            let (mode_directive, clean_content) = permission::parse_prompt_mode(content);
            ctx.context.current_prompt = Some(if mode_directive.is_some() {
                clean_content.to_string()
            } else {
                content.clone()
            });
            ctx.context.current_prompt_name = Some(name.to_string());
            if let Some(mode_str) = mode_directive {
                if mode_str == "last_user_mode" {
                    if let Some(perm) = ctx.permission {
                        perm.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .restore_user_mode();
                        let current = perm
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .mode()
                            .to_string();
                        write_ok(ctx.renderer, format!("restored user mode: {}", current));
                    }
                } else if let Some(mode) = SecurityMode::from_str(mode_str)
                    && let Some(perm) = ctx.permission
                {
                    perm.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .set_prompt_mode(mode);
                    write_ok(
                        ctx.renderer,
                        format!("security mode: {} (from prompt)", mode),
                    );
                }
            }
            ctx.rebuild_agent().await;
            write_ok(ctx.renderer, format!("active prompt: {}", name));
        } else {
            write_error(ctx.renderer, format!("unknown prompt: '{}'", name));
            if !sorted.is_empty() {
                write_ok(ctx.renderer, "available prompts:");
                for p in &sorted {
                    write_result(ctx.renderer, format!("  {}", p));
                }
            }
        }
    }
    Ok(())
}

async fn handle_theme(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let mut sorted: Vec<&String> = ctx.context.themes.keys().collect();
    sorted.sort();
    if parts.len() < 2 {
        if sorted.is_empty() {
            write_ok(ctx.renderer, "no themes available");
        } else {
            let current = ctx
                .context
                .current_theme_name
                .as_deref()
                .unwrap_or("(none)");
            write_ok(
                ctx.renderer,
                format!("available themes (current: {}):", current),
            );
            for name in &sorted {
                write_result(ctx.renderer, format!("  {}", name));
            }
            write_result(ctx.renderer, "usage: /theme <name>  |  /theme default");
        }
    } else if parts[1] == "default" {
        if ctx.context.current_theme_name.is_none() {
            write_ok(ctx.renderer, "no active theme to clear");
        } else {
            ctx.context.current_theme_name = None;
            let _ = storage::save_theme_name(None);
            if let Some(colors) = &ctx.cfg.colors {
                ctx.renderer
                    .set_colors(crate::ui::utils::UiColors::from_config(colors));
            }

            write_ok(ctx.renderer, "theme cleared (using config colors)");
        }
    } else {
        let name = parts[1].trim();
        if let Some(content) = ctx.context.themes.get(name) {
            ctx.context.current_theme_name = Some(name.to_string());
            let _ = storage::save_theme_name(Some(name));
            crate::context::themes::apply(content, ctx.renderer);

            write_ok(ctx.renderer, format!("active theme: {}", name));
        } else {
            write_error(ctx.renderer, format!("unknown theme: '{}'", name));
            if !sorted.is_empty() {
                write_ok(ctx.renderer, "available themes:");
                for t in &sorted {
                    write_result(ctx.renderer, format!("  {}", t));
                }
            }
        }
    }
    Ok(())
}

async fn handle_regen_prompts(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match context::prompts::regen() {
        Ok(()) => {
            ctx.context.prompts = context::prompts::load();
            write_ok(ctx.renderer, "default prompts regenerated");
        }
        Err(e) => {
            write_error(ctx.renderer, format!("failed to regenerate prompts: {}", e));
        }
    }
    Ok(())
}

async fn handle_regen_themes(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match context::themes::regen() {
        Ok(()) => {
            ctx.context.themes = context::themes::load();
            write_ok(ctx.renderer, "default themes regenerated");
        }
        Err(e) => {
            write_error(ctx.renderer, format!("failed to regenerate themes: {}", e));
        }
    }
    Ok(())
}
