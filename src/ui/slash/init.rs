use std::io::Write;

use crossterm::ExecutableCommand;

use crate::ui::slash::{SlashCtx, write_error, write_ok};

pub(crate) const AGENTS_CREATION_PROMPT: &str = "\
Create an AGENTS.md file for this project. Read existing AGENTS.md or CLAUDE.md files \
in parent directories, README.md, and any config files to understand the project first. \
Then write a comprehensive AGENTS.md that documents: \
1) the overall purpose and architecture \
2) build/test/lint commands \
3) coding style and conventions \
4) directory layout \
Keep it focused and actionable for a coding agent.";

const AGENTS_DESC: &str = "tells coding agents how to build, test, and work with this codebase";
const ARCHITECTURE_DESC: &str = "documents high-level codebase architecture for AI agents";

fn ask_yn(question: &str) -> bool {
    print!("{question} ");
    std::io::stdout().flush().ok();
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).ok();
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

fn exit_tui_for_io() {
    let _ = crossterm::terminal::disable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = stdout.execute(crossterm::event::DisableMouseCapture);
    let _ = stdout.execute(crossterm::terminal::LeaveAlternateScreen);
    let _ = stdout.flush();
}

fn restore_tui_and_render(ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout();
    let _ = stdout.execute(crossterm::terminal::EnterAlternateScreen);
    let _ = stdout.execute(crossterm::terminal::Clear(
        crossterm::terminal::ClearType::All,
    ));
    let _ = stdout.execute(crossterm::event::EnableMouseCapture);
    let _ = crossterm::terminal::enable_raw_mode();
    crate::ui::events::render_session(ctx.renderer, ctx.session, ctx.cli, ctx.cfg, ctx.context)
}

fn build_question(label: &str, desc: &str, exists: bool, cwd: &std::path::Path) -> String {
    if exists {
        format!("{} already exists ({desc}). Overwrite? [y/N]", label)
    } else {
        format!(
            "No {} found in {} ({desc}). Create one? [y/N]",
            label,
            cwd.display()
        )
    }
}

pub async fn handle(parts: &[&str], ctx: &mut SlashCtx<'_>) -> anyhow::Result<()> {
    let force = parts.len() >= 2 && parts[1] == "force";

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let agents_path = cwd.join("AGENTS.md");
    let arch_path = cwd.join("ARCHITECTURE.md");

    let agents_exists = agents_path.exists();
    let arch_exists = arch_path.exists();

    let (create_agents, create_arch) = if force {
        if agents_exists {
            write_ok(ctx.renderer, "AGENTS.md exists — will overwrite");
        }
        if arch_exists {
            write_ok(ctx.renderer, "ARCHITECTURE.md exists — will overwrite");
        }
        (true, true)
    } else {
        exit_tui_for_io();

        let create_a = ask_yn(&build_question(
            "AGENTS.md",
            AGENTS_DESC,
            agents_exists,
            &cwd,
        ));
        let create_b = ask_yn(&build_question(
            "ARCHITECTURE.md",
            ARCHITECTURE_DESC,
            arch_exists,
            &cwd,
        ));

        restore_tui_and_render(ctx)?;

        (create_a, create_b)
    };

    if create_arch {
        #[cfg(feature = "archmd")]
        {
            if arch_exists {
                let _ = std::fs::remove_file(&arch_path);
            }
            match crate::extras::archmd::create_architecture_template(&cwd) {
                Ok(()) => write_ok(
                    ctx.renderer,
                    format!(
                        "Created {}/ARCHITECTURE.md — edit it to describe the codebase architecture.",
                        cwd.display()
                    ),
                ),
                Err(e) => write_error(
                    ctx.renderer,
                    format!("Failed to create ARCHITECTURE.md: {}", e),
                ),
            }
        }
        #[cfg(not(feature = "archmd"))]
        {
            write_error(
                ctx.renderer,
                "ARCHITECTURE.md creation requires the 'archmd' feature.",
            );
        }
    }

    if create_agents {
        if !ctx.context.prompts.contains_key("code") {
            write_error(
                ctx.renderer,
                "no 'code' prompt found. Run /regen-prompts first.",
            );
            return Ok(());
        }
        write_ok(ctx.renderer, "delegating AGENTS.md creation to agent...");
        return Err(anyhow::anyhow!("DEFER_INIT:{}", AGENTS_CREATION_PROMPT));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::AGENTS_CREATION_PROMPT;

    #[test]
    fn test_prompt_is_non_empty() {
        assert!(!AGENTS_CREATION_PROMPT.is_empty());
    }

    #[test]
    fn test_prompt_contains_key_phrases() {
        assert!(AGENTS_CREATION_PROMPT.contains("AGENTS.md"));
        assert!(AGENTS_CREATION_PROMPT.contains("build"));
        assert!(AGENTS_CREATION_PROMPT.contains("test"));
        assert!(AGENTS_CREATION_PROMPT.contains("coding agent"));
    }

    #[test]
    fn test_prompt_starts_with_create() {
        assert!(AGENTS_CREATION_PROMPT.starts_with("Create an AGENTS.md"));
    }
}
