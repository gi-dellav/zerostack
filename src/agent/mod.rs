pub mod tools;

use compact_str::CompactString;
use rig::agent::{Agent, AgentBuilder, MultiTurnStreamItem};
use rig::completion::{CompletionModel, Message};
use rig::message::ToolResultContent;
use rig::providers::openrouter;
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingChat};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::cli::Cli;
use crate::context::ContextFiles;
use crate::event::AgentEvent;

pub const SYSTEM_PROMPT: &str = "\
You are an expert coding assistant. Help users with coding tasks by reading, writing, editing files and running commands.

Respond in the same language the user writes to you.

Formatting rules (NO markdown):
- Show file paths as  path/file.rs:42
- Show code blocks with 3 backticks, language on first line
- Keep responses concise, one paragraph per point
- Use `--` for separators instead of horizontal rules
- Do NOT use headings, bold, italic, or other markdown formatting
- For file contents show the path and relevant lines

Available tools:
- read: Read file contents (supports offset/limit for large files, max 10MB)
- write: Create or overwrite files (creates parent dirs automatically)
- edit: Edit files by exact text match. If old_text appears multiple times, shows all match locations with line numbers. Use replaceAll: true for bulk replace. Handles both LF and CRLF. Shows unified diff.
- bash: Execute bash commands (supports timeout param)
- grep: Search file contents with regex. Respects .gitignore, skips binary files. Supports context_lines param for surrounding context (like grep -C).
- find_files: Find files by regex pattern on filename. Respects .gitignore.
- list_dir: List directory entries with types and sizes. Respects .gitignore. Shows entry count for subdirectories.

Guidelines:
- Use list_dir to explore directory structure
- Use grep to search file contents (add context_lines: 2 for surrounding context)
- Use find_files to locate files by name pattern
- Use read to examine files before editing
- Use edit for precise changes. If old_text is ambiguous (multiple matches), add surrounding lines as context or set replaceAll: true
- Use write only for new files or complete rewrites
- Use bash for running commands, tests, git operations
- Be concise
- Show file paths clearly";

pub type ZAgent = Agent<openrouter::CompletionModel>;

pub fn build_agent<M: CompletionModel + 'static>(
    model: M,
    cli: &Cli,
    cfg: &crate::config::Config,
    context: &ContextFiles,
) -> Agent<M> {
    let mut preamble = SYSTEM_PROMPT.to_string();
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
        builder
            .tool(tools::ReadTool)
            .tool(tools::WriteTool)
            .tool(tools::EditTool)
            .tool(tools::BashTool)
            .tool(tools::GrepTool)
            .tool(tools::FindFilesTool)
            .tool(tools::ListDirTool)
            .build()
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

pub struct AgentRunner {
    pub event_rx: mpsc::Receiver<AgentEvent>,
}

pub fn convert_history(messages: &[crate::session::SessionMessage]) -> Vec<Message> {
    messages
        .iter()
        .map(|m| match m.role {
            crate::session::MessageRole::Assistant => Message::assistant(m.content.to_string()),
            crate::session::MessageRole::User => Message::user(m.content.to_string()),
        })
        .collect()
}

pub fn spawn_agent<M, P>(
    agent: Agent<M, P>,
    prompt: String,
    history: Vec<Message>,
) -> AgentRunner
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(256);

    tokio::spawn(async move {
        let mut stream = agent
            .stream_chat(prompt, history)
            .multi_turn(20)
            .await;

        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::Text(text),
                )) => {
                    let _ = event_tx.send(AgentEvent::Token(CompactString::from(text.text))).await;
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::Reasoning(r),
                )) => {
                    let _ = event_tx.send(AgentEvent::Reasoning(CompactString::new(r.display_text()))).await;
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ToolCall { tool_call, .. },
                )) => {
                    let _ = event_tx
                        .send(AgentEvent::ToolCall {
                            name: CompactString::from(tool_call.function.name),
                            args: tool_call.function.arguments,
                        })
                        .await;
                }
                Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                    tool_result, ..
                })) => {
                    let mut output = String::new();
                    for c in tool_result.content.iter() {
                        if let ToolResultContent::Text(t) = c {
                            if !output.is_empty() {
                                output.push('\n');
                            }
                            output.push_str(&t.text);
                        }
                    }
                    let _ = event_tx
                        .send(AgentEvent::ToolResult { output: CompactString::from(output) })
                        .await;
                }
                Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                    let _ = event_tx.send(AgentEvent::Done {
                        response: CompactString::new(res.response()),
                        tokens: 0,
                        cost: 0.0,
                    }).await;
                    break;
                }
                Err(e) => {
                    let _ = event_tx.send(AgentEvent::Error(CompactString::new(e.to_string()))).await;
                    break;
                }
                _ => {}
            }
        }
    });

    AgentRunner { event_rx }
}

pub async fn run_print<M, P>(
    agent: &Agent<M, P>,
    prompt: &str,
) -> anyhow::Result<String>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    let mut stream = agent
        .stream_chat(prompt.to_string(), Vec::<Message>::new())
        .multi_turn(20)
        .await;

    let mut full_response = String::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                text,
            ))) => {
                full_response.push_str(&text.text);
                print!("{}", text.text);
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::Reasoning(r),
            )) => {
                eprint!("{}", r.display_text());
                let _ = std::io::Write::flush(&mut std::io::stderr());
            }
            Ok(MultiTurnStreamItem::FinalResponse(_)) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error: {}", e);
                break;
            }
        }
    }

    println!();
    Ok(full_response)
}
