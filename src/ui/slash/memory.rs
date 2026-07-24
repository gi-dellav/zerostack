#[cfg(feature = "memory")]
use crate::extras::memory::{Mem, WriteMode, WriteTarget};
use crate::ui::slash::SlashCtx;
use crate::ui::slash::write_error;
#[cfg(feature = "memory")]
use crate::ui::slash::write_ok;
#[cfg(feature = "memory")]
use crate::ui::slash::write_result;

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    #[cfg(not(feature = "memory"))]
    {
        let _ = parts;
        write_error(
            ctx.renderer,
            "/memory is not available. Rebuild with:\n  cargo install --path . --debug --features memory",
        );
    }
    #[cfg(feature = "memory")]
    {
        match parts.get(1).copied() {
            None | Some("status") => handle_status(ctx),
            Some("search") => handle_search(parts, ctx),
            Some("read") => handle_read(parts, ctx),
            Some("write") => handle_write(parts, ctx),
            Some("editor") => return handle_editor(ctx),
            Some("clear") => handle_clear(parts, ctx),
            _ => {
                write_error(
                    ctx.renderer,
                    "usage: /memory [status|search|read|write|editor|clear]",
                );
            }
        }
    }
    Ok(())
}

#[cfg(feature = "memory")]
fn handle_status(ctx: &mut SlashCtx<'_>) {
    let mem = Mem::open();
    write_ok(ctx.renderer, "memory status:");

    let long_term = mem.memory_md();
    if long_term.exists() {
        match std::fs::metadata(&long_term) {
            Ok(m) => write_result(ctx.renderer, format!("  MEMORY.md: {}B", m.len())),
            Err(_) => write_result(ctx.renderer, "  MEMORY.md: exists (size unknown)"),
        }
    } else {
        write_result(ctx.renderer, "  MEMORY.md: (not created)");
    }

    let scratchpad = mem.scratchpad();
    if scratchpad.exists() {
        match std::fs::read_to_string(&scratchpad) {
            Ok(s) => {
                let open: Vec<&str> = s
                    .lines()
                    .filter(|l| {
                        let t = l.trim_start();
                        t.starts_with("- [ ]") || t.starts_with("* [ ]")
                    })
                    .collect();
                write_result(
                    ctx.renderer,
                    format!("  scratchpad: {} open item(s)", open.len()),
                );
            }
            Err(_) => write_result(ctx.renderer, "  scratchpad: exists (unreadable)"),
        }
    } else {
        write_result(ctx.renderer, "  scratchpad: (empty)");
    }

    let today = mem.daily_file(&mem.today);
    if today.exists() {
        match std::fs::read_to_string(&today) {
            Ok(s) => {
                let entries = s.lines().filter(|l| l.starts_with("### ")).count();
                write_result(ctx.renderer, format!("  today: {entries} entry(s)"));
            }
            Err(_) => write_result(ctx.renderer, "  today: exists (unreadable)"),
        }
    } else {
        write_result(ctx.renderer, "  today: (no entries)");
    }
}

#[cfg(feature = "memory")]
fn handle_search(parts: &[&str], ctx: &mut SlashCtx<'_>) {
    if parts.len() < 3 {
        write_error(ctx.renderer, "usage: /memory search <query>");
        return;
    }
    let query = parts[2..].join(" ");
    let mem = Mem::open();
    let results = mem.search(&query);
    let rendered = results.render(4000);
    write_ok(ctx.renderer, "search results:");
    for line in rendered.lines() {
        write_result(ctx.renderer, line);
    }
}

