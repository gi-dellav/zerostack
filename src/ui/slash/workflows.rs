#[cfg(feature = "workflows")]
use crate::extras::workflows;
use crate::ui::slash::SlashCtx;
use crate::ui::slash::write_error;
#[cfg(feature = "workflows")]
use crate::ui::slash::write_ok;
#[cfg(feature = "workflows")]
use crate::ui::slash::write_result;

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    #[cfg(not(feature = "workflows"))]
    {
        let _ = parts;
        write_error(
            ctx.renderer,
            "/workflow is not available. Rebuild with:\n  cargo install --path . --debug --features workflows",
        );
        return Ok(());
    }
    #[cfg(feature = "workflows")]
    {
        let cmd = parts[0];
        if cmd == "/workflow" {
            if parts.get(1).copied() == Some("list") || parts.len() < 2 {
                let wfs = workflows::load_all();
                if wfs.is_empty() {
                    write_ok(ctx.renderer, "no workflows found");
                    write_result(
                        ctx.renderer,
                        "place .workflow files in ~/.config/zerostack/workflows/ or .zerostack/workflows/",
                    );
                } else {
                    write_ok(ctx.renderer, "workflows:");
                    for wf in &wfs {
                        write_result(
                            ctx.renderer,
                            format!("  /{} ({})", wf.name, wf.path.display()),
                        );
                    }
                    write_result(ctx.renderer, "");
                    write_result(ctx.renderer, "  invoke with /<name>");
                }
                return Ok(());
            }
            write_error(ctx.renderer, "usage: /workflow [list]");
            return Ok(());
        }

        let name = cmd.strip_prefix('/').unwrap_or("");
        if let Some(wf) = workflows::load(name) {
            if wf.slash_commands.is_empty() && wf.message.is_empty() {
                write_error(ctx.renderer, format!("workflow '{}' is empty", name));
                return Ok(());
            }

            let slash_str = wf.slash_commands.join("\n");
            Err(anyhow::anyhow!(
                "DEFER_WORKFLOW:\u{1F}{}\u{1F}{}",
                slash_str,
                wf.message
            ))
        } else {
            Ok(())
        }
    }
}
