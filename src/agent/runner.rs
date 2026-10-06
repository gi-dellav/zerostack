use std::collections::HashMap;

use compact_str::CompactString;
use futures::StreamExt;
use rig::agent::{Agent, MultiTurnStreamItem, StreamingResult};
#[cfg(feature = "multimodal")]
use rig::completion::message::{AudioMediaType, DocumentMediaType, ImageMediaType};
use rig::completion::{Message, Usage};
use rig::message::ToolResultContent;
use rig::streaming::{Item, StreamEvent, StreamedUserContent};
use tokio::sync::mpsc;

use crate::event::{AgentEvent, BtwEvent};
#[cfg(feature = "hooks")]
use crate::extras::hooks::LoopInfo;
use crate::retry::{self, RetryConfig};
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

/// Start one streamed run of `agent` for `prompt` over `history`, retrying a
/// retryable first-item failure per `retry_config`.
async fn start_stream(
    agent: &Agent,
    prompt: String,
    history: Vec<Message>,
    retry_config: &RetryConfig,
) -> Result<StreamingResult, anyhow::Error> {
    retry::retry_stream_chat(retry_config, || {
        let p = prompt.clone();
        let h = history.clone();
        async move { agent.prompt(p).history(h).stream() }
    })
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Spawn an isolated, single-turn, tool-less side-question run. The full result
/// is delivered as a single [`BtwEvent::Done`] (or [`BtwEvent::Error`]) tagged
/// with `id`. Unlike [`spawn_agent`], it never registers a subagent event sink
/// and never mutates the session.
pub fn spawn_btw(
    agent: Agent,
    prompt: String,
    history: Vec<Message>,
    event_tx: mpsc::Sender<BtwEvent>,
    id: u32,
    retry_config: RetryConfig,
) -> BtwRunner {
    let join = tokio::spawn(async move {
        let mut stream = match start_stream(&agent, prompt, history, &retry_config).await {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx
                    .send(BtwEvent::Error {
                        id,
                        message: CompactString::new(e.to_string()),
                    })
                    .await;
                return;
            }
        };

        let mut acc = String::new();

        while let Some(item) = stream.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                    text,
                    ..
                }))) => acc.push_str(&text),
                Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                    let usage = res.usage();
                    let response_text = res.output;
                    let response = if response_text.is_empty() {
                        CompactString::from(acc.as_str())
                    } else {
                        CompactString::from(response_text)
                    };
                    let _ = event_tx
                        .send(BtwEvent::Done {
                            id,
                            response,
                            input_tokens: usage.input_tokens.unwrap_or(0),
                            output_tokens: usage.output_tokens.unwrap_or(0),
                            cached_input_tokens: usage.cached_input_tokens.unwrap_or(0),
                            cache_creation_input_tokens: usage
                                .cache_creation_input_tokens
                                .unwrap_or(0),
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
            // Convert non-user transcript records to Assistant for the
            // same reason as the summary above: the templates that reject
            // mid-stream System/tool roles tolerate Assistant, and code-symmetry with
            // the summary push keeps the resumed-conversation shape
            // consistent.
            MessageRole::System => messages.push(Message::assistant(msg.content.to_string())),
            MessageRole::ToolCall => {
                messages.push(Message::assistant(format!("[ToolCall]: {}", msg.content)))
            }
            MessageRole::ToolResult => {
                messages.push(Message::assistant(format!("[ToolResult]: {}", msg.content)))
            }
            MessageRole::SubagentToolCall => messages.push(Message::assistant(format!(
                "[SubagentToolCall]: {}",
                msg.content
            ))),
        }
    }

    messages
}

