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
use crate::sandbox::Sandbox;
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;

#[allow(dead_code)]
pub type ZAgent = Agent<openrouter::CompletionModel>;

pub async fn build_agent_inner<M: CompletionModel + 'static>(
    model: M,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    sandbox: Sandbox,
    #[cfg(feature = "mcp")] mcp_manager: Option<&McpClientManager>,
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

    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.display();
        preamble.push_str(&format!("\n\nCurrent working directory: {}", cwd_str));
    }

    let mut builder = AgentBuilder::new(model).preamble(&preamble);

    let max_tokens = cli.resolve_max_tokens(cfg);
    builder = builder.max_tokens(max_tokens);

    let max_turns = cli.resolve_max_agent_turns(cfg);
    builder = builder.default_max_turns(max_turns);

    if let Some(temp) = cli.temperature {
        let clamped = temp.clamp(0.0, 2.0);
        builder = builder.temperature(clamped);
    }

    if cli.resolve_no_tools(cfg) {
        builder.build()
    } else {
        let base_tools: Vec<Box<dyn rig::tool::ToolDyn>> = vec![
            Box::new(tools::ReadTool::new(permission.clone(), ask_tx.clone())),
            Box::new(tools::WriteTool::new(permission.clone(), ask_tx.clone())),
            Box::new(tools::EditTool::new(permission.clone(), ask_tx.clone())),
            Box::new(tools::BashTool::new(permission.clone(), ask_tx.clone(), sandbox.clone())),
            Box::new(tools::GrepTool::new(permission.clone(), ask_tx.clone())),
            Box::new(tools::FindFilesTool::new(permission.clone(), ask_tx.clone())),
            Box::new(tools::ListDirTool::new(permission.clone(), ask_tx.clone())),
            Box::new(tools::WriteTodoList::new(permission.clone(), ask_tx.clone())),
        ];

        #[allow(unused_mut)]
        let mut builder = builder.tools(base_tools);

        #[cfg(feature = "mcp")]
        if let Some(manager) = &mcp_manager {
            let mcp_tools = manager.collect_tools(permission.clone(), ask_tx.clone()).await;
            if !mcp_tools.is_empty() {
                let dyn_tools: Vec<Box<dyn rig::tool::ToolDyn>> = mcp_tools
                    .into_iter()
                    .map(|t| Box::new(t) as Box<dyn rig::tool::ToolDyn>)
                    .collect();
                builder = builder.tools(dyn_tools);
            }
        }

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
