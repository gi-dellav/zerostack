use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use crate::cli::Cli;
use crate::config::{self, Config};
use crate::provider::{AnyClient, ModelEntry, list_models_manual};
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

static MODEL_CACHE: LazyLock<Mutex<HashMap<String, Arc<[ModelEntry]>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns the provider's models.
///
/// Network is only touched on `refresh`, for custom gateways, or for built-in
/// providers that aren't baked (e.g. ollama). Baked built-ins are served from
/// the embedded catalog with no network call — this is what keeps startup instant.
pub(crate) async fn fetch_models_cached(
    provider: &str,
    is_custom: bool,
    client: &AnyClient,
    cli: &Cli,
    cfg: &Config,
    refresh: bool,
) -> anyhow::Result<Arc<[ModelEntry]>> {
    if !refresh {
        if let Some(hit) = MODEL_CACHE.lock().unwrap().get(provider) {
            return Ok(Arc::clone(hit)); // guard dropped here, NOT across any await
        }
        // No cache yet: serve the baked catalog for built-in providers — no network.
        if !is_custom && let Some(entries) = crate::models_catalog::catalog_entries(provider) {
            let models: Vec<ModelEntry> = entries
                .iter()
                .filter(|m| crate::provider::is_agent_model(m))
                .cloned()
                .collect();
            let arc: Arc<[ModelEntry]> = Arc::from(models.into_boxed_slice());
            MODEL_CACHE
                .lock()
                .unwrap()
                .insert(provider.to_string(), Arc::clone(&arc));
            return Ok(arc);
        }
    }
    let mut models = if is_custom {
        list_models_manual(
            provider,
            cli.api_key.as_deref(),
            &cfg.custom_providers_map(),
            cfg.api_keys.as_ref(),
        )
        .await?
    } else {
        client.list_models().await?
    };
    models.retain(crate::provider::is_agent_model);

    if provider == "openrouter" {
        match crate::provider::fetch_openrouter_pricing(
            cli.api_key.as_deref(),
            &cfg.custom_providers_map(),
            cfg.api_keys.as_ref(),
        )
        .await
        {
            Ok(prices) => {
                for m in &mut models {
                    if let Some(info) = prices.get(&m.id) {
                        m.input_price = Some(info.input_cost);
                        m.output_price = Some(info.output_cost);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to fetch OpenRouter pricing: {e}");
            }
        }
    }

    let arc: Arc<[ModelEntry]> = Arc::from(models.into_boxed_slice());
    MODEL_CACHE
        .lock()
        .unwrap()
        .insert(provider.to_string(), Arc::clone(&arc));
    Ok(arc)
}

/// sync read for the picker (no await)
pub(crate) fn cached_model_ids(provider: &str) -> Vec<String> {
    MODEL_CACHE
        .lock()
        .unwrap()
        .get(provider)
        .map(|v| v.iter().map(|m| m.id.clone()).collect())
        .unwrap_or_default()
}

/// best-effort warm; returns id list (empty on failure, never errors)
pub(crate) async fn warm_model_cache(
    provider: &str,
    is_custom: bool,
    client: &AnyClient,
    cli: &Cli,
    cfg: &Config,
) -> Vec<String> {
    let _ = fetch_models_cached(provider, is_custom, client, cli, cfg, false).await;
    cached_model_ids(provider)
}

fn lookup_pricing_from_cache(provider: &str, model_id: &str) -> Option<(f64, f64)> {
    MODEL_CACHE
        .lock()
        .unwrap()
        .get(provider)
        .and_then(|models| {
            models.iter().find_map(|m| {
                if m.id == model_id {
                    m.input_price.zip(m.output_price).or_else(|| {
                        crate::models_catalog::catalog_entries(provider).and_then(|entries| {
                            entries.iter().find_map(|e| {
                                if e.id == model_id {
                                    e.input_price.zip(e.output_price)
                                } else {
                                    None
                                }
                            })
                        })
                    })
                } else {
                    None
                }
            })
        })
}

async fn apply_model(ctx: &mut SlashCtx<'_>, model_id: &str) {
    let new_model = compact_str::CompactString::new(model_id);
    let new_agent = ctx
        .agent_build_ctx()
        .rebuild_agent(&new_model, *ctx.reasoning_enabled)
        .await;
    *ctx.agent = Some(new_agent);
    ctx.session.model = new_model.clone();
    ctx.session
        .update_context_window(ctx.cfg.resolve_context_window(
            &ctx.session.provider,
            &new_model,
            &crate::config::quick_models_map(ctx.cfg),
        ));
    if let Some((input, output)) = lookup_pricing_from_cache(&ctx.session.provider, model_id) {
        ctx.session.input_token_cost = input;
        ctx.session.output_token_cost = output;
    } else if ctx.session.provider == "openrouter"
        && let Ok(prices) = crate::provider::fetch_openrouter_pricing(
            ctx.cli.api_key.as_deref(),
            &ctx.cfg.custom_providers_map(),
            ctx.cfg.api_keys.as_ref(),
        )
        .await
        && let Some(info) = prices.get(model_id)
    {
        ctx.session.input_token_cost = info.input_cost;
        ctx.session.output_token_cost = info.output_cost;
        if ctx.cfg.context_window.is_none()
            && crate::config::Config::catalog_context_window("openrouter", model_id).is_none()
            && let Some(cw) = info.context_length
        {
            ctx.session.update_context_window(cw);
        }
    }
    write_ok(ctx.renderer, format!("switched to model: {}", new_model));
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
    ctx.session
        .update_context_window(ctx.cfg.resolve_context_window(
            new_provider,
            &ctx.session.model,
            &crate::config::quick_models_map(ctx.cfg),
        ));
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
    let new_agent = ctx
        .agent_build_ctx()
        .rebuild_agent(&new_model, *ctx.reasoning_enabled)
        .await;
    *ctx.agent = Some(new_agent);
    ctx.session.model = new_model.clone();
    ctx.session.provider = ctx.cli.resolve_provider(ctx.cfg);
    ctx.session
        .update_context_window(ctx.cfg.resolve_context_window(
            &ctx.session.provider,
            &new_model,
            &crate::config::quick_models_map(ctx.cfg),
        ));
    if let Some((input, output)) = lookup_pricing_from_cache(&ctx.session.provider, &new_model) {
        ctx.session.input_token_cost = input;
        ctx.session.output_token_cost = output;
    } else if ctx.session.provider == "openrouter"
        && let Ok(prices) = crate::provider::fetch_openrouter_pricing(
            ctx.cli.api_key.as_deref(),
            &ctx.cfg.custom_providers_map(),
            ctx.cfg.api_keys.as_ref(),
        )
        .await
        && let Some(info) = prices.get(&*new_model)
    {
        ctx.session.input_token_cost = info.input_cost;
        ctx.session.output_token_cost = info.output_cost;
        if ctx.cfg.context_window.is_none()
            && crate::config::Config::catalog_context_window("openrouter", &new_model).is_none()
            && let Some(cw) = info.context_length
        {
            ctx.session.update_context_window(cw);
        }
    }
    write_ok(ctx.renderer, format!("switched to model: {}", new_model));
    Ok(())
}

async fn handle_models(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let qm = config::quick_models_map(ctx.cfg);
    let provider = ctx.session.provider.to_string();
    let is_custom = ctx.cfg.custom_providers_map().contains_key(&provider);

    let refresh = parts.get(1).map(|s| s.trim()) == Some("refresh");

    // /models <name-or-id> — quick-model name first, else raw model id for current provider
    if parts.len() >= 2 && !refresh {
        let arg = parts[1].trim();
        if let Some(q) = qm.get(arg) {
            ctx.rebuild_agent_with_client(&q.provider, *ctx.reasoning_enabled)
                .await?;
            apply_model(ctx, &q.model).await;
            ctx.session.provider = compact_str::CompactString::new(&q.provider);
            // preserve v1.4.x pricing/cost tracking
            ctx.session.input_token_cost = q.input_token_cost;
            ctx.session.output_token_cost = q.output_token_cost;
            write_result(
                ctx.renderer,
                format!(
                    "  quick model {} — ${:.4}/M in  ${:.4}/M out",
                    arg, q.input_token_cost, q.output_token_cost
                ),
            );
            return Ok(());
        }
        apply_model(ctx, arg).await;
        return Ok(());
    }

    // ---- list mode (+ optional refresh) ----
    match fetch_models_cached(&provider, is_custom, ctx.client, ctx.cli, ctx.cfg, refresh).await {
        Ok(models) => {
            ctx.input.set_live_model_names(cached_model_ids(&provider));
            if refresh {
                // Explicit refresh: just confirm with a count overview — the picker
                // already holds the full list, so don't dump it to the scrollback.
                // Dim (DarkGrey), matching the "[system] loaded AGENTS.md" startup notices.
                write_result(
                    ctx.renderer,
                    format!(
                        "model list refreshed — quick models: {}, {} models: {}",
                        qm.len(),
                        provider,
                        models.len()
                    ),
                );
            } else {
                // Full listing: quick models, then the provider's available models.
                let mut sorted: Vec<&String> = qm.keys().collect();
                sorted.sort();
                write_ok(
                    ctx.renderer,
                    format!(
                        "quick models (current: {} | {}):",
                        ctx.session.provider, ctx.session.model
                    ),
                );
                if sorted.is_empty() {
                    write_result(ctx.renderer, "  (none — add with /models-add)");
                }
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
                if !models.is_empty() {
                    write_ok(
                        ctx.renderer,
                        format!("available from {} ({}):", provider, models.len()),
                    );
                    const CAP: usize = 50;
                    for m in models.iter().take(CAP) {
                        let ctx_win = m
                            .context_length
                            .map(|c| format!("  [{}k ctx]", c / 1000))
                            .unwrap_or_default();
                        let label = if m.display == m.id {
                            m.id.clone()
                        } else {
                            format!("{} ({})", m.display, m.id)
                        };
                        write_result(ctx.renderer, format!("  {}{}", label, ctx_win));
                    }
                    if models.len() > CAP {
                        write_result(
                            ctx.renderer,
                            format!(
                                "  … {} more — type /models <filter> or use the picker",
                                models.len() - CAP
                            ),
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::debug!("model listing failed for {}: {}", provider, e);
            if refresh {
                write_error(ctx.renderer, format!("model list refresh failed: {}", e));
            } else if is_custom {
                write_result(
                    ctx.renderer,
                    "  (live model list unavailable; type the model id directly)",
                );
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
    let max_turns = ctx.cfg.task_max_turns.unwrap_or(20);
    let _agent = crate::extras::subagents::builder::build_explore_agent(
        model,
        max_turns,
        ctx.cfg,
        #[cfg(feature = "archmd")]
        None,
    )
    .await;
    Ok(())
}
