use std::collections::HashMap;
use std::time::Duration;

use compact_str::CompactString;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::completion::{CompletionModel, Message};
use rig::providers::{anthropic, gemini, ollama, openai, openrouter};
use rig::streaming::StreamingChat;
use tokio::sync::mpsc;

use crate::agent::builder;
use crate::agent::prompt;
use crate::agent::runner::{self, AgentRunner};
use crate::auth::{AuthResolver, ProviderKind};
use crate::cli::Cli;
use crate::config::{ApiStyle, Config, CustomProviderConfig};
use crate::context::ContextFiles;
use crate::event::AgentEvent;
#[cfg(feature = "mcp")]
use crate::extras::mcp::McpClientManager;
use crate::permission::ask::AskSender;
use crate::permission::checker::PermCheck;
use crate::sandbox::Sandbox;
use crate::session::SessionMessage;

pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub api_key_env: Option<CompactString>,
    pub danger_accept_invalid_certs: bool,
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
            danger_accept_invalid_certs: custom.danger_accept_invalid_certs.unwrap_or(false),
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
        danger_accept_invalid_certs: false,
    })
}

/// Re-exported for compatibility with existing code
pub fn parse_provider(name: &str) -> Option<ProviderKind> {
    ProviderKind::from_name(name)
}

/// Pick a sensible default model when targeting `provider`. Priority:
/// a custom gateway's configured `model`, then a quick model targeting this
/// provider (carrying its pricing), then a built-in fallback. Returns
/// (model, Option<(input_cost, output_cost)>), or None if `provider` is unknown
/// and has no configured default. Used both by `/provider` and at startup so a
/// chosen provider never keeps an id that is invalid on it.
pub(crate) fn default_model_for_provider(
    provider: &str,
    cfg: &Config,
) -> Option<(String, Option<(f64, f64)>)> {
    if let Some(c) = cfg.custom_providers_map().get(provider)
        && let Some(m) = &c.model
    {
        return Some((m.to_string(), None));
    }
    // Deterministic: prefer the alphabetically-first quick model for this provider
    // (HashMap iteration order would otherwise be unstable).
    let qm = crate::config::quick_models_map(cfg);
    let mut names: Vec<&String> = qm.keys().collect();
    names.sort();
    for name in names {
        let q = &qm[name];
        if q.provider.as_str() == provider {
            return Some((
                q.model.to_string(),
                Some((q.input_token_cost, q.output_token_cost)),
            ));
        }
    }
    let m = match provider {
        "anthropic" => "claude-sonnet-4-6",
        "openai" => "gpt-5.1",
        "gemini" | "google" => "gemini-2.5-pro",
        "openrouter" => "openrouter/auto", // OpenRouter's always-valid auto-router
        "ollama" => "llama3.1",
        _ => return None,
    };
    Some((m.to_string(), None))
}

fn resolve_base_url(config: &ProviderConfig) -> Option<String> {
    config.base_url.clone()
}

/// rig 0.37 exposes two distinct OpenAI client types:
/// - `openai::Client`            -> Responses API (`/responses`). Real OpenAI,
///   including GPT-5; rig maps `max_tokens` to `max_output_tokens`, so it does
///   not hit the GPT-5 400.
/// - `openai::CompletionsClient` -> Chat Completions API (`/chat/completions`).
///   Most OpenAI-compatible gateways (vLLM / LiteLLM / self-hosted) implement
///   only this endpoint.
///
/// The two cannot share a single type, so we wrap them in an inner enum and let
/// `ApiStyle` decide which one to build.
pub enum OpenAiClient {
    Responses(openai::Client),
    Completions(openai::CompletionsClient),
}

impl OpenAiClient {
    fn completion_model(&self, name: String) -> OpenAiModel {
        match self {
            OpenAiClient::Responses(c) => OpenAiModel::Responses(c.completion_model(name)),
            OpenAiClient::Completions(c) => OpenAiModel::Completions(c.completion_model(name)),
        }
    }
}

pub enum OpenAiModel {
    Responses(openai::responses_api::ResponsesCompletionModel),
    Completions(openai::completion::CompletionModel),
}

