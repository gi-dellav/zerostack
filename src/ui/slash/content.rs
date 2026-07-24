use crate::context;
use crate::session::storage;
use crate::ui::slash::{SlashCtx, write_error, write_ok, write_result};
use crate::ui::{PromptModeOutcome, apply_prompt_mode};

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
            // Revert to the config default model
            let default_model = ctx.cli.resolve_model(ctx.cfg);
            let default_provider = ctx.cli.resolve_provider(ctx.cfg);
            let provider_changed = default_provider != ctx.session.provider;
            ctx.session.model = default_model;
            if provider_changed {
                if let Err(e) = ctx
                    .rebuild_agent_with_client(&default_provider, *ctx.reasoning_enabled)
                    .await
                {
                    write_error(ctx.renderer, format!("failed to revert provider: {}", e));
                } else {
                    ctx.session.provider = default_provider;
                }
            } else {
                ctx.rebuild_agent().await;
            }
            write_ok(ctx.renderer, "prompt cleared (back to default)");
        }
    } else {
        let name = parts[1].trim();
        if ctx.context.prompts.contains_key(name) {
            match apply_prompt_mode(name, ctx.context, ctx.permission) {
                PromptModeOutcome::RestoredUserMode => {
                    if let Some(perm) = ctx.permission {
                        let current = perm
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .mode()
                            .to_string();
                        write_ok(ctx.renderer, format!("restored user mode: {}", current));
                    }
                }
                PromptModeOutcome::Applied(mode) => {
                    write_ok(
                        ctx.renderer,
                        format!("security mode: {} (from prompt)", mode),
                    );
                }
                PromptModeOutcome::None => {}
            }
            let model_switched = ctx.switch_to_prompt_model(name).await;
            if !model_switched {
                ctx.rebuild_agent().await;
            }
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
            if let Err(e) = storage::save_theme_name(None) {
                write_error(
                    ctx.renderer,
                    format!("warning: theme choice not saved: {}", e),
                );
            }
            if let Some(colors) = &ctx.cfg.colors {
                let chat_bg = colors
                    .chat_background
                    .as_deref()
                    .and_then(crate::ui::utils::parse_color);
                let input_bg = colors
                    .input_background
                    .as_deref()
                    .and_then(crate::ui::utils::parse_color);
                let status_bg = colors
                    .status_background
                    .as_deref()
                    .and_then(crate::ui::utils::parse_color);
                ctx.renderer
                    .set_background_colors(chat_bg, input_bg, status_bg);
            }
            write_ok(ctx.renderer, "theme cleared (using config colors)");
        }
    } else {
        let name = parts[1].trim();
        if let Some(content) = ctx.context.themes.get(name) {
            ctx.context.current_theme_name = Some(name.to_string());
            if let Err(e) = storage::save_theme_name(Some(name)) {
                write_error(
                    ctx.renderer,
                    format!("warning: theme choice not saved: {}", e),
                );
            }
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
