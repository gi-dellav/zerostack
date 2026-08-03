use crate::docs;
use crate::ui::slash::{SlashCtx, write_error, write_ok, write_result};

/// Doc file names and their display titles.
const DOC_FILES: &[(&str, &str)] = &[
    ("COMMANDS.md", "Slash Commands"),
    ("CONFIG.md", "Configuration"),
    ("PROVIDERS.md", "Providers"),
    ("HASHEDIT.md", "Hash-Anchored Edits"),
    ("MEMORY.md", "Memory System"),
    ("ARCHITECTURE.md", "ARCHITECTURE.md Support"),
    ("SUBAGENTS.md", "Subagents"),
];

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let docs_dir = docs::global_docs_dir();

    if parts.len() < 2 {
        write_ok(ctx.renderer, "docs:");
        for (file, title) in DOC_FILES {
            let path = docs_dir.join(file);
            let size = if path.exists() {
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            } else {
                0
            };
            write_result(
                ctx.renderer,
                format!("  /docs {}  — {}  ({} bytes)", file, title, size),
            );
        }
        write_result(ctx.renderer, "");
        write_result(ctx.renderer, "  /docs list  — show this listing");
        return Ok(());
    }

    let arg = parts[1];

    if arg == "list" {
        write_ok(ctx.renderer, "docs:");
        for (file, title) in DOC_FILES {
            let path = docs_dir.join(file);
            let size = if path.exists() {
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            } else {
                0
            };
            write_result(
                ctx.renderer,
                format!("  {}  — {}  ({} bytes)", file, title, size),
            );
        }
        return Ok(());
    }

    let (file, title) = match DOC_FILES.iter().find(|(f, _)| *f == arg) {
        Some(found) => found,
        None => {
            write_error(
                ctx.renderer,
                format!("unknown doc: {} (try /docs list)", arg),
            );
            return Ok(());
        }
    };

    let path = docs_dir.join(file);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            write_error(ctx.renderer, format!("failed to read {}: {}", file, e));
            return Ok(());
        }
    };

    write_ok(ctx.renderer, format!("── {} ──", title));
    write_result(ctx.renderer, "");

    for line in content.lines() {
        write_result(ctx.renderer, line);
    }

    Ok(())
}
