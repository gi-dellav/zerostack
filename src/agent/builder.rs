use compact_str::CompactString;
use rig::agent::{Agent, AgentBuilder};
use rig::completion::CompletionModel;
use rig::providers::openrouter;
use smallvec::SmallVec;

use crate::agent::prompt::{SYSTEM_PROMPT, TODO_TOOLS_PROMPT};
use crate::agent::tools;
use crate::cli::Cli;
use crate::config::Config;
use crate::context::ContextFiles;
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::sandbox::Sandbox;

#[allow(dead_code)]
pub type ZAgent = Agent<openrouter::CompletionModel>;

#[allow(clippy::too_many_arguments)]
pub async fn build_agent_inner<M: CompletionModel + 'static>(
    model: M,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    sandbox: Sandbox,
    reasoning_enabled: bool,
    #[cfg(feature = "mcp")] mcp_manager: Option<&McpClientManager>,
) -> Agent<M> {
    let reasoning_prefix = if reasoning_enabled {
        "You reason carefully and think step-by-step.\n\n"
    } else {
        "You respond concisely without showing your reasoning.\n\n"
    };
    let context_agents = context.agents.as_deref().unwrap_or("");
    #[cfg(feature = "archmd")]
    let context_architecture = context.architecture.as_deref().unwrap_or("");
    let context_prompt = context.current_prompt.as_deref().unwrap_or("");
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let total_len = reasoning_prefix.len()
        + SYSTEM_PROMPT.len()
        + 1
        + TODO_TOOLS_PROMPT.len()
        + if context.agents.is_some() {
            2 + context_agents.len()
        } else {
            0
        }
        + if context.current_prompt.is_some() {
            6 + context_prompt.len()
        } else {
            0
        }
        + if !cwd.is_empty() { 30 + cwd.len() } else { 0 };

    #[cfg(feature = "archmd")]
    let total_len = total_len
        + if context.architecture.is_some() {
            2 + context_architecture.len()
        } else {
            0
        };

    #[cfg(feature = "memory")]
    let total_len = total_len
        + context.memory.as_deref().map_or(0, |m| m.len() + 8) // "\n\n---\n\n" + content
        + crate::agent::prompt::MEMORY_TOOLS_PROMPT.len();

    // Add extra files content to preamble budget
    let extra_files_content: Vec<String> = context
        .extra_files
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .map(|content| format!("Content of {}:\n{}", p.display(), content))
        })
        .collect();
    let extra_files_len: usize = extra_files_content.iter().map(|s| s.len() + 2).sum();
    let total_len = total_len + extra_files_len;

    let mut preamble = String::with_capacity(total_len);
    preamble.push_str(reasoning_prefix);
    preamble.push_str(SYSTEM_PROMPT);
    preamble.push('\n');
    preamble.push_str(TODO_TOOLS_PROMPT);
    if !context_agents.is_empty() {
        preamble.push_str("\n\n");
        preamble.push_str(context_agents);
    }
    #[cfg(feature = "archmd")]
    if !context_architecture.is_empty() {
        preamble.push_str("\n\n");
        preamble.push_str(context_architecture);
    }
    if !context_prompt.is_empty() {
        preamble.push_str("\n\n---\n\n");
        preamble.push_str(context_prompt);
    }
    if !cwd.is_empty() {
        preamble.push_str("\n\nCurrent working directory: ");
        preamble.push_str(&cwd);
    }
    for content in &extra_files_content {
        preamble.push_str("\n\n---\n\n");
        preamble.push_str(content);
    }
    #[cfg(feature = "memory")]
    {
        crate::extras::memory::append_memory_block(&mut preamble, context.memory.as_deref());
        preamble.push_str(crate::agent::prompt::MEMORY_TOOLS_PROMPT);
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
        let max_text_file_size = cfg.max_text_file_size;
        let base_tools: SmallVec<[Box<dyn rig::tool::ToolDyn>; 8]> = SmallVec::from_buf([
            Box::new(tools::ReadTool::new(
                permission.clone(),
                ask_tx.clone(),
                max_text_file_size,
            )),
            Box::new(tools::WriteTool::new(
                permission.clone(),
                ask_tx.clone(),
                max_text_file_size,
            )),
            Box::new(tools::EditTool::new(permission.clone(), ask_tx.clone())),
            Box::new(tools::BashTool::new(
                permission.clone(),
                ask_tx.clone(),
                sandbox.clone(),
            )),
            Box::new(tools::GrepTool::new(permission.clone(), ask_tx.clone())),
            Box::new(tools::FindFilesTool::new(
                permission.clone(),
                ask_tx.clone(),
            )),
            Box::new(tools::ListDirTool::new(permission.clone(), ask_tx.clone())),
            Box::new(tools::WriteTodoList::new(
                permission.clone(),
                ask_tx.clone(),
            )),
        ]);

        let mut builder = builder.tools(base_tools.into_vec());

        #[cfg(feature = "subagents")]
        if cfg.task_enabled.unwrap_or(true) {
            use crate::extras::subagents::task_tool::TaskTool;
            builder = builder.tool(TaskTool::new(permission.clone(), ask_tx.clone()));
        }

        #[cfg(feature = "memory")]
        {
            use crate::extras::memory::{MemoryRead, MemorySearch, MemoryWrite};
            builder = builder
                .tool(MemoryWrite::new(permission.clone(), ask_tx.clone()))
                .tool(MemoryRead::new(permission.clone(), ask_tx.clone()))
                .tool(MemorySearch::new(permission.clone(), ask_tx.clone()));
        }

        #[cfg(feature = "mcp")]
        if let Some(manager) = &mcp_manager {
            let allow_all = cfg.allow_all_mcp_calls.unwrap_or(false);
            if allow_all && let Some(ref perm) = permission {
                perm.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_allow_all_mcp_calls(true);
            }
            let mcp_tools = manager
                .collect_tools(permission.clone(), ask_tx.clone())
                .await;
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

/// Dedicated system prompt for the `/btw` side-assistant. Deliberately NOT the
/// main coding `SYSTEM_PROMPT`: that one is all about using read/write/bash
/// tools, so pairing it with "you have no tools" made the model refuse and tell
/// the user to wait for the main agent. This prompt frames `/btw` as a quick,
/// read-only Q&A helper whose only job is to answer the user's question.
const BTW_SYSTEM_PROMPT: &str = "\
You are a fast side-assistant for quick \"by the way\" questions during a coding \
session. The user pressed /btw to ask you something in parallel with the main \
assistant, WITHOUT interrupting it.

Your only job: answer the user's question directly, briefly, and helpfully, using \
the conversation so far and the project context below. Reply in the user's \
language.

Match your length to the question: greetings, thanks, or yes/no questions get a \
ONE-LINE reply. Do NOT volunteer project setup, build, run, or test instructions \
unless the user explicitly asks how to build or run. The project context below is \
background for answering; it is NOT a script to recite.

This is a read-only side channel: you have read-only tools (read, grep, \
find_files, list_dir) to look things up, but you CANNOT write files, run \
commands, or change anything, and your reply is NOT saved to the conversation. \
Use the read tools when answering needs a file you do not already have in \
context, and keep it to what the question asks. Do NOT attempt or plan the main \
task, and do NOT tell the user to wait for the main assistant; just answer what \
they asked.";

/// Max model turns for a `/btw` side question. Higher than 1 so it can read a
/// file (or grep) and then answer, but small to keep side questions quick.
const BTW_MAX_TURNS: usize = 8;

/// Builds the isolated `/btw` agent: a lightweight read-only Q&A helper with the
/// project context for reference, NO tools, and a single turn. Never mutates the
/// session.
#[allow(clippy::too_many_arguments)]
pub fn build_btw_agent_inner<M: CompletionModel + 'static>(
    model: M,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: &Option<PermCheck>,
    ask_tx: &Option<AskSender>,
    _reasoning_enabled: bool,
) -> Agent<M> {
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let mut preamble = String::new();
    preamble.push_str(BTW_SYSTEM_PROMPT);

    // Project context, for reference only — NOT instructions to act on.
    let has_ctx = context.agents.as_deref().is_some_and(|s| !s.is_empty()) || !cwd.is_empty();
    if has_ctx {
        preamble.push_str("\n\n## Project context (for reference)\n");
    }
    if let Some(agents) = context.agents.as_deref()
        && !agents.is_empty()
    {
        preamble.push_str("\n");
        preamble.push_str(agents);
    }
    #[cfg(feature = "archmd")]
    if let Some(arch) = context.architecture.as_deref()
        && !arch.is_empty()
    {
        preamble.push_str("\n\n");
        preamble.push_str(arch);
    }
    if let Some(p) = context.current_prompt.as_deref()
        && !p.is_empty()
    {
        preamble.push_str("\n\n");
        preamble.push_str(p);
    }
    if !cwd.is_empty() {
        preamble.push_str("\n\nCurrent working directory: ");
        preamble.push_str(&cwd);
    }
    #[cfg(feature = "memory")]
    crate::extras::memory::append_memory_block(&mut preamble, context.memory.as_deref());

    let max_tokens = cli.resolve_max_tokens(cfg);

    // Honor --no-tools: fall back to a pure-context, single-turn answer.
    if cli.resolve_no_tools(cfg) {
        let mut builder = AgentBuilder::new(model)
            .preamble(&preamble)
            .default_max_turns(1)
            .max_tokens(max_tokens);
        if let Some(temp) = cli.temperature {
            builder = builder.temperature(temp.clamp(0.0, 2.0));
        }
        return builder.build();
    }

    // Read-only tools only (read/grep/find_files/list_dir): a side question can
    // look things up, but has no write/edit/bash, so it still has no side
    // effects to roll back and never mutates the session. Allow multiple turns
    // so it can read then answer.
    let max_text_file_size = cfg.max_text_file_size;
    let read_tools: Vec<Box<dyn rig::tool::ToolDyn>> = vec![
        Box::new(tools::ReadTool::new(
            permission.clone(),
            ask_tx.clone(),
            max_text_file_size,
        )),
        Box::new(tools::GrepTool::new(permission.clone(), ask_tx.clone())),
        Box::new(tools::FindFilesTool::new(
            permission.clone(),
            ask_tx.clone(),
        )),
        Box::new(tools::ListDirTool::new(permission.clone(), ask_tx.clone())),
    ];

    let mut builder = AgentBuilder::new(model)
        .preamble(&preamble)
        .default_max_turns(BTW_MAX_TURNS)
        .max_tokens(max_tokens)
        .tools(read_tools);

    if let Some(temp) = cli.temperature {
        builder = builder.temperature(temp.clamp(0.0, 2.0));
    }

    builder.build()
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
