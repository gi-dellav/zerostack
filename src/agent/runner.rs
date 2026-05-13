use compact_str::CompactString;
use futures::StreamExt;
use rig::agent::{Agent, MultiTurnStreamItem};
use rig::completion::{CompletionModel, Message};
use rig::message::ToolResultContent;
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingChat};
use tokio::sync::mpsc;

use crate::event::AgentEvent;
use crate::session::{MessageRole, Session};

pub struct AgentRunner {
    pub event_rx: mpsc::Receiver<AgentEvent>,
}

pub fn convert_history(session: &Session) -> Vec<Message> {
    let (summary, first_kept) = session.compacted_context();
    let mut messages = Vec::new();

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

pub fn spawn_agent<M, P>(agent: Agent<M, P>, prompt: String, history: Vec<Message>) -> AgentRunner
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
    P: rig::agent::PromptHook<M> + 'static,
{
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(256);

    tokio::spawn(async move {
        let mut stream = agent.stream_chat(prompt, history).multi_turn(20).await;

        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                    text,
                ))) => {
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
                            output: CompactString::from(output),
                        })
                        .await;
                }
                Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                    let response_text = res.response();
                    let estimated_tokens = Session::estimate_tokens(response_text);
                    let _ = event_tx
                        .send(AgentEvent::Done {
                            response: CompactString::from(response_text),
                            tokens: estimated_tokens,
                            cost: 0.0,
                        })
                        .await;
                    break;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(AgentEvent::Error(CompactString::new(e.to_string())))
                        .await;
                    break;
                }
                _ => {}
            }
        }
    });

    AgentRunner { event_rx }
}

pub async fn run_print<M, P>(agent: &Agent<M, P>, prompt: &str) -> anyhow::Result<String>
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
