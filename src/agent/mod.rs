pub mod tools;

use rig::agent::{Agent, AgentBuilder, MultiTurnStreamItem};
use rig::client::{CompletionClient, ProviderClient};
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
- read: Read file contents
- bash: Execute bash commands
- edit: Make surgical edits to files
- write: Create or overwrite files

Guidelines:
- Use bash for file operations like ls, grep, find
- Use read to examine files before editing
- Use edit for precise changes (old text must match exactly)
- Use write only for new files or complete rewrites
- Be concise
- Show file paths clearly";

pub fn build_agent<M: CompletionModel + 'static>(
    model: M,
    cli: &Cli,
    context: &ContextFiles,
) -> Agent<M> {
    let mut preamble = SYSTEM_PROMPT.to_string();
    if let Some(agents) = &context.agents {
        preamble.push_str("\n\n");
        preamble.push_str(agents);
    }

    let mut builder = AgentBuilder::new(model)
        .preamble(&preamble)
        .max_tokens(cli.max_tokens);

    if let Some(temp) = cli.temperature {
        builder = builder.temperature(temp);
    }

    if cli.no_tools {
        builder.build()
    } else {
        builder
            .tool(tools::ReadTool)
            .tool(tools::WriteTool)
            .tool(tools::EditTool)
            .tool(tools::BashTool)
            .build()
    }
}

pub fn create_model(cli: &Cli) -> anyhow::Result<openrouter::CompletionModel> {
    let client = if let Some(key) = &cli.api_key {
        openrouter::Client::new(key)?
    } else {
        openrouter::Client::from_env()?
    };
    Ok(client.completion_model(&cli.model))
}

pub struct AgentRunner {
    pub event_rx: mpsc::Receiver<AgentEvent>,
}

pub fn convert_history(messages: &[crate::session::SessionMessage]) -> Vec<Message> {
    messages
        .iter()
        .map(|m| match m.role.as_str() {
            "assistant" => Message::assistant(&m.content),
            _ => Message::user(&m.content),
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
                    let _ = event_tx.send(AgentEvent::Token(text.text)).await;
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ToolCall { tool_call, .. },
                )) => {
                    let _ = event_tx
                        .send(AgentEvent::ToolCall {
                            name: tool_call.function.name,
                            args: tool_call.function.arguments,
                        })
                        .await;
                }
                Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                    tool_result, ..
                })) => {
                    let output = tool_result
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            ToolResultContent::Text(t) => Some(t.text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let _ = event_tx
                        .send(AgentEvent::ToolResult { output })
                        .await;
                }
                Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                    let response = res.response().to_string();
                    let _ = event_tx.send(AgentEvent::Done { response }).await;
                    break;
                }
                Err(e) => {
                    let msg = e.to_string();
                    let _ = event_tx.send(AgentEvent::Error(msg)).await;
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