#[cfg(feature = "memory")]
fn handle_read(parts: &[&str], ctx: &mut SlashCtx<'_>) {
    if parts.len() < 3 {
        write_error(ctx.renderer, "usage: /memory read <source> [name]");
        write_result(
            ctx.renderer,
            "  sources: long_term, scratchpad, daily, note",
        );
        return;
    }
    let mem = Mem::open();
    let source = parts[2].to_lowercase();
    let path = match source.as_str() {
        "long_term" | "long" => Some(mem.memory_md()),
        "scratchpad" => Some(mem.scratchpad()),
        "daily" => Some(mem.daily_file(&mem.today)),
        "note" => {
            let name = parts.get(3);
            name.and_then(|n| mem.note_path(n))
        }
        _ => {
            write_error(ctx.renderer, format!("unknown source: {source}"));
            write_result(
                ctx.renderer,
                "  sources: long_term, scratchpad, daily, note",
            );
            None
        }
    };
    if let Some(p) = path {
        match std::fs::read_to_string(&p) {
            Ok(s) => {
                let capped: String = if s.len() > 4000 {
                    s.chars().take(4000).collect::<String>() + "\n…[truncated]"
                } else {
                    s
                };
                write_ok(ctx.renderer, format!("{} ({source}):", p.display()));
                for line in capped.lines() {
                    write_result(ctx.renderer, line);
                }
            }
            Err(e) => write_error(ctx.renderer, format!("read error: {e}")),
        }
    }
}

#[cfg(feature = "memory")]
fn handle_write(parts: &[&str], ctx: &mut SlashCtx<'_>) {
    if parts.len() < 4 {
        write_error(ctx.renderer, "usage: /memory write <target> <content>");
        write_result(
            ctx.renderer,
            "  targets: long_term, scratchpad, daily, note:<name>",
        );
        return;
    }
    let mem = Mem::open();
    let target_str = parts[2].to_lowercase();
    let content = parts[3..].join(" ");

    let (target, name) = if let Some(note_name) = target_str.strip_prefix("note:") {
        (WriteTarget::Note, Some(note_name))
    } else {
        match target_str.as_str() {
            "long_term" | "long" => (WriteTarget::LongTerm, None),
            "scratchpad" => (WriteTarget::Scratchpad, None),
            "daily" => (WriteTarget::Daily, None),
            _ => {
                write_error(ctx.renderer, format!("unknown target: {target_str}"));
                write_result(
                    ctx.renderer,
                    "  targets: long_term, scratchpad, daily, note:<name>",
                );
                return;
            }
        }
    };

    match mem.write(target, &content, WriteMode::Append, name) {
        Ok(msg) => write_ok(ctx.renderer, msg),
        Err(e) => write_error(ctx.renderer, format!("write error: {e}")),
    }
}

#[cfg(feature = "memory")]
fn handle_editor(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let mem = Mem::open();
    let path = mem.memory_md();
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        write_error(ctx.renderer, format!("cannot create memory dir: {e}"));
        return Ok(());
    }
    if let Err(e) = std::fs::write(&path, "") {
        write_error(
            ctx.renderer,
            format!("cannot prepare {}: {e}", path.display()),
        );
        return Ok(());
    }
    write_ok(
        ctx.renderer,
        format!("opening {} in editor...", path.display()),
    );
    Err(anyhow::anyhow!("DEFER_EDITOR:{}", path.display()))
}

#[cfg(feature = "memory")]
fn handle_clear(parts: &[&str], ctx: &mut SlashCtx<'_>) {
    if parts.len() < 3 {
        write_error(ctx.renderer, "usage: /memory clear scratchpad|daily");
        return;
    }
    let mem = Mem::open();
    let target = parts[2].to_lowercase();
    let _path = match target.as_str() {
        "scratchpad" => Some(mem.scratchpad()),
        "daily" => Some(mem.daily_file(&mem.today)),
        _ => {
            write_error(ctx.renderer, "clear only supports: scratchpad, daily");
            None
        }
    };
    if _path.is_some() {
        match mem.write(
            if target == "scratchpad" {
                WriteTarget::Scratchpad
            } else {
                WriteTarget::Daily
            },
            "",
            WriteMode::Overwrite,
            None,
        ) {
            Ok(msg) => write_ok(ctx.renderer, msg),
            Err(e) => write_error(ctx.renderer, format!("clear error: {e}")),
        }
    }
}
