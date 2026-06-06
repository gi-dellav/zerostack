use compact_str::CompactString;
use futures::StreamExt;
use rig::agent::{Agent, MultiTurnStreamItem, StreamingResult};
use rig::completion::{CompletionModel, Message};
use rig::message::ToolResultContent;
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingChat};
use tokio::sync::mpsc;

use crate::event::{AgentEvent, BtwEvent};
use crate::session::{MessageRole, Session};

pub struct AgentRunner {
    pub event_rx: mpsc::Receiver<AgentEvent>,
    /// Cancels the underlying agent task. Without this a superseded or
    /// interrupted run keeps driving its stream — and therefore keeps executing
    /// tools (edit/write/bash) — invisibly. Aborting stops it for real.
    pub abort_handle: tokio::task::AbortHandle,
}

/// Handle to an in-flight `/btw` side-question task. The `abort_handle` lets the
/// UI cancel the side question (e.g. on Ctrl-C) without touching the main agent.
pub struct BtwRunner {
    pub abort_handle: tokio::task::AbortHandle,
}

/// Spawn an isolated, single-turn, tool-less side-question run. The full result
/// is delivered as a single [`BtwEvent::Done`] (or [`BtwEvent::Error`]) tagged
/// with `id`. Unlike [`spawn_agent`], it never registers a subagent event sink
/// and never mutates the session.
pub fn spawn_btw<M, P>(
    agent: Agent<M, P>,
    prompt: String,
    history: Vec<Message>,
    event_tx: mpsc::Sender<BtwEvent>,
    id: u32,
) -> BtwRunner
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    let join = tokio::spawn(async move {
        let mut stream = agent.stream_chat(prompt, history).await;
        let mut acc = String::new();

        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                    text,
                ))) => acc.push_str(&text.text),
                Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                    let response_text = res.response();
                    let usage = res.usage();
                    let response = if response_text.is_empty() {
                        CompactString::from(acc.as_str())
                    } else {
                        CompactString::from(response_text)
                    };
                    let _ = event_tx
                        .send(BtwEvent::Done {
                            id,
                            response,
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                        })
                        .await;
                    return;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(BtwEvent::Error {
                            id,
                            message: CompactString::new(e.to_string()),
                        })
                        .await;
                    return;
                }
                _ => {}
            }
        }

        let _ = event_tx
            .send(BtwEvent::Error {
                id,
                message: CompactString::new("side question ended without a response"),
            })
            .await;
    });

    BtwRunner {
        abort_handle: join.abort_handle(),
    }
}

pub fn convert_history(session: &Session) -> Vec<Message> {
    let (summary, first_kept) = session.compacted_context();
    let remaining = session.messages.len().saturating_sub(first_kept);
    let extra = if summary.is_some() { 1 } else { 0 };
    let mut messages = Vec::with_capacity(remaining + extra);

    // The compaction summary is emitted as an Assistant message rather
    // than a System message: the agent already has a System preamble
    // (SYSTEM_PROMPT + mode prompt + context files), and some model chat
    // templates (notably Qwen 3.x) refuse any System message past
    // position 0. Assistant role also produces clean User↔Assistant
    // alternation when the next user prompt arrives, which reads as
    // "the agent recaps what it did, then the user continues" — a
    // natural resumed-conversation shape. The "[Recap of my prior work
    // in this conversation]" prefix labels the message as a self-recap
    // so the agent doesn't treat it as a fresh continuation of its own
    // voice.
    if let Some(summary) = summary {
        messages.push(Message::assistant(format!(
            "[Recap of my prior work in this conversation]\n{}",
            summary
        )));
    }

    for msg in &session.messages[first_kept..] {
        match msg.role {
            MessageRole::User => messages.push(Message::user(msg.content.to_string())),
            MessageRole::Assistant => messages.push(Message::assistant(msg.content.to_string())),
            // Convert any persisted System messages to Assistant for the
            // same reason as the summary above: the templates that reject
            // mid-stream System tolerate Assistant, and code-symmetry with
            // the summary push keeps the resumed-conversation shape
            // consistent.
            MessageRole::System => messages.push(Message::assistant(msg.content.to_string())),
        }
    }

    messages
}

