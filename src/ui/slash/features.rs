use crate::ui::apply_current_prompt_mode;
use crate::ui::events::render_session;
use crate::ui::slash::{SlashCtx, write_error, write_ok, write_result};

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    match parts[0] {
        "/compress" | "/compact" => handle_compress(parts, ctx),
        "/loop" => handle_loop(parts, ctx).await,
        "/worktree" => handle_worktree(parts, ctx).await,
        "/wt-merge" => handle_wt_merge(parts, ctx).await,
        "/wt-exit" => handle_wt_exit(parts, ctx).await,
        "/advisor" => handle_advisor(parts, ctx).await,
        _ => Ok(()),
    }
}

fn handle_compress(_parts: &[&str], _ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let instructions = if _parts.len() > 1 {
        Some(_parts[1..].join(" "))
    } else {
        None
    };
    let instr_str = instructions.unwrap_or_default();
    Err(anyhow::anyhow!("DEFER_COMPRESS:{}", instr_str))
}

#[cfg(feature = "loop")]
async fn handle_loop(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 || (parts.len() >= 2 && parts[1] == "status") {
        if let Some(ls) = ctx.loop_state {
            let status = if ls.active { "active" } else { "stopped" };
            write_ok(
                ctx.renderer,
                format!(
                    "loop {}: {} ({})",
                    status,
                    ls.iteration_label(),
                    ls.plan_file.display()
                ),
            );
        } else {
            write_ok(ctx.renderer, "no active loop");
            write_result(ctx.renderer, "usage: /loop <prompt>  |  /loop stop");
        }
    } else if parts[1] == "stop" {
        if let Some(ls) = ctx.loop_state {
            ls.active = false;
            write_ok(ctx.renderer, "loop stopped");
        } else {
            write_ok(ctx.renderer, "no active loop");
        }
    } else {
        let prompt = parts[1..].join(" ");
        if prompt.is_empty() {
            write_error(ctx.renderer, "usage: /loop <prompt>");
            return Ok(());
        }
        let plan_file = std::path::PathBuf::from("LOOP_PLAN.md");
        let ls = crate::extras::r#loop::LoopState::new(prompt, plan_file, None, None);
        *ctx.loop_state = Some(ls);
        write_ok(
            ctx.renderer,
            "loop started — iteration 1 will run after this message",
        );
    }
    Ok(())
}

#[cfg(not(feature = "loop"))]
async fn handle_loop(_parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    write_error(
        ctx.renderer,
        "/loop requires the 'loop' feature: cargo build --features loop",
    );
    Ok(())
}

#[cfg(feature = "advisor")]
async fn handle_advisor(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    crate::extras::advisor::slash::handle(parts, ctx).await
}

#[cfg(not(feature = "advisor"))]
async fn handle_advisor(_parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    write_error(
        ctx.renderer,
        "/advisor requires the 'advisor' feature: cargo build --features advisor",
    );
    Ok(())
}

#[cfg(feature = "git-worktree")]
async fn handle_worktree(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    if parts.len() < 2 {
        write_error(ctx.renderer, "usage: /worktree <name>");
        return Ok(());
    }
    let name = parts[1].trim();
    if name.is_empty() || name.contains(' ') || name.contains('/') {
        write_error(
            ctx.renderer,
            "invalid name: use a single word without spaces or slashes",
        );
        return Ok(());
    }

    let wt_base_dir = ctx.cli.resolve_wt_base_dir(ctx.cfg);
    match crate::extras::git_worktree::create(name, wt_base_dir.as_deref()) {
        Ok((path, _info)) => {
            std::env::set_current_dir(&path)
                .map_err(|e| anyhow::anyhow!("failed to change directory: {}", e))?;
            ctx.session.working_dir = compact_str::CompactString::new(path.to_string_lossy());
            ctx.context.reload();
            apply_current_prompt_mode(ctx.context, ctx.permission);
            ctx.rebuild_agent().await;
            render_session(ctx.renderer, ctx.session, ctx.cli, ctx.cfg, ctx.context)?;
            write_ok(
                ctx.renderer,
                format!("worktree created: branch '{}' at {}", name, path.display()),
            );
        }
        Err(e) => {
            write_error(ctx.renderer, format!("failed: {}", e));
        }
    }
    Ok(())
}

#[cfg(not(feature = "git-worktree"))]
async fn handle_worktree(_parts: &[&str], _ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(feature = "git-worktree")]
async fn handle_wt_merge(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let info = match crate::extras::git_worktree::detect() {
        Some(i) => i,
        None => {
            write_error(ctx.renderer, "not in a git worktree");
            return Ok(());
        }
    };
    let target = if parts.len() >= 2 {
        parts[1].trim().to_string()
    } else {
        match crate::extras::git_worktree::default_branch(&info.main_repo_path) {
            Some(b) => b,
            None => {
                write_error(
                    ctx.renderer,
                    "no target branch specified and couldn't detect main/master",
                );
                return Ok(());
            }
        }
    };
    let repo_name = crate::extras::git_worktree::repo_name(&info.main_repo_path);
    let main_path = info.main_repo_path.display();
    let wt_path = info.worktree_path.display();
    write_ok(
        ctx.renderer,
        format!(
            "merging '{}' into '{}' in {}...",
            info.branch, target, repo_name
        ),
    );
    Err(anyhow::anyhow!(
        "DEFER_WT_MERGE\x1F{}\x1F{}\x1F{}\x1F{}\x1F{}",
        info.branch,
        target,
        main_path,
        wt_path,
        repo_name
    ))
}

#[cfg(not(feature = "git-worktree"))]
async fn handle_wt_merge(_parts: &[&str], _ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(feature = "git-worktree")]
async fn handle_wt_exit(_parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let info = match crate::extras::git_worktree::detect() {
        Some(i) => i,
        None => {
            write_error(ctx.renderer, "not in a git worktree");
            return Ok(());
        }
    };
    let main_path = info.main_repo_path.display();
    write_ok(
        ctx.renderer,
        format!("returning to main repo at {}", main_path),
    );
    Err(anyhow::anyhow!(
        "DEFER_WT_EXIT\x1F{}\x1F{}",
        main_path,
        info.worktree_path.display()
    ))
}

#[cfg(not(feature = "git-worktree"))]
async fn handle_wt_exit(_parts: &[&str], _ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    Ok(())
}
