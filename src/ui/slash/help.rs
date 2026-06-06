use crate::ui::slash::{SlashCtx, write_ok, write_result};

pub fn handle_welcome(renderer: &mut crate::ui::renderer::Renderer) {
    let _ = crate::ui::events::show_welcome(renderer);
}

pub fn handle(_parts: &[&str], ctx: &mut SlashCtx<'_>) {
    write_ok(ctx.renderer, "commands:");
    write_result(
        ctx.renderer,
        "  /add [path]            add file(s) to context",
    );
    write_result(
        ctx.renderer,
        "  /drop <path>           remove file from context",
    );
    write_result(
        ctx.renderer,
        "  /drop-all              remove all added files from context",
    );
    write_result(
        ctx.renderer,
        "  /init [force]          create AGENTS.md for this project",
    );
    write_result(
        ctx.renderer,
        "  /memory [status|search|read|write|editor|clear]  manage memory",
    );
    write_result(ctx.renderer, "  /clear [/new]          clear screen");
    write_result(
        ctx.renderer,
        "  /provider [name]       show or switch provider",
    );
    write_result(ctx.renderer, "  /models                list quick models");
    write_result(
        ctx.renderer,
        "  /models <name>         switch to a quick model",
    );
    write_result(ctx.renderer, "  /models-add <n> <p> <m> save a quick model");
    write_result(
        ctx.renderer,
        "  /sessions              list recent sessions",
    );
    write_result(
        ctx.renderer,
        "  /sessions <id>         load a session (by ID prefix)",
    );
    write_result(ctx.renderer, "  /sessions delete <id>  delete a session");
    write_result(
        ctx.renderer,
        "  /reasoning             toggle LLM reasoning ability",
    );
    write_result(
        ctx.renderer,
        "  /thinking              alias for /reasoning",
    );
    write_result(
        ctx.renderer,
        "  /mode                  show/change security mode",
    );
    write_result(
        ctx.renderer,
        "  /mode <mode>           set mode (standard|restrictive|readonly|guarded|yolo)",
    );
    #[cfg(feature = "mcp")]
    {
        write_result(
            ctx.renderer,
            "  /mcp                   list MCP servers and tools",
        );
        write_result(
            ctx.renderer,
            "  /mcp <server>          list tools of an MCP server",
        );
    }
    write_result(ctx.renderer, "  /clear [/new]          clear screen");
    write_result(ctx.renderer, "  /undo                  undo last exchange");
    write_result(ctx.renderer, "  /retry                 retry last prompt");
    write_result(
        ctx.renderer,
        "  /compress [/compact]   compress conversation history",
    );
    write_result(
        ctx.renderer,
        "  /compress [instr]      compress with custom instructions",
    );
    write_result(
        ctx.renderer,
        "  /editsys [mode]        edit system (similarity | hashedit)",
    );
    #[cfg(feature = "loop")]
    {
        write_result(
            ctx.renderer,
            "  /loop [prompt]         start iterative coding loop",
        );
        write_result(ctx.renderer, "  /loop stop             stop the loop");
    }
    #[cfg(not(feature = "loop"))]
    {
        write_result(
            ctx.renderer,
            "  /loop [prompt]         start iterative coding loop (req. 'loop' feature)",
        );
    }
    #[cfg(feature = "advisor")]
    {
        crate::extras::advisor::help::write_help(ctx);
    }
    #[cfg(not(feature = "advisor"))]
    {
        write_result(
            ctx.renderer,
            "  /advisor               show advisor status (req. 'advisor' feature)",
        );
    }
    write_result(
        ctx.renderer,
        "  /prompt                list available prompts",
    );
    write_result(ctx.renderer, "  /prompt <name>         activate a prompt");
    write_result(ctx.renderer, "  /prompt default        clear active prompt");
    write_result(
        ctx.renderer,
        "  /theme                 list available themes",
    );
    write_result(ctx.renderer, "  /theme <name>          activate a theme");
    write_result(ctx.renderer, "  /theme default         clear active theme");
    write_result(
        ctx.renderer,
        "  /regen-prompts        restore built-in prompts to global dir",
    );
    write_result(
        ctx.renderer,
        "  /regen-themes         restore built-in themes to config dir",
    );
    #[cfg(feature = "git-worktree")]
    {
        write_result(
            ctx.renderer,
            "  /worktree <name>       create a git worktree on <name> branch and cd into it",
        );
        write_result(
            ctx.renderer,
            "  /wt-merge [branch]     merge worktree branch into [branch] (default: main/master)",
        );
        write_result(
            ctx.renderer,
            "  /wt-exit               exit worktree and return to main repo",
        );
    }
    write_result(
        ctx.renderer,
        "  /history               show global chat history",
    );
    write_result(ctx.renderer, "  /quit [/exit]          exit zerostack");
    write_result(
        ctx.renderer,
        "  /welcome               show the quickstart guide",
    );
    write_result(ctx.renderer, "  /tutorial              alias for /welcome");
    write_result(ctx.renderer, "  /help                  show this message");
    write_result(ctx.renderer, "");
    #[cfg(feature = "subagents")]
    {
        write_result(
            ctx.renderer,
            "  /model-subagent [name] show or switch subagent model",
        );
        write_result(
            ctx.renderer,
            "  /models-subagent       list quick models for subagent",
        );
        write_result(
            ctx.renderer,
            "  /models-subagent <n>   switch subagent to a quick model",
        );
    }
    write_ok(ctx.renderer, "keys:");
    write_result(ctx.renderer, "  PgUp/PgDn             scroll chat history");
    write_result(ctx.renderer, "  Home/End               jump to top/bottom");
    write_result(
        ctx.renderer,
        "  @<query>               file picker (Tab/Enter select, Esc cancel)",
    );
    write_result(
        ctx.renderer,
        "  mouse drag             select text (copies to clipboard on release)",
    );
    write_result(
        ctx.renderer,
        "  Esc (while selected)   clear selection (no copy)",
    );
    write_result(ctx.renderer, "  Ctrl+R                 toggle reasoning");
    write_result(ctx.renderer, "  Ctrl+C / Ctrl+D        interrupt/quit");
    write_result(ctx.renderer, "  mouse scroll           scroll chat");
}
