use crate::config;
use crate::ui::slash::{SlashCtx, write_error, write_ok, write_result};

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match parts[0] {
        "/provider" => handle_provider(parts, ctx).await,
        "/model" => handle_model(parts, ctx).await,
        "/models" => handle_models(parts, ctx).await,
        "/models-add" => handle_models_add(parts, ctx).await,
        #[cfg(feature = "subagents")]
        "/model-subagent" => handle_model_subagent(parts, ctx).await,
        #[cfg(feature = "subagents")]
        "/models-subagent" => handle_models_subagent(parts, ctx).await,
        _ => Ok(()),
    }
}

async fn handle_provider(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 {
        write_ok(
            ctx.renderer,
            format!("current provider: {}", ctx.session.provider),
        );
        return Ok(());
    }
    let new_provider = parts[1].trim();
    if crate::provider::parse_provider(new_provider).is_none()
        && !ctx.cfg.custom_providers_map().contains_key(new_provider)
    {
        write_error(
            ctx.renderer,
            format!("unknown provider: '{}'", new_provider),
        );
        return Ok(());
    }
    // Default the model to something valid for the new provider BEFORE rebuilding,
    // since rebuild_agent_with_client reads session.model. Otherwise the old id
    // (e.g. an OpenRouter id) is carried onto a provider where it is invalid.
    if let Some((model, costs)) = crate::provider::default_model_for_provider(new_provider, ctx.cfg)
    {
        ctx.session.model = compact_str::CompactString::new(&model);
        if let Some((inc, outc)) = costs {
            ctx.session.input_token_cost = inc;
            ctx.session.output_token_cost = outc;
        }
    }
    ctx.rebuild_agent_with_client(new_provider, *ctx.reasoning_enabled)
        .await?;
    ctx.session.provider = compact_str::CompactString::new(new_provider);
    write_ok(
        ctx.renderer,
        format!(
            "switched to provider: {} (model: {})",
            new_provider, ctx.session.model
        ),
    );
    Ok(())
}

async fn handle_model(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 {
        write_ok(
            ctx.renderer,
            format!("current model: {}", ctx.session.model),
        );
        return Ok(());
    }
    let new_model = compact_str::CompactString::new(parts[1].trim());
    let model = ctx.client.completion_model(new_model.to_string());
    *ctx.agent = Some(
        crate::provider::build_agent(
            model,
            ctx.cli,
            ctx.cfg,
            ctx.context,
            ctx.permission.clone(),
            ctx.ask_tx.clone(),
            ctx.sandbox.clone(),
            *ctx.reasoning_enabled,
            #[cfg(feature = "mcp")]
            ctx.mcp_manager,
        )
        .await,
    );
    ctx.session.model = new_model.clone();
    ctx.session.provider = ctx.cli.resolve_provider(ctx.cfg);
    write_ok(ctx.renderer, format!("switched to model: {}", new_model));
    Ok(())
}

async fn handle_models(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let qm = config::quick_models_map(ctx.cfg);
    let mut sorted: Vec<&String> = qm.keys().collect();
    sorted.sort();
    if parts.len() < 2 {
        if sorted.is_empty() {
            write_ok(ctx.renderer, "no quick models defined");
        } else {
            write_ok(
                ctx.renderer,
                format!(
                    "quick models (current: {} | {}):",
                    ctx.session.provider, ctx.session.model
                ),
            );
            for name in &sorted {
                let q = &qm[name.as_str()];
                write_result(
                    ctx.renderer,
                    format!(
                        "  {}  ({} / {})  ${:.4}/M in  ${:.4}/M out",
                        name, q.provider, q.model, q.input_token_cost, q.output_token_cost
                    ),
                );
            }
        }
    } else {
        let name = parts[1].trim();
        if let Some(q) = qm.get(name) {
            ctx.rebuild_agent_with_client(&q.provider, *ctx.reasoning_enabled)
                .await?;
            let model = ctx.client.completion_model(q.model.to_string());
            *ctx.agent = Some(
                crate::provider::build_agent(
                    model,
                    ctx.cli,
                    ctx.cfg,
                    ctx.context,
                    ctx.permission.clone(),
                    ctx.ask_tx.clone(),
                    ctx.sandbox.clone(),
                    *ctx.reasoning_enabled,
                    #[cfg(feature = "mcp")]
                    ctx.mcp_manager,
                )
                .await,
            );
            ctx.session.provider = compact_str::CompactString::new(&q.provider);
            ctx.session.model = compact_str::CompactString::new(&q.model);
            ctx.session.input_token_cost = q.input_token_cost;
            ctx.session.output_token_cost = q.output_token_cost;
            write_ok(
                ctx.renderer,
                format!(
                    "switched to quick model: {} ({} / {})  ${:.4}/M in  ${:.4}/M out",
                    name, q.provider, q.model, q.input_token_cost, q.output_token_cost
                ),
            );
        } else {
            write_error(ctx.renderer, format!("unknown quick model: '{}'", name));
            if !sorted.is_empty() {
                write_ok(ctx.renderer, "available quick models:");
                for n in &sorted {
                    write_result(ctx.renderer, format!("  {}", n));
                }
            }
        }
    }
    Ok(())
}