#[cfg(feature = "multimodal")]
pub fn media_to_messages(media: &[crate::extras::multimodal::MediaAttachment]) -> Vec<Message> {
    use rig::completion::message::UserContent;

    media
        .iter()
        .map(|m| match m {
            crate::extras::multimodal::MediaAttachment::Image { data, mime, .. } => Message::User {
                content: vec![UserContent::image_raw(
                    data.clone(),
                    Some(image_media_type(mime)),
                    None,
                )],
            },
            crate::extras::multimodal::MediaAttachment::Audio { data, mime, .. } => Message::User {
                content: vec![UserContent::audio_raw(
                    data.clone(),
                    Some(audio_media_type(mime)),
                )],
            },
            crate::extras::multimodal::MediaAttachment::Document { data, mime, .. } => {
                Message::User {
                    content: vec![UserContent::document_raw(
                        data.clone(),
                        Some(document_media_type(mime)),
                    )],
                }
            }
        })
        .collect()
}

#[cfg(feature = "multimodal")]
fn image_media_type(mime: &str) -> ImageMediaType {
    match mime {
        "image/png" => ImageMediaType::PNG,
        "image/jpeg" => ImageMediaType::JPEG,
        "image/gif" => ImageMediaType::GIF,
        "image/webp" => ImageMediaType::WEBP,
        other => {
            tracing::warn!("unknown image mime type: {other}, defaulting to PNG");
            ImageMediaType::PNG
        }
    }
}

#[cfg(feature = "multimodal")]
fn audio_media_type(mime: &str) -> AudioMediaType {
    match mime {
        "audio/mpeg" => AudioMediaType::MP3,
        "audio/wav" => AudioMediaType::WAV,
        "audio/ogg" => AudioMediaType::OGG,
        "audio/flac" => AudioMediaType::FLAC,
        "audio/mp4" => AudioMediaType::M4A,
        "audio/aac" => AudioMediaType::AAC,
        other => {
            tracing::warn!("unknown audio mime type: {other}, defaulting to MP3");
            AudioMediaType::MP3
        }
    }
}

#[cfg(feature = "multimodal")]
fn document_media_type(mime: &str) -> DocumentMediaType {
    match mime {
        "application/pdf" => DocumentMediaType::PDF,
        other => {
            tracing::warn!("unknown document mime type: {other}, defaulting to PDF");
            DocumentMediaType::PDF
        }
    }
}

async fn continue_prompt_injector<M>(
    agent: &Agent<M>,
    retry_prompt: &str,
    retry_history: &[Message],
    tool_interactions: &[Message],
    retry_config: &RetryConfig,
) -> StreamingResult<M::StreamingResponse>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
{
    let mut new_history = retry_history.to_vec();
    new_history.extend_from_slice(tool_interactions);
    new_history.push(Message::user(retry_prompt.to_string()));
    new_history.push(Message::assistant(String::new()));
    match retry::retry_stream_chat(retry_config, || {
        let h = new_history.clone();
        async move { agent.stream_chat("Please continue.", h).await }
    })
    .await
    {
        Ok(stream) => stream,
        Err(e) => Box::pin(futures::stream::once(async move { Err(e) })),
    }
}

