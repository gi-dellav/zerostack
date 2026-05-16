use compact_str::CompactString;
use rig::agent::{Agent, AgentBuilder};
use rig::completion::CompletionModel;
use rig::providers::openrouter;

use crate::agent::prompt::{SYSTEM_PROMPT, TODO_TOOLS_PROMPT};
use crate::agent::tools;
use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;

#[allow(dead_code)]
pub type ZAgent = Agent<openrouter::CompletionModel>;

pub fn build_agent_inner<M: CompletionModel + 'static>(
    model: M,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
) -> Agent<M> {
    let mut preamble = SYSTEM_PROMPT.to_string();
    preamble.push('\n');
    preamble.push_str(TODO_TOOLS_PROMPT);
    if let Some(agents) = &context.agents {
        preamble.push_str("\n\n");
        preamble.push_str(agents);
    }

    if let Some(prompt) = &context.current_prompt {
        preamble.push_str("\n\n---\n\n");
        preamble.push_str(prompt);
    }

    // Inject current working directory so the agent knows where it is
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.display();
        preamble.push_str(&format!("\n\nCurrent working directory: {}", cwd_str));
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
            .tool(tools::ReadTool::new(permission.clone(), ask_tx.clone()))
            .tool(tools::WriteTool::new(permission.clone(), ask_tx.clone()))
            .tool(tools::EditTool::new(permission.clone(), ask_tx.clone()))
            .tool(tools::BashTool::new(permission.clone(), ask_tx.clone()))
            .tool(tools::GrepTool::new(permission.clone(), ask_tx.clone()))
            .tool(tools::FindFilesTool::new(
                permission.clone(),
                ask_tx.clone(),
            ))
            .tool(tools::ListDirTool::new(permission.clone(), ask_tx.clone()))
            .tool(tools::WriteTodoList::new(permission, ask_tx));

        builder.build()
    }
}

#[allow(dead_code)]
pub fn create_client(api_key: Option<&str>) -> anyhow::Result<openrouter::Client> {
    let key = api_key
        .map(CompactString::new)
        .or_else(|| {
            std::env::var("OPENROUTER_API_KEY")
                .ok()
                .map(CompactString::new)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No API key found. Set OPENROUTER_API_KEY environment variable or pass --api-key."
            )
        })?;
    Ok(openrouter::Client::new(String::from(key))?)
}
