use crate::agent::prompt::COMPACTION_PROMPT;
use crate::session::SessionMessage;
use rig::agent::AgentBuilder;
use rig::client::CompletionClient;
use rig::providers::openrouter;

fn serialize_conversation(messages: &[SessionMessage]) -> String {
    let mut result = String::new();
    for msg in messages {
        let role_tag = match msg.role {
            crate::session::MessageRole::User => "User",
            crate::session::MessageRole::Assistant => "Assistant",
            crate::session::MessageRole::System => "System",
        };
        result.push_str(&format!("[{}]: {}\n\n", role_tag, msg.content));
    }
    result
}

pub async fn compress_messages(
    client: &openrouter::Client,
    model_name: &str,
    messages: &[SessionMessage],
    previous_summary: Option<&str>,
    instructions: Option<&str>,
) -> anyhow::Result<String> {
    let model = client.completion_model(model_name.to_string());
    let conversation = serialize_conversation(messages);

    // Truncate to ~6000 chars to keep summarization fast
    let conversation = if conversation.len() > 6000 {
        let mut truncated = String::from(&conversation[..6000]);
        truncated.push_str("\n\n... [truncated]");
        truncated
    } else {
        conversation
    };

    let prompt = COMPACTION_PROMPT
        .replace("{conversation}", &conversation)
        .replace("{previous_summary}", previous_summary.unwrap_or("(none)"))
        .replace("{instructions}", instructions.unwrap_or("(none)"));

    let agent = AgentBuilder::new(model)
        .preamble("You are a conversation summarizer.")
        .build();

    use rig::completion::Message;
    use rig::streaming::StreamingChat;
    let mut stream = agent
        .stream_chat(prompt, Vec::<Message>::new())
        .multi_turn(1)
        .await;

    let mut response = String::new();
    use futures::StreamExt;
    while let Some(item) = stream.next().await {
        match item {
            Ok(rig::agent::MultiTurnStreamItem::StreamAssistantItem(
                rig::streaming::StreamedAssistantContent::Text(text),
            )) => response.push_str(&text.text),
            Ok(rig::agent::MultiTurnStreamItem::FinalResponse(res)) => {
                response = res.response().to_string();
                break;
            }
            Err(e) => return Err(anyhow::anyhow!("Compression failed: {}", e)),
            _ => {}
        }
    }

    if response.is_empty() {
        anyhow::bail!("Compression returned empty response");
    }

    Ok(response)
}
