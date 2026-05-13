use std::collections::HashMap;

use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::completion::{CompletionModel, Message};
use rig::providers::{anthropic, gemini, ollama, openai, openrouter};
use rig::streaming::StreamingChat;

use crate::agent::builder;
use crate::agent::prompt;
use crate::agent::runner::{self, AgentRunner};
use crate::cli::Cli;
use crate::config::{Config, CustomProviderConfig};
use crate::context::ContextFiles;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::session::SessionMessage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProviderKind {
    OpenRouter,
    OpenAI,
    Anthropic,
    Gemini,
    Ollama,
}

pub fn parse_provider(name: &str) -> Option<ProviderKind> {
    match name.to_lowercase().as_str() {
        "openrouter" => Some(ProviderKind::OpenRouter),
        "openai" => Some(ProviderKind::OpenAI),
        "anthropic" => Some(ProviderKind::Anthropic),
        "gemini" | "google" => Some(ProviderKind::Gemini),
        "ollama" => Some(ProviderKind::Ollama),
        _ => None,
    }
}

pub struct ProviderInfo {
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
}

pub fn resolve_provider_info(
    name: &str,
    custom_providers: &HashMap<String, CustomProviderConfig>,
) -> Option<ProviderInfo> {
    if let Some(custom) = custom_providers.get(name) {
        let kind = parse_provider(&custom.provider_type)?;
        return Some(ProviderInfo {
            kind,
            base_url: Some(custom.base_url.clone()),
            api_key_env: custom.api_key_env.clone(),
        });
    }
    let kind = parse_provider(name)?;
    Some(ProviderInfo {
        kind,
        base_url: None,
        api_key_env: None,
    })
}

fn provider_env_var(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAI => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
        ProviderKind::Gemini => "GEMINI_API_KEY",
        ProviderKind::Ollama => "OLLAMA_API_KEY",
        ProviderKind::OpenRouter => "OPENROUTER_API_KEY",
    }
}

fn resolve_api_key(
    kind: ProviderKind,
    api_key_env_override: Option<&str>,
    cli_key: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(key) = cli_key.filter(|k| !k.is_empty()) {
        return Ok(key.to_string());
    }

    let env_var = api_key_env_override
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| provider_env_var(kind));

    if let Ok(key) = std::env::var(env_var)
        && !key.is_empty()
    {
        return Ok(key);
    }

    if kind == ProviderKind::Ollama {
        return Ok(String::new());
    }

    anyhow::bail!(
        "No API key found for {kind:?}. Set the {env_var} environment variable or pass --api-key."
    )
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
    pub async fn run_print(&self, prompt: &str) -> anyhow::Result<String> {
        match self {
            AnyAgent::OpenRouter(a) => runner::run_print(a, prompt).await,
            AnyAgent::OpenAI(a) => runner::run_print(a, prompt).await,
            AnyAgent::Anthropic(a) => runner::run_print(a, prompt).await,
            AnyAgent::Gemini(a) => runner::run_print(a, prompt).await,
            AnyAgent::Ollama(a) => runner::run_print(a, prompt).await,
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
) -> anyhow::Result<AnyClient> {
    let info = resolve_provider_info(provider_name, custom_providers).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown provider: {}. Supported providers: openrouter, openai, anthropic, gemini, ollama",
            provider_name
        )
    })?;

    let key = resolve_api_key(info.kind, info.api_key_env.as_deref(), api_key)?;

    match info.kind {
        ProviderKind::OpenAI => {
            let mut b = openai::CompletionsClient::builder().api_key(&key);
            if let Some(base_url) = &info.base_url {
                b = b.base_url(base_url);
            }
            Ok(AnyClient::OpenAI(b.build()?))
        }
        ProviderKind::Anthropic => {
            let mut b = anthropic::Client::builder().api_key(&key);
            if let Some(base_url) = &info.base_url {
                b = b.base_url(base_url);
            }
            Ok(AnyClient::Anthropic(b.build()?))
        }
        ProviderKind::Gemini => {
            let mut b = gemini::Client::builder().api_key(&key);
            if let Some(base_url) = &info.base_url {
                b = b.base_url(base_url);
            }
            Ok(AnyClient::Gemini(b.build()?))
        }
        ProviderKind::Ollama => {
            let key: ollama::OllamaApiKey = key.as_str().into();
            let mut b = ollama::Client::builder().api_key(key);
            if let Some(base_url) = &info.base_url {
                b = b.base_url(base_url);
            }
            Ok(AnyClient::Ollama(b.build()?))
        }
        ProviderKind::OpenRouter => {
            let mut b = openrouter::Client::builder().api_key(&key);
            if let Some(base_url) = &info.base_url {
                b = b.base_url(base_url);
            }
            Ok(AnyClient::OpenRouter(b.build()?))
        }
    }
}

pub fn build_agent(
    model: AnyModel,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    todo_tools_enabled: bool,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
) -> AnyAgent {
    match model {
        AnyModel::OpenRouter(m) => AnyAgent::OpenRouter(builder::build_agent_inner(
            m, cli, cfg, context, todo_tools_enabled, permission, ask_tx,
        )),
        AnyModel::OpenAI(m) => AnyAgent::OpenAI(builder::build_agent_inner(
            m, cli, cfg, context, todo_tools_enabled, permission, ask_tx,
        )),
        AnyModel::Anthropic(m) => AnyAgent::Anthropic(builder::build_agent_inner(
            m, cli, cfg, context, todo_tools_enabled, permission, ask_tx,
        )),
        AnyModel::Gemini(m) => AnyAgent::Gemini(builder::build_agent_inner(
            m, cli, cfg, context, todo_tools_enabled, permission, ask_tx,
        )),
        AnyModel::Ollama(m) => AnyAgent::Ollama(builder::build_agent_inner(
            m, cli, cfg, context, todo_tools_enabled, permission, ask_tx,
        )),
    }
}
