use std::collections::HashMap;

use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::completion::{CompletionModel, Message};
use rig::providers::{anthropic, gemini, ollama, openai, openrouter};
use rig::streaming::StreamingChat;

use crate::agent::builder;
use crate::agent::prompt;
use crate::agent::runner::{self, AgentRunner};
use crate::auth::{AuthResolver, ProviderKind};
use crate::cli::Cli;
use crate::config::{Config, CustomProviderConfig};
use crate::context::ContextFiles;
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::sandbox::Sandbox;
use crate::session::SessionMessage;

pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
}

pub fn resolve_provider_config(
    name: &str,
    custom_providers: &HashMap<String, CustomProviderConfig>,
) -> anyhow::Result<ProviderConfig> {
    if let Some(custom) = custom_providers.get(name) {
        let kind = ProviderKind::from_name(&custom.provider_type)
            .ok_or_else(|| anyhow::anyhow!("Unknown provider type: {}", custom.provider_type))?;
        return Ok(ProviderConfig {
            kind,
            base_url: Some(custom.base_url.clone()),
            api_key_env: custom.api_key_env.clone(),
        });
    }
    let kind = ProviderKind::from_name(name).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown provider: '{}'. Supported: openrouter, openai, anthropic, gemini, ollama",
            name
        )
    })?;

    Ok(ProviderConfig {
        kind,
        base_url: None,
        api_key_env: None,
    })
}

/// Re-exported for compatibility with existing code
pub fn parse_provider(name: &str) -> Option<ProviderKind> {
    ProviderKind::from_name(name)
}

fn resolve_base_url(config: &ProviderConfig) -> Option<String> {
    config.base_url.clone()
}

pub enum AnyClient {
    OpenRouter(openrouter::Client),
    OpenAI(openai::CompletionsClient),
    Anthropic(anthropic::Client),
    Gemini(gemini::Client),
    Ollama(ollama::Client),
}

impl AnyClient {
    pub fn completion_model(&self, name: impl Into<String>) -> AnyModel {
        let name = name.into();
        match self {
            AnyClient::OpenRouter(c) => AnyModel::OpenRouter(c.completion_model(name)),
            AnyClient::OpenAI(c) => AnyModel::OpenAI(c.completion_model(name)),
            AnyClient::Anthropic(c) => AnyModel::Anthropic(c.completion_model(name)),
            AnyClient::Gemini(c) => AnyModel::Gemini(c.completion_model(name)),
            AnyClient::Ollama(c) => AnyModel::Ollama(c.completion_model(name)),
        }
    }

    pub async fn compress_messages(
        &self,
        model_name: &str,
        messages: &[SessionMessage],
        previous_summary: Option<&str>,
        instructions: Option<&str>,
    ) -> anyhow::Result<String> {
        let conversation = serialize_conversation(messages);
        let conversation = if conversation.len() > 6000 {
            let mut truncated = String::from(&conversation[..6000]);
            truncated.push_str("\n\n... [truncated]");
            truncated
        } else {
            conversation
        };

        let prompt = prompt::COMPACTION_PROMPT
            .replace("{conversation}", &conversation)
            .replace("{previous_summary}", previous_summary.unwrap_or("(none)"))
            .replace("{instructions}", instructions.unwrap_or("(none)"));

        let model = self.completion_model(model_name.to_string());
        let response = summarize_with_model(model, prompt).await?;
        Ok(response)
    }
}

async fn summarize_with_model(model: AnyModel, prompt: String) -> anyhow::Result<String> {
    match model {
        AnyModel::OpenRouter(m) => run_summarizer(m, prompt).await,
        AnyModel::OpenAI(m) => run_summarizer(m, prompt).await,
        AnyModel::Anthropic(m) => run_summarizer(m, prompt).await,
        AnyModel::Gemini(m) => run_summarizer(m, prompt).await,
        AnyModel::Ollama(m) => run_summarizer(m, prompt).await,
    }
}