impl Clone for OpenAiClient {
    fn clone(&self) -> Self {
        match self {
            OpenAiClient::Responses(c) => OpenAiClient::Responses(c.clone()),
            OpenAiClient::Completions(c) => OpenAiClient::Completions(c.clone()),
        }
    }
}

impl Clone for AnyClient {
    fn clone(&self) -> Self {
        match self {
            AnyClient::OpenRouter(c) => AnyClient::OpenRouter(c.clone()),
            AnyClient::OpenAI(c) => AnyClient::OpenAI(c.clone()),
            AnyClient::Anthropic(c) => AnyClient::Anthropic(c.clone()),
            AnyClient::Gemini(c) => AnyClient::Gemini(c.clone()),
            AnyClient::Ollama(c) => AnyClient::Ollama(c.clone()),
        }
    }
}

#[derive(Clone)]
pub enum OpenAiAgent {
    Responses(Agent<openai::responses_api::ResponsesCompletionModel>),
    Completions(Agent<openai::completion::CompletionModel>),
}

pub enum AnyClient {
    OpenRouter(openrouter::Client),
    OpenAI(OpenAiClient),
    Anthropic(anthropic::Client),
    Gemini(gemini::Client),
    Ollama(ollama::Client),
}

impl AnyClient {
    #[allow(dead_code)]
    pub fn provider_name(&self) -> &'static str {
        match self {
            AnyClient::OpenRouter(_) => "openrouter",
            AnyClient::OpenAI(_) => "openai",
            AnyClient::Anthropic(_) => "anthropic",
            AnyClient::Gemini(_) => "gemini",
            AnyClient::Ollama(_) => "ollama",
        }
    }

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
        AnyModel::OpenAI(m) => match m {
            OpenAiModel::Responses(m) => run_summarizer(m, prompt).await,
            OpenAiModel::Completions(m) => run_summarizer(m, prompt).await,
        },
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
    OpenAI(OpenAiModel),
    Anthropic(anthropic::completion::CompletionModel),
    Gemini(gemini::completion::CompletionModel),
    Ollama(ollama::CompletionModel),
}

#[derive(Clone)]
pub enum AnyAgent {
    OpenRouter(Agent<openrouter::completion::CompletionModel>),
    OpenAI(OpenAiAgent),
    Anthropic(Agent<anthropic::completion::CompletionModel>),
    Gemini(Agent<gemini::completion::CompletionModel>),
    Ollama(Agent<ollama::CompletionModel>),
}

impl AnyAgent {
    pub async fn run_print(&self, prompt: &str, max_turns: usize) -> anyhow::Result<String> {
        match self {
            AnyAgent::OpenRouter(a) => runner::run_print(a, prompt, max_turns).await,
            AnyAgent::OpenAI(a) => match a {
                OpenAiAgent::Responses(a) => runner::run_print(a, prompt, max_turns).await,
                OpenAiAgent::Completions(a) => runner::run_print(a, prompt, max_turns).await,
            },
            AnyAgent::Anthropic(a) => runner::run_print(a, prompt, max_turns).await,
            AnyAgent::Gemini(a) => runner::run_print(a, prompt, max_turns).await,
            AnyAgent::Ollama(a) => runner::run_print(a, prompt, max_turns).await,
        }
    }

    #[cfg(feature = "subagents")]
    pub async fn run_subagent(
        &self,
        prompt: &str,
        max_turns: usize,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
    ) -> anyhow::Result<String> {
        match self {
            AnyAgent::OpenRouter(a) => runner::run_subagent(a, prompt, max_turns, event_tx).await,
            AnyAgent::OpenAI(a) => match a {
                OpenAiAgent::Responses(a) => {
                    runner::run_subagent(a, prompt, max_turns, event_tx).await
                }
                OpenAiAgent::Completions(a) => {
                    runner::run_subagent(a, prompt, max_turns, event_tx).await
                }
            },
            AnyAgent::Anthropic(a) => runner::run_subagent(a, prompt, max_turns, event_tx).await,
            AnyAgent::Gemini(a) => runner::run_subagent(a, prompt, max_turns, event_tx).await,
            AnyAgent::Ollama(a) => runner::run_subagent(a, prompt, max_turns, event_tx).await,
        }
    }