async fn handle_models_add(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 3 {
        write_ok(
            ctx.renderer,
            "usage: /models-add <name> <provider> <model> [input_cost_per_M output_cost_per_M]",
        );
        return Ok(());
    }
    let name = parts[1].trim().to_string();
    let rest = parts[2].trim();
    let (provider, model, input_cost, output_cost) = match rest.split_once(' ') {
        Some((p, m)) if parts.len() >= 5 => (
            p.trim().to_string(),
            m.trim().to_string(),
            parts[3].trim().parse::<f64>().unwrap_or(0.0),
            parts[4].trim().parse::<f64>().unwrap_or(0.0),
        ),
        Some((p, m)) => (p.trim().to_string(), m.trim().to_string(), 0.0, 0.0),
        None => {
            write_ok(
                ctx.renderer,
                "usage: /models-add <name> <provider> <model> [input_cost_per_M output_cost_per_M]",
            );
            return Ok(());
        }
    };
    if name.is_empty() || provider.is_empty() || model.is_empty() {
        write_ok(
            ctx.renderer,
            "usage: /models-add <name> <provider> <model> [input_cost_per_M output_cost_per_M]",
        );
        return Ok(());
    }
    match config::save_quick_model(&name, &provider, &model, input_cost, output_cost) {
        Ok(()) => {
            write_ok(
                ctx.renderer,
                format!(
                    "saved quick model: {} ({} / {})  ${}/M in  ${}/M out",
                    name, provider, model, input_cost, output_cost
                ),
            );
        }
        Err(e) => {
            write_error(ctx.renderer, format!("failed to save quick model: {}", e));
        }
    }
    Ok(())
}

#[cfg(feature = "subagents")]
async fn handle_model_subagent(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    use crate::extras::subagents;

    if parts.len() < 2 {
        let (provider_name, model_name) =
            subagents::with_config(|cfg| (cfg.client.provider_name(), cfg.model_name.clone()));
        write_ok(
            ctx.renderer,
            format!("current subagent model: {} / {}", provider_name, model_name),
        );
        return Ok(());
    }

    let new_model = parts[1].trim().to_string();
    let model = ctx.client.completion_model(new_model.clone());
    model_for_subagent(ctx, model).await?;
    subagents::set_model_name(new_model.clone());
    write_ok(
        ctx.renderer,
        format!("switched subagent to model: {}", new_model),
    );
    Ok(())
}

#[cfg(feature = "subagents")]
async fn handle_models_subagent(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    use crate::extras::subagents;

    let qm = config::quick_models_map(ctx.cfg);
    let mut sorted: Vec<&String> = qm.keys().collect();
    sorted.sort();

    if parts.len() < 2 {
        let (provider_name, model_name) =
            subagents::with_config(|cfg| (cfg.client.provider_name(), cfg.model_name.clone()));
        if sorted.is_empty() {
            write_ok(
                ctx.renderer,
                format!(
                    "current subagent: {} / {} (no quick models defined)",
                    provider_name, model_name
                ),
            );
        } else {
            write_ok(
                ctx.renderer,
                format!(
                    "quick models (current subagent: {} | {}):",
                    provider_name, model_name
                ),
            );
            for name in &sorted {
                let q = &qm[name.as_str()];
                write_result(
                    ctx.renderer,
                    format!(
                        "  {}  ({} / {})  ${:.4}/M in  ${:.4}/M out",
                        name, q.provider, q.model, q.input_token_cost, q.output_token_cost
                    ),
                );
            }
        }
        return Ok(());
    }

    let name = parts[1].trim();
    if let Some(q) = qm.get(name) {
        if q.provider.as_str() != ctx.client.provider_name() {
            let new_client = crate::provider::create_client(
                &q.provider,
                ctx.cli.api_key.as_deref(),
                &ctx.cfg.custom_providers_map(),
                ctx.cfg.api_keys.as_ref(),
            )?;
            let model = new_client.completion_model(q.model.to_string());
            model_for_subagent(ctx, model).await?;
            subagents::set_client_and_model(new_client, q.model.to_string());
        } else {
            let model = ctx.client.completion_model(q.model.to_string());
            model_for_subagent(ctx, model).await?;
            subagents::set_model_name(q.model.to_string());
        }
        write_ok(
            ctx.renderer,
            format!(
                "switched subagent to quick model: {} ({} / {})  ${:.4}/M in  ${:.4}/M out",
                name, q.provider, q.model, q.input_token_cost, q.output_token_cost
            ),
        );
    } else {
        write_error(ctx.renderer, format!("unknown quick model: '{}'", name));
        if !sorted.is_empty() {
            write_ok(ctx.renderer, "available quick models:");
            for n in &sorted {
                write_result(ctx.renderer, format!("  {}", n));
            }
        }
    }
    Ok(())
}

/// Validate a model handle by trying to build a subagent with it.
/// If it fails, the error is shown but does not abort the command.
#[cfg(feature = "subagents")]
async fn model_for_subagent(
    ctx: &mut SlashCtx<'_>,
    model: crate::provider::AnyModel,
) -> anyhow::Result<()> {
    let max_turns = ctx.cfg.task_max_turns.unwrap_or(15);
    let _agent = crate::extras::subagents::builder::build_explore_agent(
        model,
        max_turns,
        #[cfg(feature = "archmd")]
        None,
    )
    .await;
    Ok(())
}