/// Append a streamed `ToolCall`/`ToolResult` message to a turn's interaction
/// history, merging it into the previous message when both are the same role.
///
/// `From<ToolCall>` turns every streamed call into its own `Assistant`
/// message, so a parallel batch (all calls streamed before any result) would
/// otherwise replay as `assistant(tool_calls) -> assistant(tool_calls) -> tool
/// -> tool`. Strict OpenAI-compatible backends (DeepSeek) reject the first
/// `assistant` because its calls are followed by another assistant message
/// instead of their tool results. Merging on insert restores the
/// one-assistant-with-all-calls / one-user-with-all-results wire shape.
/// Sequential rounds stay separate: their tool result sits between the calls,
/// so nothing merges across the boundary.
fn merge_push(interactions: &mut Vec<Message>, message: Message) {
    fn join<T: Clone>(previous: &mut rig::OneOrMany<T>, next: &rig::OneOrMany<T>) {
        *previous = rig::OneOrMany::many(
            previous
                .iter()
                .cloned()
                .chain(next.iter().cloned())
                .collect::<Vec<_>>(),
        )
        .expect("two non-empty message parts merge to a non-empty message");
    }

    match (interactions.last_mut(), message) {
        (
            Some(Message::Assistant {
                content: previous, ..
            }),
            Message::Assistant { content, .. },
        ) => join(previous, &content),
        (Some(Message::User { content: previous }), Message::User { content }) => {
            join(previous, &content)
        }
        (_, message) => interactions.push(message),
    }
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

pub fn spawn_agent(
    agent: Agent,
    prompt: String,
    history: Vec<Message>,
    retry_config: RetryConfig,
    // `--loop` iteration/active state, for the `Stop` hook envelope's
    // `loop_iteration`/`loop_active` fields (per-iteration reset of
    // `stop_hook_active`/the block cap falls out for free: each iteration is
    // a fresh call to this function). `None` outside loop mode.
    #[cfg(feature = "hooks")] loop_info: Option<LoopInfo>,
) -> AgentRunner {
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(32);

    #[cfg(feature = "subagents")]
    crate::extras::subagents::set_subagent_event_tx(event_tx.clone());

    let join = tokio::spawn(async move {
        tracing::debug!(
            "spawn_agent: prompt_len={}, history_len={}, max_attempts={}",
            prompt.len(),
            history.len(),
            retry_config.max_attempts,
        );
        // The conversation handed to rig's own multi-turn loop. Each completed
        // run appends its transcript (`PromptResponse::messages`), so a
        // Stop-hook continuation resumes from the exact committed state.
        let mut conversation: Vec<Message> = history;
        let mut current_prompt = prompt.clone();
        let mut empty_response_count: u32 = 0;
        const MAX_EMPTY_RESPONSES: u32 = 3;
        #[cfg(feature = "hooks")]
        let mut stop_hook_active = false;
        #[cfg(feature = "hooks")]
        let mut consecutive_stop_blocks: u32 = 0;
        #[cfg(feature = "hooks")]
        const MAX_STOP_BLOCKS: u32 = 8;

        loop {
            let mut stream = match start_stream(
                &agent,
                current_prompt.clone(),
                conversation.clone(),
                &retry_config,
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("agent stream failed to start: {e}");
                    let _ = event_tx
                        .send(AgentEvent::Error(CompactString::new(e.to_string())))
                        .await;
                    return;
                }
            };

            // In-flight calls by rig `CallId`. A map, not a single slot:
            // providers may stream a whole batch of parallel `ToolCall`s before
            // any of their results, so pairing by "most recent call" records
            // the wrong name against a result.
            let mut pending_tool_names: HashMap<String, String> = HashMap::new();
            let mut response: Option<rig::agent::PromptResponse> = None;

            while let Some(item) = stream.next().await {
                match item {
                    Ok(MultiTurnStreamItem::StreamAssistantItem(item)) => match item {
                        Item::Event(StreamEvent::Text { text, .. }) => {
                            let _ = event_tx
                                .send(AgentEvent::Token(CompactString::from(text)))
                                .await;
                        }
                        Item::Event(StreamEvent::Reasoning { text, .. }) => {
                            if !text.is_empty() {
                                let _ = event_tx
                                    .send(AgentEvent::Reasoning(CompactString::from(text)))
                                    .await;
                            }
                            StreamedAssistantContent::ToolCall {
                                tool_call,
                                internal_call_id,
                            } => {
                                let tool_name = &tool_call.function.name;
                                tracing::debug!(
                                    "agent tool start: name={}, args_len={}",
                                    tool_name,
                                    tool_call.function.arguments.to_string().len(),
                                );
                                pending_tool_names
                                    .insert(internal_call_id.clone(), tool_name.clone());
                                merge_push(&mut tool_interactions, tool_call.clone().into());
                                let _ = event_tx
                                    .send(AgentEvent::ToolCall {
                                        call_id: CompactString::from(internal_call_id),
                                        name: CompactString::from(tool_call.function.name),
                                        args: tool_call.function.arguments,
                                    })
                                    .await;
                            }
                            _ => {}
                        }
                        _ => {}
                    },
                    Ok(MultiTurnStreamItem::ToolCall { tool_call }) => {
                        let call_id = tool_call.id.to_string();
                        let tool_name = tool_call.function.name.to_string();
                        tracing::debug!(
                            "agent tool start: name={}, args_len={}",
                            tool_name,
                            tool_call.function.arguments.to_string().len(),
                        );
                        pending_tool_names.insert(call_id.clone(), tool_name.clone());
                        let _ = event_tx
                            .send(AgentEvent::ToolCall {
                                call_id: CompactString::from(call_id),
                                name: CompactString::from(tool_name),
                                args: tool_call.function.arguments,
                            })
                            .await;
                    }
                    Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                        tool_result,
                    })) => {
                        let call_id = tool_result.call.to_string();
                        let tool_name = CompactString::new(
                            pending_tool_names
                                .remove(&call_id)
                                .unwrap_or_else(|| tool_result.name.to_string()),
                        );
                        let mut output = String::new();
                        for c in tool_result.content.iter() {
                            if let ToolResultContent::Text(t) = c {
                                if !output.is_empty() {
                                    output.push('\n');
                                }
                                output.push_str(&t.text);
                            }
                        }
                        tracing::debug!(
                            "agent tool result: name={}, output_len={}",
                            tool_name,
                            output.len(),
                        );
                        let _ = event_tx
                            .send(AgentEvent::ToolResult {
                                call_id: CompactString::from(call_id),
                                name: tool_name,
                                output: CompactString::from(output),
                            })
                            .await;
                        merge_push(&mut tool_interactions, tool_result.clone().into());
                    }
                    Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                        let usage = res.usage();
                        let response_text = res.output;
                        tracing::info!(
                            "agent done: input_tokens={}, output_tokens={}, cached_input_tokens={}, cache_creation_input_tokens={}",
                            usage.input_tokens,
                            usage.output_tokens,
                            usage.cached_input_tokens,
                            usage.cache_creation_input_tokens,
                        );

                        if !response_text.is_empty() {
                            #[cfg(feature = "hooks")]
                            if let crate::extras::hooks::StopGate::Continue { reason } =
                                crate::extras::hooks::dispatch_stop(
                                    stop_hook_active,
                                    loop_info.map(|info| u64::from(info.iteration)),
                                    loop_info.map(|info| info.active),
                                )
                                .await
                            {
                                consecutive_stop_blocks += 1;
                                if consecutive_stop_blocks <= MAX_STOP_BLOCKS {
                                    stop_hook_active = true;
                                    tracing::info!(
                                        "hooks: Stop hook forced continuation ({consecutive_stop_blocks}/{MAX_STOP_BLOCKS}): {reason}"
                                    );
                                    next_instruction = Some(reason);
                                    break;
                                }
                                tracing::warn!(
                                    "hooks: Stop block cap ({MAX_STOP_BLOCKS}) reached without progress; forcing release"
                                );
                            }
                            let _ = event_tx
                                .send(AgentEvent::Done {
                                    response: CompactString::from(response_text),
                                    input_tokens: usage.input_tokens,
                                    output_tokens: usage.output_tokens,
                                    cached_input_tokens: usage.cached_input_tokens,
                                    cache_creation_input_tokens: usage.cache_creation_input_tokens,
                                })
                                .await;
                            return;
                        }
                        empty_response_count += 1;
                        if empty_response_count >= MAX_EMPTY_RESPONSES {
                            tracing::warn!(
                                "agent: {MAX_EMPTY_RESPONSES} consecutive empty responses, aborting"
                            );
                            let _ = event_tx
                                .send(AgentEvent::Error(CompactString::from(
                                    "Agent returned empty response too many times, aborting.",
                                )))
                                .await;
                            return;
                        }
                        break;
                    }
                    Ok(MultiTurnStreamItem::CompletionCall(call)) => {
                        let usage = call.usage;
                        tracing::debug!(
                            "agent completion: input_tokens={:?}, output_tokens={:?}",
                            usage.input_tokens,
                            usage.output_tokens,
                        );
                        let _ = event_tx
                            .send(AgentEvent::CompletionCall {
                                input_tokens: usage.input_tokens.unwrap_or(0),
                                output_tokens: usage.output_tokens.unwrap_or(0),
                                cached_input_tokens: usage.cached_input_tokens.unwrap_or(0),
                                cache_creation_input_tokens: usage
                                    .cache_creation_input_tokens
                                    .unwrap_or(0),
                            })
                            .await;
                    }
                    Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                        response = Some(res);
                    }
                    Err(e) => {
                        tracing::error!("agent stream error: {e}");
                        let _ = event_tx
                            .send(AgentEvent::Error(CompactString::new(e.to_string())))
                            .await;
                        return;
                    }
                    _ => {}
                }
            }

            let Some(res) = response else {
                let _ = event_tx
                    .send(AgentEvent::Error(CompactString::from(
                        "agent stream ended without a final response",
                    )))
                    .await;
                return;
            };

            if let Some(messages) = &res.messages {
                conversation.extend(messages.iter().cloned());
            }
            let usage = res.usage();

            if !res.output.is_empty() {
                #[cfg(feature = "hooks")]
                if let crate::extras::hooks::StopGate::Continue { reason } =
                    crate::extras::hooks::dispatch_stop(
                        stop_hook_active,
                        loop_info.map(|info| u64::from(info.iteration)),
                        loop_info.map(|info| info.active),
                    )
                    .await
                {
                    consecutive_stop_blocks += 1;
                    if consecutive_stop_blocks <= MAX_STOP_BLOCKS {
                        stop_hook_active = true;
                        tracing::info!(
                            "hooks: Stop hook forced continuation ({consecutive_stop_blocks}/{MAX_STOP_BLOCKS}): {reason}"
                        );
                        current_prompt = reason;
                        continue;
                    }
                    tracing::warn!(
                        "hooks: Stop block cap ({MAX_STOP_BLOCKS}) reached without progress; forcing release"
                    );
                }
                let _ = event_tx
                    .send(AgentEvent::Done {
                        response: CompactString::from(res.output),
                        input_tokens: usage.input_tokens.unwrap_or(0),
                        output_tokens: usage.output_tokens.unwrap_or(0),
                        cached_input_tokens: usage.cached_input_tokens.unwrap_or(0),
                        cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or(0),
                    })
                    .await;
                return;
            }

            empty_response_count += 1;
            if empty_response_count >= MAX_EMPTY_RESPONSES {
                tracing::warn!(
                    "agent: {MAX_EMPTY_RESPONSES} consecutive empty responses, aborting"
                );
                let _ = event_tx
                    .send(AgentEvent::Error(CompactString::from(
                        "Agent returned empty response too many times, aborting.",
                    )))
                    .await;
                return;
            }
            current_prompt = prompt.clone();
        }
    });

    AgentRunner {
        event_rx,
        abort_handle: join.abort_handle(),
    }
}