async fn continue_prompt_injector<M, P>(
    agent: &Agent<M, P>,
    retry_prompt: &str,
    retry_history: &[Message],
    tool_interactions: &[Message],
) -> StreamingResult<M::StreamingResponse>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    let mut new_history = retry_history.to_vec();
    new_history.extend_from_slice(tool_interactions);
    new_history.push(Message::user(retry_prompt.to_string()));
    new_history.push(Message::assistant(String::new()));
    agent.stream_chat("Please continue.", new_history).await
}

/// Builds the forked context for a `/btw` side question: the committed
/// conversation history, plus — when the main agent is mid-task — a synthesized
/// note describing the in-flight turn so the side question can see what the
/// agent is doing right now. The returned messages are a by-value snapshot; the
/// session is never mutated, so there is nothing to roll back afterwards.
pub fn build_btw_snapshot(
    session: &Session,
    turn_trace: &[CompactString],
    main_running: bool,
) -> Vec<Message> {
    let mut snapshot = convert_history(session);
    if main_running && !turn_trace.is_empty() {
        snapshot.push(Message::user(format!(
            "(Context only — the main assistant is working in parallel right now. \
Its progress so far this turn:\n{}\nThe last step may still be running. Use this \
only if the user's question is about what the main assistant is doing.)",
            turn_trace.join("\n")
        )));
    }
    snapshot
}

pub fn spawn_agent<M, P>(agent: Agent<M, P>, prompt: String, history: Vec<Message>) -> AgentRunner
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(32);

    #[cfg(feature = "subagents")]
    crate::extras::subagents::set_subagent_event_tx(event_tx.clone());

    #[cfg(feature = "advisor")]
    crate::extras::advisor::set_advisor_event_tx(event_tx.clone());

    let join = tokio::spawn(async move {
        let retry_prompt = prompt.clone();
        let retry_history: Vec<Message> = history.clone();
        let mut tool_interactions: Vec<Message> = Vec::new();
        let mut last_tool_name: Option<String> = None;

        let mut stream = agent.stream_chat(prompt, history).await;

        loop {
            while let Some(item) = stream.next().await {
                match item {
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::Text(text),
                    )) => {
                        let _ = event_tx
                            .send(AgentEvent::Token(CompactString::from(text.text)))
                            .await;
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::Reasoning(r),
                    )) => {
                        let _ = event_tx
                            .send(AgentEvent::Reasoning(CompactString::new(r.display_text())))
                            .await;
                    }
                    Ok(MultiTurnStreamItem::StreamAssistantItem(
                        StreamedAssistantContent::ToolCall { tool_call, .. },
                    )) => {
                        last_tool_name = Some(tool_call.function.name.clone());
                        tool_interactions.push(tool_call.clone().into());
                        let _ = event_tx
                            .send(AgentEvent::ToolCall {
                                name: CompactString::from(tool_call.function.name),
                                args: tool_call.function.arguments,
                            })
                            .await;
                    }
                    Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                        tool_result,
                        ..
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
                            .send(AgentEvent::ToolResult {
                                name: CompactString::new(last_tool_name.take().unwrap_or_default()),
                                output: CompactString::from(output),
                            })
                            .await;
                        tool_interactions.push(tool_result.clone().into());
                    }
                    Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                        let response_text = res.response();
                        let usage = res.usage();

                        if !response_text.is_empty() {
                            let _ = event_tx
                                .send(AgentEvent::Done {
                                    response: CompactString::from(response_text),
                                    input_tokens: usage.input_tokens,
                                    output_tokens: usage.output_tokens,
                                })
                                .await;
                            return;
                        }
                        break;
                    }
                    Err(e) => {
                        let _ = event_tx
                            .send(AgentEvent::Error(CompactString::new(e.to_string())))
                            .await;
                        return;
                    }
                    _ => {}
                }
            }

            stream =
                continue_prompt_injector(&agent, &retry_prompt, &retry_history, &tool_interactions)
                    .await;
        }
    });

    AgentRunner {
        event_rx,
        abort_handle: join.abort_handle(),
    }
}