async fn run_summarizer<M>(model: M, prompt: String) -> anyhow::Result<String>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: Send + Sync + Unpin + Clone + 'static,
{
    let agent = rig::agent::AgentBuilder::new(model)
        .preamble("You are a conversation summarizer.")
        .build();

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

pub enum AnyModel {
    OpenRouter(openrouter::completion::CompletionModel),
    OpenAI(openai::completion::CompletionModel),
    Anthropic(anthropic::completion::CompletionModel),
    Gemini(gemini::completion::CompletionModel),
    Ollama(ollama::CompletionModel),
}

#[derive(Clone)]
pub enum AnyAgent {
    OpenRouter(Agent<openrouter::completion::CompletionModel>),
    OpenAI(Agent<openai::completion::CompletionModel>),
    Anthropic(Agent<anthropic::completion::CompletionModel>),
    Gemini(Agent<gemini::completion::CompletionModel>),
    Ollama(Agent<ollama::CompletionModel>),
}

impl AnyAgent {
    pub async fn run_print(&self, prompt: &str, max_turns: usize) -> anyhow::Result<String> {
        match self {
            AnyAgent::OpenRouter(a) => runner::run_print(a, prompt, max_turns).await,
            AnyAgent::OpenAI(a) => runner::run_print(a, prompt, max_turns).await,
            AnyAgent::Anthropic(a) => runner::run_print(a, prompt, max_turns).await,
            AnyAgent::Gemini(a) => runner::run_print(a, prompt, max_turns).await,
            AnyAgent::Ollama(a) => runner::run_print(a, prompt, max_turns).await,
        }
    }

    pub fn spawn_runner(self, prompt: String, history: Vec<Message>) -> AgentRunner {
        match self {
            AnyAgent::OpenRouter(a) => runner::spawn_agent(a, prompt, history),
            AnyAgent::OpenAI(a) => runner::spawn_agent(a, prompt, history),
            AnyAgent::Anthropic(a) => runner::spawn_agent(a, prompt, history),
            AnyAgent::Gemini(a) => runner::spawn_agent(a, prompt, history),
            AnyAgent::Ollama(a) => runner::spawn_agent(a, prompt, history),
        }
    }
}

pub fn create_client(
    provider_name: &str,
    api_key: Option<&str>,
    custom_providers: &HashMap<String, CustomProviderConfig>,
    config_api_keys: Option<&HashMap<String, String>>,
) -> anyhow::Result<AnyClient> {
    let config = resolve_provider_config(provider_name, custom_providers)?;
    let base_url = resolve_base_url(&config);

    let resolver = AuthResolver::new(config.kind)
        .with_cli_key(api_key)
        .with_env_override(config.api_key_env.as_deref())
        .with_config_keys(config_api_keys);
    let key = resolver.resolve()?;

    match config.kind {
        ProviderKind::OpenAI => build_openai_client(&key, base_url.as_deref()),
        ProviderKind::Anthropic => build_anthropic_client(&key, base_url.as_deref()),
        ProviderKind::Gemini => build_gemini_client(&key, base_url.as_deref()),
        ProviderKind::Ollama => build_ollama_client(&key, base_url.as_deref()),
        ProviderKind::OpenRouter => build_openrouter_client(&key, base_url.as_deref()),
    }
}

pub fn build_openai_client(key: &str, base_url: Option<&str>) -> anyhow::Result<AnyClient> {
    let builder = match base_url {
        Some(u) => openai::CompletionsClient::builder()
            .api_key(key)
            .base_url(u),
        None => openai::CompletionsClient::builder().api_key(key),
    };
    Ok(AnyClient::OpenAI(builder.build()?))
}

fn build_anthropic_client(key: &str, base_url: Option<&str>) -> anyhow::Result<AnyClient> {
    let builder = match base_url {
        Some(u) => anthropic::Client::builder().api_key(key).base_url(u),
        None => anthropic::Client::builder().api_key(key),
    };
    Ok(AnyClient::Anthropic(builder.build()?))
}

fn build_gemini_client(key: &str, base_url: Option<&str>) -> anyhow::Result<AnyClient> {
    let builder = match base_url {
        Some(u) => gemini::Client::builder().api_key(key).base_url(u),
        None => gemini::Client::builder().api_key(key),
    };
    Ok(AnyClient::Gemini(builder.build()?))
}

fn build_ollama_client(key: &str, base_url: Option<&str>) -> anyhow::Result<AnyClient> {
    let ollama_key: ollama::OllamaApiKey = key.into();
    let builder = match base_url {
        Some(u) => ollama::Client::builder().api_key(ollama_key).base_url(u),
        None => ollama::Client::builder().api_key(ollama_key),
    };
    Ok(AnyClient::Ollama(builder.build()?))
}

fn build_openrouter_client(key: &str, base_url: Option<&str>) -> anyhow::Result<AnyClient> {
    let builder = match base_url {
        Some(u) => openrouter::Client::builder().api_key(key).base_url(u),
        None => openrouter::Client::builder().api_key(key),
    };
    Ok(AnyClient::OpenRouter(builder.build()?))
}

#[allow(clippy::too_many_arguments)]
pub async fn build_agent(
    model: AnyModel,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    sandbox: Sandbox,
    reasoning_enabled: bool,
    #[cfg(feature = "mcp")] mcp_manager: Option<&McpClientManager>,
) -> AnyAgent {
    match model {
        AnyModel::OpenRouter(m) => AnyAgent::OpenRouter(
            builder::build_agent_inner(
                m,
                cli,
                cfg,
                context,
                permission,
                ask_tx,
                sandbox.clone(),
                reasoning_enabled,
                #[cfg(feature = "mcp")]
                mcp_manager,
            )
            .await,
        ),
        AnyModel::OpenAI(m) => AnyAgent::OpenAI(
            builder::build_agent_inner(
                m,
                cli,
                cfg,
                context,
                permission,
                ask_tx,
                sandbox.clone(),
                reasoning_enabled,
                #[cfg(feature = "mcp")]
                mcp_manager,
            )
            .await,
        ),
        AnyModel::Anthropic(m) => AnyAgent::Anthropic(
            builder::build_agent_inner(
                m,
                cli,
                cfg,
                context,
                permission,
                ask_tx,
                sandbox.clone(),
                reasoning_enabled,
                #[cfg(feature = "mcp")]
                mcp_manager,
            )
            .await,
        ),
        AnyModel::Gemini(m) => AnyAgent::Gemini(
            builder::build_agent_inner(
                m,
                cli,
                cfg,
                context,
                permission,
                ask_tx,
                sandbox.clone(),
                reasoning_enabled,
                #[cfg(feature = "mcp")]
                mcp_manager,
            )
            .await,
        ),
        AnyModel::Ollama(m) => AnyAgent::Ollama(
            builder::build_agent_inner(
                m,
                cli,
                cfg,
                context,
                permission,
                ask_tx,
                sandbox,
                reasoning_enabled,
                #[cfg(feature = "mcp")]
                mcp_manager,
            )
            .await,
        ),
    }
}