/// Headless (`-p`, `--loop`) counterpart to [`spawn_agent`]. Drives rig's own
/// multi-turn loop and prints tokens as they stream. A `Stop` hook forces one
/// more turn by starting a fresh run over the committed transcript.
pub async fn run_print(
    agent: &Agent,
    prompt: &str,
    pure_stdout: bool,
    retry_config: &RetryConfig,
    // Prior turns from a resumed session (e.g. `--continue`), converted via
    // `convert_history`.
    history: Vec<Message>,
    #[cfg(feature = "hooks")] loop_info: Option<LoopInfo>,
) -> anyhow::Result<PrintOutcome> {
    let mut conversation = history;
    #[cfg_attr(not(feature = "hooks"), allow(unused_mut))]
    let mut current_prompt = prompt.to_string();
    let mut full_response = String::new();
    let mut recorded_interactions: Vec<ToolInteraction> = Vec::new();
    // Subagent tool calls reach this loop on a side channel rather than as
    // stream items: `run_subagent` sends them from the tokio task the `task`
    // tool spawned, concurrently with this turn's own stream.
    #[cfg(feature = "subagents")]
    let (subagent_tx, mut subagent_rx) = mpsc::channel::<AgentEvent>(32);
    #[cfg(feature = "subagents")]
    crate::extras::subagents::set_subagent_event_tx(subagent_tx.clone());
    #[cfg(feature = "subagents")]
    let _subagent_tx = subagent_tx;
    #[cfg(feature = "subagents")]
    let mut pending_subagent_calls: Vec<SubagentCall> = Vec::new();
    let mut usage = Usage::default();
    let mut continue_turn = true;
    #[cfg(feature = "hooks")]
    let mut next_instruction: Option<String> = None;
    #[cfg(feature = "hooks")]
    let mut stop_hook_active = false;
    #[cfg(feature = "hooks")]
    let mut consecutive_stop_blocks: u32 = 0;
    #[cfg(feature = "hooks")]
    const MAX_STOP_BLOCKS: u32 = 8;

    while continue_turn {
        continue_turn = false;
        let mut stream = start_stream(
            agent,
            current_prompt.clone(),
            conversation.clone(),
            retry_config,
        )
        .await?;

        // In-flight calls by rig `CallId`: (name, args).
        let mut pending_calls: HashMap<String, (String, serde_json::Value)> = HashMap::new();

        loop {
            // Wait for the next stream item while staying available to the
            // subagent channel.
            #[cfg(feature = "subagents")]
            let next_item = loop {
                tokio::select! {
                    biased;
                    Some(event) = subagent_rx.recv() => {
                        push_subagent_call(&mut pending_subagent_calls, event);
                    }
                    item = stream.next() => break item,
                }
            };
            #[cfg(not(feature = "subagents"))]
            let next_item = stream.next().await;

            let Some(item) = next_item else { break };
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                    text,
                    ..
                }))) => {
                    full_response.push_str(&text);
                    print!("{text}");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
                Ok(MultiTurnStreamItem::StreamAssistantItem(Item::Event(
                    StreamEvent::Reasoning { text, .. },
                ))) => {
                    eprint!("{text}");
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                }
                Ok(MultiTurnStreamItem::ToolCall { tool_call }) => {
                    let call_id = tool_call.id.to_string();
                    let name = tool_call.function.name.to_string();
                    let args = tool_call.function.arguments.clone();
                    if pure_stdout {
                        let summary = format_tool_args_summary(&args);
                        println!("\n◈ {} {}", name, summary);
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                    pending_calls.insert(internal_call_id, (name, args));
                    #[cfg(feature = "hooks")]
                    merge_push(&mut tool_interactions, tool_call.clone().into());
                }
                Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                    tool_result,
                })) => {
                    let call_id = tool_result.call.to_string();
                    let (name, args) = pending_calls.remove(&call_id).unwrap_or_else(|| {
                        tracing::warn!(
                            "tool result with no matching pending call (call_id={id})",
                            id = call_id,
                        );
                        (tool_result.name.to_string(), serde_json::Value::Null)
                    });
                    let mut output = String::new();
                    for c in tool_result.content.iter() {
                        if let ToolResultContent::Text(t) = c {
                            if !output.is_empty() {
                                output.push('\n');
                            }
                            output.push_str(&t.text);
                        }
                    }
                    if pure_stdout && !output.is_empty() {
                        println!("◈ {} result:", name);
                        let lines: Vec<&str> = output.lines().collect();
                        if lines.len() > 40 {
                            let truncated: Vec<&str> = lines.iter().take(40).copied().collect();
                            println!("{}", truncated.join("\n"));
                            println!("(truncated {} more lines)", lines.len().saturating_sub(40));
                        } else {
                            println!("{output}");
                        }
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                    #[cfg(feature = "subagents")]
                    while let Ok(event) = subagent_rx.try_recv() {
                        push_subagent_call(&mut pending_subagent_calls, event);
                    }
                    recorded_interactions.push(ToolInteraction {
                        name,
                        args,
                        output,
                        #[cfg(feature = "subagents")]
                        subagent_calls: std::mem::take(&mut pending_subagent_calls),
                    });
                    #[cfg(feature = "hooks")]
                    merge_push(&mut tool_interactions, tool_result.clone().into());
                }
                Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                    usage = res.usage();
                    if let Some(messages) = &res.messages {
                        conversation.extend(messages.iter().cloned());
                    }
                    #[cfg(feature = "hooks")]
                    if let crate::extras::hooks::StopGate::Continue { reason } =
                        crate::extras::hooks::dispatch_stop(
                            stop_hook_active,
                            loop_info.map(|info| u64::from(info.iteration)),
                            loop_info.map(|info| info.active),
                        )
                        .await
                    {
                        consecutive_stop_blocks += 1;
                        if consecutive_stop_blocks <= MAX_STOP_BLOCKS {
                            stop_hook_active = true;
                            tracing::info!(
                                "hooks: Stop hook forced continuation ({consecutive_stop_blocks}/{MAX_STOP_BLOCKS}): {reason}"
                            );
                            next_instruction = Some(reason);
                            continue_turn = true;
                        } else {
                            tracing::warn!(
                                "hooks: Stop block cap ({MAX_STOP_BLOCKS}) reached without progress; forcing release"
                            );
                        }
                    }
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(anyhow::anyhow!("{e}"));
                }
            }
        }

        #[cfg(feature = "hooks")]
        if continue_turn {
            current_prompt = next_instruction
                .take()
                .unwrap_or_else(|| prompt.to_string());
        }
    }

    println!();
    Ok(PrintOutcome {
        response: full_response,
        usage,
        tool_interactions: recorded_interactions,
    })
}

