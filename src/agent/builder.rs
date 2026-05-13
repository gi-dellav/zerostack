use compact_str::CompactString;
use rig::agent::{Agent, AgentBuilder};
use rig::completion::CompletionModel;
use rig::providers::openrouter;

use crate::agent::prompt::{SYSTEM_PROMPT, TODO_TOOLS_PROMPT};
use crate::agent::tools;
use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;

pub type ZAgent = Agent<openrouter::CompletionModel>;

pub fn build_agent<M: CompletionModel + 'static>(
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
            builder
                .tool(tools::WriteTodoList)
                .tool(tools::AskUserQuestion)
        } else {
            builder
        };

        builder.build()
    }
}

pub fn create_client(api_key: Option<&str>) -> anyhow::Result<openrouter::Client> {
    let key = api_key
        .map(CompactString::new)
        .or_else(|| std::env::var("OPENROUTER_API_KEY").ok().map(CompactString::new))
        .ok_or_else(|| anyhow::anyhow!(
            "No API key found. Set OPENROUTER_API_KEY environment variable or pass --api-key."
        ))?;
    Ok(openrouter::Client::new(String::from(key))?)
}
