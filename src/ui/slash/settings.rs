use crate::agent::tools;
use crate::config::types::EditSystem;
use crate::permission::SecurityMode;
use crate::ui::slash::{SlashCtx, write_error, write_ok, write_result};

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match parts[0] {
        "/reasoning" | "/thinking" => handle_reasoning(parts, ctx).await,
        "/mode" => handle_mode(parts, ctx).await,
        "/toggle" => handle_toggle(parts, ctx).await,
        "/editsys" => handle_editsys(parts, ctx).await,
        "/advisor" => {
            #[cfg(feature = "advisor")]
            {
                handle_advisor(parts, ctx).await
            }
            #[cfg(not(feature = "advisor"))]
            {
                write_error(
                    ctx.renderer,
                    "Advisor support not enabled (build with --features advisor)",
                );
                Ok(())
            }
        }
        #[cfg(feature = "mcp")]
        "/mcp" => handle_mcp(parts, ctx).await,
        #[cfg(not(feature = "mcp"))]
        "/mcp" => {
            write_error(
                ctx.renderer,
                "MCP support not enabled (build with --features mcp)",
            );
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn handle_reasoning(_parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    *ctx.reasoning_enabled = !*ctx.reasoning_enabled;
    *ctx.show_reasoning = *ctx.reasoning_enabled;
    ctx.rebuild_agent().await;
    write_ok(
        ctx.renderer,
        format!(
            "reasoning: {}",
            if *ctx.reasoning_enabled { "on" } else { "off" }
        ),
    );
    Ok(())
}

#[cfg(feature = "advisor")]
async fn handle_advisor(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    use crate::extras::advisor;

    let current = advisor::with_config(|c| c.clone());

    if parts.len() < 2 {
        write_ok(ctx.renderer, "advisor:");
        write_result(
            ctx.renderer,
            format!("  enabled: {}", if current.enabled { "yes" } else { "no" }),
        );
        write_result(
            ctx.renderer,
            format!(
                "  mode: {}",
                if current.human_handoff {
                    "human handoff"
                } else {
                    "model"
                }
            ),
        );
        write_result(ctx.renderer, format!("  model: {}", current.advisor_model));
        write_result(
            ctx.renderer,
            format!(
                "  max uses: {}",
                current
                    .max_uses
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unlimited".to_string())
            ),
        );
        write_result(ctx.renderer, "");
        write_result(
            ctx.renderer,
            format!("  context limit: {} KB", current.kilobytes_limit),
        );
        write_result(ctx.renderer, "");
        write_result(ctx.renderer, "  /advisor on|off");
        write_result(ctx.renderer, "  /advisor handoff [on|off]");
        write_result(ctx.renderer, "  /advisor model <name>");
        write_result(ctx.renderer, "  /advisor max-uses <n>");
        write_result(ctx.renderer, "  /advisor context-limit <kilobytes>");
        return Ok(());
    }

    match parts[1] {
        "on" => {
            let mut cfg = current;
            cfg.enabled = true;
            advisor::init_config(cfg);
            ctx.rebuild_agent().await;
            write_ok(ctx.renderer, "advisor: on");
        }
        "off" => {
            let mut cfg = current;
            cfg.enabled = false;
            advisor::init_config(cfg);
            ctx.rebuild_agent().await;
            write_ok(ctx.renderer, "advisor: off");
        }
        "handoff" => {
            let new_state = match parts.get(2).copied() {
                Some("on") | None => true,
                Some("off") => false,
                Some(other) => {
                    write_error(ctx.renderer, format!("invalid: '{}', use on or off", other));
                    return Ok(());
                }
            };
            let mut cfg = current;
            cfg.human_handoff = new_state;
            // In human handoff mode, need a handoff_tx; use existing client's
            // tx or create a new channel
            if new_state && cfg.handoff_tx.is_none() {
                // Can't create a new handoff channel at runtime without TUI
                write_error(
                    ctx.renderer,
                    "Human handoff requires a TUI channel (start with --advisor-human-handoff)",
                );
                return Ok(());
            }
            advisor::init_config(cfg);
            ctx.rebuild_agent().await;
            write_ok(
                ctx.renderer,
                format!("advisor handoff: {}", if new_state { "on" } else { "off" }),
            );
        }
        "model" => {
            if let Some(model) = parts.get(2) {
                let mut cfg = current;
                cfg.advisor_model = model.to_string();
                advisor::init_config(cfg);
                ctx.rebuild_agent().await;
                write_ok(ctx.renderer, format!("advisor model: {}", model));
            } else {
                write_error(ctx.renderer, "usage: /advisor model <name>");
            }
        }
        "max-uses" => {
            if let Some(n) = parts.get(2).and_then(|s| s.parse::<usize>().ok()) {
                let mut cfg = current;
                cfg.max_uses = if n == 0 { None } else { Some(n) };
                let label = cfg
                    .max_uses
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unlimited".to_string());
                advisor::init_config(cfg);
                write_ok(ctx.renderer, format!("advisor max uses: {}", label));
            } else {
                write_error(
                    ctx.renderer,
                    "usage: /advisor max-uses <number|0=unlimited>",
                );
            }
        }
        "context-limit" => {
            if let Some(n) = parts.get(2).and_then(|s| s.parse::<u32>().ok()) {
                let mut cfg = current;
                cfg.kilobytes_limit = n;
                advisor::init_config(cfg);
                write_ok(
                    ctx.renderer,
                    format!(
                        "advisor context limit: {} KB ({} chars head / {} chars tail)",
                        n,
                        n as usize * 1024 / 2,
                        n as usize * 1024 / 2,
                    ),
                );
            } else {
                write_error(ctx.renderer, "usage: /advisor context-limit <kilobytes>");
            }
        }
        _ => {
            write_error(
                ctx.renderer,
                format!(
                    "unknown: '{}' (on|off|handoff|model|max-uses|context-limit)",
                    parts[1]
                ),
            );
        }
    }
    Ok(())
}

async fn handle_mode(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let current_mode = ctx
        .permission
        .as_ref()
        .map(|p| p.lock().unwrap_or_else(|e| e.into_inner()).mode())
        .unwrap_or(SecurityMode::Standard);

    if parts.len() < 2 {
        write_ok(ctx.renderer, "security mode:");
        write_result(ctx.renderer, format!("  current: {}", current_mode));
        write_result(ctx.renderer, "");
        write_result(
            ctx.renderer,
            "  /mode standard      allow within CWD, ask for external",
        );
        write_result(ctx.renderer, "  /mode restrictive   ask for all operations");
        write_result(
            ctx.renderer,
            "  /mode readonly      allow reads, deny everything else",
        );
        write_result(
            ctx.renderer,
            "  /mode guarded       allow reads, ask for everything else",
        );
        write_result(
            ctx.renderer,
            "  /mode yolo          allow all, ask for destructive bash",
        );
        return Ok(());
    }
    match parts[1] {
        "standard" => set_mode(ctx, SecurityMode::Standard, "standard").await,
        "restrictive" => set_mode(ctx, SecurityMode::Restrictive, "restrictive").await,
        "readonly" => set_mode(ctx, SecurityMode::ReadOnly, "readonly").await,
        "guarded" => set_mode(ctx, SecurityMode::Guarded, "guarded").await,
        "yolo" => set_mode(ctx, SecurityMode::Yolo, "yolo").await,
        _ => write_error(ctx.renderer, format!("unknown mode: {}", parts[1])),
    }
    Ok(())
}

async fn set_mode(ctx: &mut SlashCtx<'_>, mode: SecurityMode, label: &str) {
    if let Some(p) = ctx.permission {
        p.lock().unwrap_or_else(|e| e.into_inner()).set_mode(mode);
        write_ok(ctx.renderer, format!("security mode: {}", label));
    } else {
        write_error(ctx.renderer, "permission system not active");
    }
}

async fn handle_toggle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 {
        write_ok(ctx.renderer, "usage: /toggle <feature> [on|off]");
        write_ok(ctx.renderer, "features:");
        write_result(
            ctx.renderer,
            format!(
                "  todo  {}",
                if *ctx.todo_tools_enabled { "on" } else { "off" }
            ),
        );
    } else {
        let new_state = match parts.get(2).copied() {
            Some("on") => true,
            Some("off") => false,
            Some(other) => {
                write_error(ctx.renderer, format!("invalid: '{}', use on or off", other));
                return Ok(());
            }
            None => !*ctx.todo_tools_enabled,
        };
        if new_state == *ctx.todo_tools_enabled {
            write_ok(
                ctx.renderer,
                format!(
                    "todo tools already {}",
                    if new_state { "on" } else { "off" }
                ),
            );
        } else {
            *ctx.todo_tools_enabled = new_state;
            ctx.rebuild_agent().await;
            write_ok(
                ctx.renderer,
                format!(
                    "todo tools: {}",
                    if *ctx.todo_tools_enabled { "on" } else { "off" }
                ),
            );
        }
    }
    Ok(())
}

async fn handle_editsys(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let current = tools::edit_system();
    if parts.len() < 2 {
        write_ok(ctx.renderer, format!("edit system: {}", current));
        write_result(
            ctx.renderer,
            "  /editsys similarity   SEARCH/REPLACE with fuzzy matching",
        );
        write_result(
            ctx.renderer,
            "  /editsys hashedit     tag-based (CRC-32 line hashes)",
        );
        return Ok(());
    }
    match parts[1] {
        "similarity" => {
            tools::set_edit_system(EditSystem::Similarity);
            write_ok(ctx.renderer, "edit system: similarity (SEARCH/REPLACE)");
        }
        "hashedit" => {
            tools::set_edit_system(EditSystem::Hashedit);
            write_ok(ctx.renderer, "edit system: hashedit (tag-based)");
        }
        _ => write_error(
            ctx.renderer,
            format!("unknown: '{}' (similarity|hashedit)", parts[1]),
        ),
    }
    Ok(())
}

/// Prefix understood by the caller in `ui::mod` to start the interactive OAuth
/// login for a server. The login (browser wait) runs there as a background task
/// so the TUI stays responsive; on success the server is reconnected.
#[cfg(feature = "mcp")]
pub(crate) const DEFER_MCP_LOGIN: &str = "DEFER_MCP_LOGIN:";

#[cfg(feature = "mcp")]
async fn handle_mcp(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() >= 2 && parts[1] == "login" {
        return handle_mcp_login(parts.get(2).map(|s| s.trim()), ctx).await;
    }
    if parts.len() >= 2 && parts[1] == "logout" {
        return handle_mcp_logout(parts.get(2).map(|s| s.trim()), ctx);
    }

    let Some(mgr) = ctx.mcp_manager else {
        write_ok(ctx.renderer, "no MCP servers configured");
        return Ok(());
    };

    // `/mcp <server>` — list tools for one server.
    if parts.len() > 1 {
        let name = parts[1].trim();
        if let Some(handle) = mgr.get_handle(name).await {
            match handle.read().await.list_tools().await {
                Ok(tools) => {
                    if tools.is_empty() {
                        write_ok(ctx.renderer, format!("server '{}' has no tools", name));
                    } else {
                        write_ok(ctx.renderer, format!("tools on '{}':", name));
                        for tool in &tools {
                            let desc = tool.description.as_deref().unwrap_or("");
                            write_result(ctx.renderer, format!("  {}  {}", tool.name, desc));
                        }
                    }
                }
                Err(e) => {
                    write_error(
                        ctx.renderer,
                        format!("error listing tools on '{}': {}", name, e),
                    );
                }
            }
        } else if is_oauth_server(ctx, name) {
            write_error(
                ctx.renderer,
                format!("server '{name}' is not connected (run /mcp login {name})"),
            );
        } else if server_is_configured(ctx, name) {
            write_error(ctx.renderer, format!("server '{name}' is not connected"));
        } else {
            write_error(ctx.renderer, format!("unknown MCP server: '{name}'"));
        }
        return Ok(());
    }

    // `/mcp` — list all configured servers, green = connected, red = not.
    let Some(servers) = ctx.cfg.mcp_servers.as_ref().filter(|s| !s.is_empty()) else {
        write_ok(ctx.renderer, "no MCP servers configured");
        return Ok(());
    };
    write_ok(ctx.renderer, "MCP servers:");
    let mut names: Vec<&String> = servers.keys().collect();
    names.sort();
    for name in names {
        if let Some(handle) = mgr.get_handle(name).await {
            let label = match handle.read().await.list_tools().await {
                Ok(tools) => format!("  + {name} ({} tools)", tools.len()),
                Err(_) => format!("  + {name} (connected)"),
            };
            ctx.renderer
                .write_line(&label, crossterm::style::Color::Green)?;
        } else {
            let oauth = matches!(
                servers.get(name),
                Some(crate::extras::mcp::config::McpServerConfig::Url { oauth: Some(o), .. })
                    if o.settings().is_some()
            );
            let label = if oauth {
                format!("  - {name} (unauthenticated, run /mcp login {name})")
            } else {
                format!("  - {name} (not connected)")
            };
            ctx.renderer
                .write_line(&label, crossterm::style::Color::Red)?;
        }
    }
    Ok(())
}

/// True if a server name exists in config.
#[cfg(feature = "mcp")]
fn server_is_configured(ctx: &SlashCtx<'_>, name: &str) -> bool {
    ctx.cfg
        .mcp_servers
        .as_ref()
        .is_some_and(|m| m.contains_key(name))
}

/// True if a configured server is a URL server with OAuth enabled.
#[cfg(feature = "mcp")]
fn is_oauth_server(ctx: &SlashCtx<'_>, name: &str) -> bool {
    use crate::extras::mcp::config::McpServerConfig;
    matches!(
        ctx.cfg.mcp_servers.as_ref().and_then(|m| m.get(name)),
        Some(McpServerConfig::Url { oauth: Some(o), .. }) if o.settings().is_some()
    )
}

/// Resolve a URL-based server's OAuth settings + url from config, or report why not.
#[cfg(feature = "mcp")]
fn resolve_oauth_server(
    ctx: &SlashCtx<'_>,
    name: &str,
) -> Result<(String, crate::extras::mcp::config::OAuthSettings), String> {
    use crate::extras::mcp::config::McpServerConfig;
    let servers = ctx
        .cfg
        .mcp_servers
        .as_ref()
        .ok_or_else(|| "no MCP servers configured".to_string())?;
    let server = servers
        .get(name)
        .ok_or_else(|| format!("unknown MCP server: '{name}'"))?;
    match server {
        McpServerConfig::Url { url, oauth, .. } => {
            let settings = oauth
                .as_ref()
                .and_then(|o| o.settings())
                .ok_or_else(|| format!("server '{name}' does not have OAuth enabled"))?;
            Ok((url.clone(), settings))
        }
        McpServerConfig::Command { .. } => Err(format!(
            "server '{name}' is command-based; OAuth applies to URL servers"
        )),
    }
}

#[cfg(feature = "mcp")]
async fn handle_mcp_login(name: Option<&str>, ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        write_error(ctx.renderer, "usage: /mcp login <server>");
        return Ok(());
    };
    // Validate config here so errors get friendly messages. The actual login
    // (network discovery + browser wait) runs in the event loop, which spawns
    // the wait as a background task so the TUI never freezes.
    if let Err(e) = resolve_oauth_server(ctx, name) {
        write_error(ctx.renderer, e);
        return Ok(());
    }
    Err(anyhow::anyhow!("{}{}", DEFER_MCP_LOGIN, name))
}

#[cfg(feature = "mcp")]
fn handle_mcp_logout(name: Option<&str>, ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        write_error(ctx.renderer, "usage: /mcp logout <server>");
        return Ok(());
    };
    match crate::extras::mcp::oauth::logout(name) {
        Ok(true) => write_ok(
            ctx.renderer,
            format!("removed stored OAuth token for '{name}' (effective next start)"),
        ),
        Ok(false) => write_ok(ctx.renderer, format!("no stored OAuth token for '{name}'")),
        Err(e) => write_error(ctx.renderer, format!("logout failed: {e}")),
    }
    Ok(())
}