pub async fn run_print<M, P>(
    agent: &Agent<M, P>,
    prompt: &str,
    max_turns: usize,
    pure_stdout: bool,
) -> anyhow::Result<String>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    let mut stream = agent
        .stream_chat(prompt.to_string(), Vec::<Message>::new())
        .multi_turn(max_turns)
        .await;

    let mut full_response = String::new();
    let mut last_tool_name: Option<String> = None;

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text))) => {
                full_response.push_str(&text.text);
                print!("{}", text.text);
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(
                r,
            ))) => {
                eprint!("{}", r.display_text());
                let _ = std::io::Write::flush(&mut std::io::stderr());
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall {
                tool_call,
                ..
            })) if pure_stdout => {
                let name = &tool_call.function.name;
                last_tool_name = Some(name.clone());
                let summary = format_tool_args_summary(&tool_call.function.arguments);
                println!("\n◈ {} {}", name, summary);
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
            Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                tool_result,
                ..
            })) if pure_stdout => {
                let name = last_tool_name.take().unwrap_or_default();
                let mut output = String::new();
                for c in tool_result.content.iter() {
                    if let ToolResultContent::Text(t) = c {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str(&t.text);
                    }
                }
                if !output.is_empty() {
                    println!("◈ {} result:", name);
                    let lines: Vec<&str> = output.lines().collect();
                    if lines.len() > 40 {
                        let truncated: Vec<&str> = lines.iter().take(40).copied().collect();
                        println!("{}", truncated.join("\n"));
                        println!("(truncated {} more lines)", lines.len().saturating_sub(40));
                    } else {
                        println!("{}", output);
                    }
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
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

fn format_tool_args_summary(args_json: &serde_json::Value) -> String {
    match args_json {
        serde_json::Value::Object(obj) => {
            let first_key = [
                "path",
                "file_path",
                "pattern",
                "command",
                "description",
                "content",
                "name",
                "question",
                "prompt",
            ];
            for key in &first_key {
                if let Some(val) = obj.get(*key) {
                    let s = match val {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let truncated: String = if s.len() > 120 {
                        format!("{}...", &s[..117])
                    } else {
                        s
                    };
                    return format!("{}", truncated);
                }
            }
            String::new()
        }
        _ => format!("{}", args_json),
    }
}

/// Run an agent silently (no stdout/stderr printing), collecting the full
/// response text. Used by subagent tasks and advisor.
#[cfg(any(feature = "subagents", feature = "advisor"))]
pub async fn run_subagent<M, P>(
    agent: &Agent<M, P>,
    prompt: &str,
    max_turns: usize,
    event_tx: Option<&mpsc::Sender<AgentEvent>>,
) -> anyhow::Result<String>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    let mut stream = agent
        .stream_chat(prompt.to_string(), Vec::<Message>::new())
        .multi_turn(max_turns)
        .await;

    let mut full_response = String::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(text))) => {
                full_response.push_str(&text.text);
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall {
                tool_call,
                ..
            })) => {
                if let Some(tx) = event_tx {
                    let _ = tx
                        .send(AgentEvent::SubagentToolCall {
                            name: CompactString::from(tool_call.function.name),
                            args: tool_call.function.arguments,
                        })
                        .await;
                }
            }
            Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                full_response = res.response().to_string();
                break;
            }
            Ok(_) => {}
            Err(e) => {
                return Err(anyhow::anyhow!("subagent error: {}", e));
            }
        }
    }

    if full_response.is_empty() {
        anyhow::bail!("subagent returned empty response");
    }

    Ok(full_response)
}