/// One complete tool call/result round trip from a `run_print` turn: the
/// call's name and complete, untruncated argument JSON, plus the result text
/// it produced.
#[derive(Debug, Clone)]
pub struct ToolInteraction {
    pub name: String,
    pub args: serde_json::Value,
    pub output: String,
    /// The tool calls subagents made while this call was running, in arrival
    /// order.
    #[cfg(feature = "subagents")]
    pub subagent_calls: Vec<SubagentCall>,
}

/// One tool call made by a subagent, as reported by
/// [`AgentEvent::SubagentToolCall`]: name plus complete, untruncated argument
/// JSON.
#[cfg(feature = "subagents")]
#[derive(Debug, Clone)]
pub struct SubagentCall {
    pub name: String,
    pub args: serde_json::Value,
}

/// Collects a `SubagentToolCall` event; any other event on that channel is
/// not something a subagent emits and is ignored.
#[cfg(feature = "subagents")]
fn push_subagent_call(collected: &mut Vec<SubagentCall>, event: AgentEvent) {
    if let AgentEvent::SubagentToolCall { name, args } = event {
        collected.push(SubagentCall {
            name: name.to_string(),
            args,
        });
    }
}

/// [`run_print`]'s return value: the assistant's final response text, token
/// usage, and this turn's ordered tool interactions for the caller to
/// persist into the session.
pub struct PrintOutcome {
    pub response: String,
    pub usage: Usage,
    pub tool_interactions: Vec<ToolInteraction>,
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
                        let mut end = 117;
                        while !s.is_char_boundary(end) {
                            end -= 1;
                        }
                        format!("{}...", &s[..end])
                    } else {
                        s
                    };
                    return truncated.to_string();
                }
            }
            String::new()
        }
        _ => format!("{}", args_json),
    }
}

