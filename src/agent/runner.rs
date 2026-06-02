use compact_str::CompactString;
use futures::StreamExt;
use rig::agent::{Agent, MultiTurnStreamItem};
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

    if let Some(summary) = summary {
        messages.push(Message::system(format!(
            "[Previous conversation summary]\n{}",
            summary
        )));
    }

    for msg in &session.messages[first_kept..] {
        match msg.role {
            MessageRole::User => messages.push(Message::user(msg.content.to_string())),
            MessageRole::Assistant => messages.push(Message::assistant(msg.content.to_string())),
            MessageRole::System => messages.push(Message::system(msg.content.to_string())),
        }
    }

    messages
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

    let join = tokio::spawn(async move {
        // Clone prompt and history so they're available for a potential retry
        // when the model returns an empty final response.
        let retry_prompt = prompt.clone();
        let retry_history: Vec<Message> = history.clone();

        let mut stream = agent.stream_chat(prompt, history).await;
        let mut last_tool_name: Option<String> = None;
        let mut tool_interactions: Vec<Message> = Vec::new();
        let mut retry_count = 0u8;

        loop {
            let mut retrying = false;

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

                        if response_text.is_empty() {
                            retrying = true;
                            break;
                        }

                        let _ = event_tx
                            .send(AgentEvent::Done {
                                response: CompactString::from(response_text),
                                input_tokens: usage.input_tokens,
                                output_tokens: usage.output_tokens,
                            })
                            .await;
                        return;
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

            if retrying && retry_count < 2 {
                retry_count += 1;
                let mut new_history = retry_history.clone();
                new_history.extend(tool_interactions.clone());
                new_history.push(Message::user(retry_prompt.clone()));
                new_history.push(Message::assistant(String::new()));
                stream = agent.stream_chat("Please continue.", new_history).await;
                continue;
            }

            let _ = event_tx
                .send(AgentEvent::Error(CompactString::new(
                    "Stream ended without final response",
                )))
                .await;
            return;
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
                print!("{}", text.text);
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Reasoning(
                r,
            ))) => {
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

/// Run an agent silently (no stdout/stderr printing), collecting the full
/// response text. Used by subagent tasks.
#[cfg(feature = "subagents")]
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
