use std::io::Write;

use crossterm::ExecutableCommand;

use crate::ui::slash::{SlashCtx, write_ok, write_result};

pub fn handle_welcome(renderer: &mut crate::ui::renderer::Renderer) {
    let _ = crate::ui::events::show_welcome(renderer);
}

pub fn handle_tutor(renderer: &mut crate::ui::renderer::Renderer) {
    match run_tutor() {
        Ok(()) => {}
        Err(e) => {
            let _ = renderer.write_line(&format!("{}", e), crate::ui::slash::C_ERROR);
        }
    }
}

fn run_tutor() -> anyhow::Result<()> {
    let _ = crossterm::terminal::disable_raw_mode();
    let mut stdout = std::io::stdout();
    let _ = stdout.execute(crossterm::event::DisableMouseCapture);
    let _ = stdout.execute(crossterm::terminal::LeaveAlternateScreen);
    let _ = stdout.flush();

    let result = crate::docs::show_get_started();

    let _ = stdout.execute(crossterm::terminal::EnterAlternateScreen);
    let _ = stdout.execute(crossterm::terminal::Clear(
        crossterm::terminal::ClearType::All,
    ));
    let _ = stdout.execute(crossterm::event::EnableMouseCapture);
    let _ = crossterm::terminal::enable_raw_mode();

    result
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
        "  /export [file]         export session to HTML (or .jsonl)",
    );
    write_result(
        ctx.renderer,
        "  /import <file>         import a session from JSONL or JSON",
    );
    write_result(
        ctx.renderer,
        "  /share                 share session as a secret GitHub gist",
    );
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
    write_result(
        ctx.renderer,
        "  /toggle                show toggleable features",
    );
    write_result(
        ctx.renderer,
        "  /toggle todo [on|off]  toggle todo-list tools",
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
        write_result(
            ctx.renderer,
            "  /mcp login <server>    OAuth login to an MCP server",
        );
        write_result(
            ctx.renderer,
            "  /mcp logout <server>   remove a server's stored OAuth token",
        );
    }
    write_result(ctx.renderer, "  /clear [/new]          clear screen");
    write_result(ctx.renderer, "  /undo                  undo last exchange");
    write_result(
        ctx.renderer,
        "  /redo                  restore the last /undo or rewind",
    );
    write_result(
        ctx.renderer,
        "  /rewind                rewind to an earlier turn (picker)",
    );
    write_result(ctx.renderer, "  /retry                 retry last prompt");
    write_result(
        ctx.renderer,
        "  /queue                 list input queued while agent is busy",
    );
    write_result(ctx.renderer, "  /queue clear           clear the queue");
    write_result(
        ctx.renderer,
        "  /queue pop             remove the last queued input",
    );
    write_result(
        ctx.renderer,
        "  /btw <message>         ask a side question in parallel (no session trace)",
    );
    write_result(
        ctx.renderer,
        "  /review [msg]          review code (auto message if omitted)",
    );
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
    #[cfg(feature = "advisor")]
    {
        write_result(ctx.renderer, "  /advisor               show advisor status");
        write_result(
            ctx.renderer,
            "  /advisor on|off        enable or disable advisor",
        );
        write_result(
            ctx.renderer,
            "  /advisor handoff [on|off]  toggle human handoff mode",
        );
        write_result(ctx.renderer, "  /advisor model <name>  set advisor model");
        write_result(
            ctx.renderer,
            "  /advisor max-uses <n>  set max advisor calls per request",
        );
        write_result(
            ctx.renderer,
            "  /advisor context-limit <n>  set max KB sent to advisor",
        );
    }
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
    write_result(
        ctx.renderer,
        "  /prompt                list available prompts",
    );
    write_result(ctx.renderer, "  /prompt <name>         activate a prompt");
    write_result(ctx.renderer, "  /prompt default        clear active prompt");
    write_result(
        ctx.renderer,
        "  /rename <name>         rename current session",
    );
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
    #[cfg(feature = "hooks")]
    write_result(
        ctx.renderer,
        "  /hooks                 show configured hook events and handlers",
    );
    write_result(
        ctx.renderer,
        "  /history               show global chat history",
    );
    write_result(ctx.renderer, "  /quit [/exit]          exit zerostack");
    write_result(
        ctx.renderer,
        "  /welcome               show the quickstart guide",
    );
    write_result(
        ctx.renderer,
        "  /tutor                 open GET_STARTED.md in less",
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
