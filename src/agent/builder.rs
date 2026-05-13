use rig::agent::{Agent, AgentBuilder};
use rig::completion::CompletionModel;

use crate::agent::prompt::{SYSTEM_PROMPT, TODO_TOOLS_PROMPT};
use crate::agent::tools;
use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;

pub fn build_agent_inner<M: CompletionModel + 'static>(
    model: M,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    todo_tools_enabled: bool,
) -> Agent<M> {
    let mut preamble = SYSTEM_PROMPT.to_string();
    if todo_tools_enabled {
        preamble.push('\n');
        preamble.push_str(TODO_TOOLS_PROMPT);
    }
    if let Some(agents) = &context.agents {
        preamble.push_str("\n\n");
        preamble.push_str(agents);
    }

    let mut builder = AgentBuilder::new(model).preamble(&preamble);

    let max_tokens = cli.resolve_max_tokens(cfg);
    builder = builder.max_tokens(max_tokens);

    if let Some(temp) = cli.temperature {
        let clamped = temp.clamp(0.0, 2.0);
        builder = builder.temperature(clamped);
    }

    if cli.resolve_no_tools(cfg) {
        builder.build()
    } else {
        let builder = builder
            .tool(tools::ReadTool)
            .tool(tools::WriteTool)
            .tool(tools::EditTool)
            .tool(tools::BashTool)
            .tool(tools::GrepTool)
            .tool(tools::FindFilesTool)
            .tool(tools::ListDirTool);

        let builder = if todo_tools_enabled {
            builder.tool(tools::WriteTodoList)
        } else {
            builder
        };

        builder.build()
    }
}
