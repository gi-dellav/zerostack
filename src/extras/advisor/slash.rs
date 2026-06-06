use crate::ui::slash::{SlashCtx, write_ok, write_result};

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 || parts[1] == "status" {
        let cfg = &ctx.cfg;
        if let Some(adv_cfg) = cfg.advisor.as_ref()
            && adv_cfg.enabled
        {
            let model = adv_cfg.model.as_deref().unwrap_or("(not set)");
            let provider = adv_cfg.provider.as_deref().unwrap_or("(main provider)");
            write_ok(
                ctx.renderer,
                format!(
                    "advisor: enabled  model={}  provider={}  max_turns={}",
                    model, provider, adv_cfg.max_turns
                ),
            );
        } else {
            write_ok(ctx.renderer, "advisor: disabled");
            write_result(
                ctx.renderer,
                "Configure in config.toml: [advisor] enabled=true model=\"deepseek-v4-pro\"",
            );
        }
    } else {
        write_result(ctx.renderer, "advisor configuration is set in config.toml:");
        write_result(ctx.renderer, "  [advisor]");
        write_result(ctx.renderer, "  enabled = true");
        write_result(ctx.renderer, "  model = \"deepseek-v4-pro\"");
        write_result(
            ctx.renderer,
            "  provider = \"openrouter\"  # optional, defaults to main provider",
        );
        write_result(ctx.renderer, "  max_turns = 5  # advisor turn budget");
    }
    Ok(())
}