/// Run an agent silently (no stdout/stderr printing), collecting the full
/// response text. Used by subagent tasks.
#[cfg(feature = "subagents")]
pub async fn run_subagent(
    agent: &Agent,
    prompt: &str,
    max_turns: usize,
    event_tx: Option<&mpsc::Sender<AgentEvent>>,
    retry_config: &RetryConfig,
) -> anyhow::Result<String> {
    let mut stream = retry::retry_stream_chat(retry_config, || {
        let p = prompt.to_string();
        async move {
            agent
                .prompt(p)
                .history(Vec::<Message>::new())
                .max_turns(max_turns)
                .stream()
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("subagent error: {e}"))?;

    let mut full_response = String::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                text,
                ..
            }))) => {
                full_response.push_str(&text);
            }
            Ok(MultiTurnStreamItem::ToolCall { tool_call }) => {
                if let Some(tx) = event_tx {
                    let _ = tx
                        .send(AgentEvent::SubagentToolCall {
                            name: CompactString::from(tool_call.function.name.to_string()),
                            args: tool_call.function.arguments,
                        })
                        .await;
                }
            }
            Ok(MultiTurnStreamItem::FinalResponse(res)) => {
                full_response = res.output.to_string();
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

#[cfg(test)]
mod tests {
    use super::{merge_push, streamed_reasoning_text};
    use rig::OneOrMany;
    use rig::completion::Message;
    use rig::message::{
        AssistantContent, ToolCall, ToolFunction, ToolResult, ToolResultContent, UserContent,
    };
    use rig::streaming::StreamedAssistantContent;

    fn assistant_tool_call(id: &str) -> Message {
        Message::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::ToolCall(ToolCall::new(
                id.to_string(),
                ToolFunction::new("read".to_string(), serde_json::json!({})),
            ))),
        }
    }

    fn user_tool_result(id: &str) -> Message {
        Message::User {
            content: OneOrMany::one(UserContent::ToolResult(ToolResult {
                id: id.to_string(),
                call_id: None,
                content: OneOrMany::one(ToolResultContent::text("ok")),
            })),
        }
    }

    #[test]
    fn merge_push_groups_parallel_tool_calls_and_results() {
        let mut interactions = Vec::new();
        merge_push(&mut interactions, assistant_tool_call("call-1"));
        merge_push(&mut interactions, assistant_tool_call("call-2"));
        merge_push(&mut interactions, user_tool_result("call-1"));
        merge_push(&mut interactions, user_tool_result("call-2"));

        assert_eq!(interactions.len(), 2);
        match &interactions[0] {
            Message::Assistant { content, .. } => assert_eq!(content.len(), 2),
            other => panic!("expected coalesced assistant message, got {other:?}"),
        }
        match &interactions[1] {
            Message::User { content } => assert_eq!(content.len(), 2),
            other => panic!("expected coalesced user message, got {other:?}"),
        }
    }

    #[test]
    fn merge_push_keeps_sequential_rounds_separate() {
        let mut interactions = Vec::new();
        merge_push(&mut interactions, assistant_tool_call("call-1"));
        merge_push(&mut interactions, user_tool_result("call-1"));
        merge_push(&mut interactions, assistant_tool_call("call-2"));
        merge_push(&mut interactions, user_tool_result("call-2"));

        assert_eq!(interactions.len(), 4);
    }

    #[test]
    fn streamed_reasoning_delta_is_forwardable_as_reasoning_text() {
        let content = StreamedAssistantContent::<()>::ReasoningDelta {
            id: Some("rs_demo".to_string()),
            reasoning: "thinking in progress".to_string(),
        };

        assert_eq!(
            streamed_reasoning_text(&content).as_deref(),
            Some("thinking in progress")
        );
    }

    #[test]
    fn empty_reasoning_delta_is_ignored() {
        let content = StreamedAssistantContent::<()>::ReasoningDelta {
            id: None,
            reasoning: String::new(),
        };

        assert!(streamed_reasoning_text(&content).is_none());
    }
}