    pub fn spawn_runner(self, prompt: String, history: Vec<Message>) -> AgentRunner {
        match self {
            AnyAgent::OpenRouter(a) => runner::spawn_agent(a, prompt, history),
            AnyAgent::OpenAI(a) => match a {
                OpenAiAgent::Responses(a) => runner::spawn_agent(a, prompt, history),
                OpenAiAgent::Completions(a) => runner::spawn_agent(a, prompt, history),
            },
            AnyAgent::Anthropic(a) => runner::spawn_agent(a, prompt, history),
            AnyAgent::Gemini(a) => runner::spawn_agent(a, prompt, history),
            AnyAgent::Ollama(a) => runner::spawn_agent(a, prompt, history),
        }
    }
}

/// Expands a value that is exactly "${VAR}" to the environment variable's value;
/// any other format is returned as-is. Only whole-string `${VAR}` is supported
/// (the common, safe case) rather than arbitrary interpolation.
fn expand_env(value: &str) -> anyhow::Result<String> {
    if let Some(var) = value.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        std::env::var(var).map_err(|_| {
            anyhow::anyhow!(
                "Environment variable '{var}' (referenced in a custom provider header) is not set"
            )
        })
    } else {
        Ok(value.to_string())
    }
}

/// Builds a shared reqwest client, combining:
/// - `danger_accept_invalid_certs` (from #62; the TLS toggle shared by all providers)
/// - a custom provider's `headers` (values support `${ENV_VAR}` expansion) and `timeout_secs`
///
/// When the provider is not custom (`custom == None`) and TLS is not disabled,
/// the resulting client is equivalent to `reqwest::Client::default()`, so the
/// behavior of existing providers is unchanged.
fn build_http_client(
    provider_name: &str,
    danger_accept_invalid_certs: bool,
    custom: Option<&CustomProviderConfig>,
) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();

    if let Some(cfg) = custom {
        if !cfg.headers.is_empty() {
            let mut headers = HeaderMap::new();
            for (name, raw_value) in &cfg.headers {
                let value = expand_env(raw_value)?;
                let header_name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|e| anyhow::anyhow!("Invalid header name '{name}': {e}"))?;
                let header_value = HeaderValue::from_str(&value)
                    .map_err(|e| anyhow::anyhow!("Invalid value for header '{name}': {e}"))?;
                headers.insert(header_name, header_value);
            }
            builder = builder.default_headers(headers);
        }
        if let Some(secs) = cfg.timeout_secs {
            builder = builder.timeout(Duration::from_secs(secs));
        }
    }

    if danger_accept_invalid_certs {
        tracing::warn!(
            "TLS certificate verification DISABLED for provider '{}' \
             (danger_accept_invalid_certs = true). Connections are vulnerable to MITM.",
            provider_name
        );
        builder = builder.danger_accept_invalid_certs(true);
    }

    builder.build().map_err(Into::into)
}

/// Determines which API style the OpenAI family should use:
/// if `api_style` is set explicitly, honor it; otherwise default to Completions
/// when a base_url is present (i.e. a compatible gateway) and Responses when it
/// is absent (i.e. real api.openai.com).
fn resolve_api_style(base_url: Option<&str>, custom: Option<&CustomProviderConfig>) -> ApiStyle {
    custom.and_then(|c| c.api_style).unwrap_or({
        if base_url.is_some() {
            ApiStyle::Completions
        } else {
            ApiStyle::Responses
        }
    })
}

/// Builds an OpenAI-family client (Responses or Completions) using the
/// already-constructed shared http_client.
fn build_openai_client(
    key: &str,
    base_url: Option<&str>,
    custom: Option<&CustomProviderConfig>,
    http_client: reqwest::Client,
) -> anyhow::Result<OpenAiClient> {
    let style = resolve_api_style(base_url, custom);

    match style {
        ApiStyle::Responses => {
            let client = match base_url {
                Some(u) => openai::Client::builder()
                    .api_key(key)
                    .base_url(u)
                    .http_client(http_client)
                    .build()?,
                None => openai::Client::builder()
                    .api_key(key)
                    .http_client(http_client)
                    .build()?,
            };
            Ok(OpenAiClient::Responses(client))
        }
        ApiStyle::Completions => {
            let client = match base_url {
                Some(u) => openai::CompletionsClient::builder()
                    .api_key(key)
                    .base_url(u)
                    .http_client(http_client)
                    .build()?,
                None => openai::CompletionsClient::builder()
                    .api_key(key)
                    .http_client(http_client)
                    .build()?,
            };
            Ok(OpenAiClient::Completions(client))
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
        .with_config_keys(config_api_keys)
        .with_custom_provider_name(Some(provider_name));
    let key = resolver.resolve()?;

    match config.kind {
        ProviderKind::OpenAI => {
            let custom = custom_providers.get(provider_name);
            let http_client =
                build_http_client(provider_name, config.danger_accept_invalid_certs, custom)?;
            Ok(AnyClient::OpenAI(build_openai_client(
                &key,
                base_url.as_deref(),
                custom,
                http_client,
            )?))
        }
        ProviderKind::Anthropic => build_anthropic_client(&key, base_url.as_deref()),
        ProviderKind::Gemini => build_gemini_client(&key, base_url.as_deref()),
        ProviderKind::Ollama => build_ollama_client(&key, base_url.as_deref()),
        ProviderKind::OpenRouter => build_openrouter_client(&key, base_url.as_deref()),
    }
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

/// Builds an OpenAiModel (Responses / Completions) into the matching OpenAiAgent.
#[allow(clippy::too_many_arguments)]
async fn build_openai_agent(
    model: OpenAiModel,
    cli: &Cli,
    cfg: &Config,
    context: &ContextFiles,
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    sandbox: Sandbox,
    reasoning_enabled: bool,
    #[cfg(feature = "mcp")] mcp_manager: Option<&McpClientManager>,
) -> OpenAiAgent {
    match model {
        OpenAiModel::Responses(m) => OpenAiAgent::Responses(
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
        OpenAiModel::Completions(m) => OpenAiAgent::Completions(
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
            build_openai_agent(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiStyle, CustomProviderConfig};

    fn cfg(api_style: Option<ApiStyle>) -> CustomProviderConfig {
        CustomProviderConfig {
            provider_type: "openai".into(),
            base_url: "https://gw.example/v1".to_string(),
            api_key_env: None,
            danger_accept_invalid_certs: None,
            api_style,
            headers: std::collections::HashMap::new(),
            timeout_secs: None,
            model: None,
        }
    }

    #[test]
    fn defaults_to_responses_without_base_url() {
        assert_eq!(resolve_api_style(None, None), ApiStyle::Responses);
    }

    #[test]
    fn defaults_to_completions_with_base_url() {
        assert_eq!(
            resolve_api_style(Some("https://gw.example/v1"), None),
            ApiStyle::Completions
        );
    }

    #[test]
    fn explicit_style_overrides_base_url_heuristic() {
        let c = cfg(Some(ApiStyle::Responses));
        assert_eq!(
            resolve_api_style(Some("https://gw.example/v1"), Some(&c)),
            ApiStyle::Responses
        );
    }

    #[test]
    fn explicit_completions_overrides_no_base_url() {
        let c = cfg(Some(ApiStyle::Completions));
        assert_eq!(resolve_api_style(None, Some(&c)), ApiStyle::Completions);
    }

    #[test]
    fn expand_env_passthrough() {
        assert_eq!(expand_env("Bearer abc").unwrap(), "Bearer abc");
    }

    #[test]
    fn expand_env_reads_var() {
        // SAFETY: test-only; set and removed within a single test
        unsafe { std::env::set_var("ZS_TEST_HDR", "secret-value") };
        assert_eq!(expand_env("${ZS_TEST_HDR}").unwrap(), "secret-value");
        unsafe { std::env::remove_var("ZS_TEST_HDR") };
    }

    #[test]
    fn expand_env_missing_var_errors() {
        assert!(expand_env("${ZS_DEFINITELY_NOT_SET_98237}").is_err());
    }
}
