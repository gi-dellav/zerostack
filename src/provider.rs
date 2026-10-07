use std::collections::HashMap;
use std::time::Duration;

use compact_str::CompactString;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rig::agent::Agent;
use rig::completion::Message;
use rig::model::{ModelInfo, ModelList};
use rig::operation::Completion;
use rig::providers::openai::wire::{OPENROUTER, OpenAIConfig, Route};
use rig::{
    DynModel,
    providers::{anthropic, gemini, ollama, openai},
};
use tokio::sync::mpsc;

use crate::agent::builder::{self, AgentBuild};
use crate::agent::prompt;
use crate::agent::runner::{self, AgentRunner};
use crate::auth::{AuthResolver, ProviderKind};
use crate::config::{ApiStyle, Config, CustomProviderConfig};
#[cfg(any(feature = "hooks", feature = "subagents"))]
use crate::event::AgentEvent;
#[cfg(feature = "hooks")]
use crate::extras::hooks::LoopInfo;
use crate::retry::{self, RetryConfig};
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
        let kind = ProviderKind::from_name(&custom.provider_type).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown provider type: {}. Run `zerostack --setup` to configure providers.",
                custom.provider_type
            )
        })?;
        return Ok(ProviderConfig {
            kind,
            base_url: Some(custom.base_url.clone()),
            api_key_env: custom.api_key_env.clone(),
            danger_accept_invalid_certs: custom.danger_accept_invalid_certs.unwrap_or(false),
        });
    }
    let kind = ProviderKind::from_name(name).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown provider: '{}'. Supported: openrouter, openai, anthropic, gemini, ollama. Run `zerostack --setup` to configure providers.",
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
    let qm = crate::config::quick_models_map_ref(cfg);
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

/// rig 0.43 exposes OpenAI-family models through one client type:
/// `openai::OpenAI` (an `OpenAIConfig` on a transport). The completion
/// endpoint is chosen per model via [`Route`]:
/// - `Route::Responses` (`/responses`) — real OpenAI, including GPT-5; rig
///   maps `max_tokens` to `max_output_tokens`, so it does not hit the GPT-5 400.
/// - `Route::Chat` (`/chat/completions`) — most OpenAI-compatible gateways
///   (vLLM / LiteLLM / self-hosted) implement only this endpoint.
///
/// [`ApiStyle`] decides which route a model uses; [`AnyModel`] carries models
/// already built for the right route, so no client enum is needed.
pub type OpenAiClient = openai::OpenAI;

/// A completion model: an erased `DynModel<Completion>` plus OpenRouter-only
/// extra body params (see [`openrouter_anthropic_routing`]). `None` for every
/// other provider.
#[derive(Clone)]
pub struct OpenRouterModel {
    pub model: DynModel<Completion>,
    pub extra: Option<serde_json::Value>,
}

#[derive(Clone)]
pub enum OpenAiModel {
    Responses(DynModel<Completion>),
    Completions(DynModel<Completion>),
}

impl OpenAiModel {
    fn as_dyn(&self) -> DynModel<Completion> {
        match self {
            OpenAiModel::Responses(m) => m.clone(),
            OpenAiModel::Completions(m) => m.clone(),
        }
    }
}

#[derive(Clone)]
pub enum OpenAiAgent {
    Responses(Agent),
    Completions(Agent),
}

#[derive(Clone)]
pub enum AnyClient {
    OpenRouter(openai::OpenAI),
    OpenAI(OpenAiClient),
    Anthropic(anthropic::Anthropic),
    Gemini(gemini::Gemini),
    Ollama(ollama::Ollama),
}

/// Extra OpenRouter request body params that pin a Claude model to the
/// Anthropic direct route, or `None` for any non-Claude model.
///
/// `cache_control` breakpoints (used for prompt caching) are only honored on
/// OpenRouter's Anthropic direct route; the Bedrock and Vertex routes silently
/// drop them. So for Claude models we force `provider.order = ["Anthropic"]`
/// (keeping `allow_fallbacks: true` so the request still succeeds if Anthropic
/// is momentarily unavailable). Every other OpenRouter model caches
/// automatically and is left untouched.
///
/// OpenRouter namespaces Claude under `anthropic/`, optionally with a leading
/// `~` marking a floating "-latest" alias (e.g. `~anthropic/claude-sonnet-latest`).
/// The `~` is part of the real slug, so strip it before matching.
pub(crate) fn openrouter_anthropic_routing(model_id: &str) -> Option<serde_json::Value> {
    let slug = model_id.strip_prefix('~').unwrap_or(model_id);
    slug.starts_with("anthropic/").then(|| {
        serde_json::json!({
            "provider": { "order": ["Anthropic"], "allow_fallbacks": true }
        })
    })
}

/// Shallow-merges user-configured `extra_body` into provider-internal routing
/// params (e.g. OpenRouter's `provider.order`). Top-level keys from `extra_body`
/// win on collision. Returns `None` when both are absent so callers can avoid an
/// empty `additional_params` call.
pub(crate) fn merge_extra_body(
    base: Option<serde_json::Value>,
    extra: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    match (base, extra) {
        (Some(serde_json::Value::Object(mut b)), Some(serde_json::Value::Object(e))) => {
            b.extend(e);
            Some(serde_json::Value::Object(b))
        }
        (base, None) => base,
        (None, extra) => extra,
        // Non-object base (shouldn't happen for routing) — user value takes over.
        (Some(_), extra) => extra,
    }
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
            AnyClient::OpenRouter(c) => {
                let extra = openrouter_anthropic_routing(&name);
                let mut wire_model = c.completion(name);
                if extra.is_some() {
                    wire_model.wire = wire_model.wire.with_prompt_caching();
                }
                AnyModel::OpenRouter(OpenRouterModel {
                    model: wire_model.erase(),
                    extra,
                })
            }
            AnyClient::OpenAI(c) => AnyModel::OpenAI(match c.config().route {
                Some(Route::Chat) => OpenAiModel::Completions(c.completion(name).erase()),
                _ => OpenAiModel::Responses(c.completion(name).erase()),
            }),
            AnyClient::Anthropic(c) => {
                let mut wire_model = c.completion(name);
                wire_model.wire = wire_model.wire.with_prompt_caching();
                AnyModel::Anthropic(wire_model.erase())
            }
            AnyClient::Gemini(c) => AnyModel::Gemini(c.completion(name).erase()),
            AnyClient::Ollama(c) => AnyModel::Ollama(c.completion(name).erase()),
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

        let prompt = prompt::COMPACTION_PROMPT
            .replace("{conversation}", &conversation)
            .replace("{previous_summary}", previous_summary.unwrap_or("(none)"))
            .replace("{instructions}", instructions.unwrap_or("(none)"));

        let model = self.completion_model(model_name.to_string());
        let response = summarize_with_model(model, prompt).await?;
        Ok(response)
    }
}

#[derive(Clone)]
pub struct ModelEntry {
    pub id: String,
    pub display: String,
    pub context_length: Option<u32>,
    pub kind: Option<String>,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
}

impl ModelEntry {
    fn from_rig(m: &ModelInfo) -> Self {
        Self {
            id: m.id.clone(),
            display: m.display_name().to_string(),
            context_length: m.context_length,
            kind: m.r#type.clone(),
            input_price: None,
            output_price: None,
        }
    }
}

/// Chat/completion model suitable as an agent (not embedding/image/audio/etc.)?
pub fn is_agent_model(m: &ModelEntry) -> bool {
    if let Some(t) = m.kind.as_deref() {
        let t = t.to_lowercase();
        if [
            "embed",
            "image",
            "audio",
            "video",
            "moderation",
            "rerank",
            "tts",
            "speech",
        ]
        .iter()
        .any(|k| t.contains(k))
        {
            return false;
        }
    }
    let id = m.id.to_lowercase();
    const DENY: &[&str] = &[
        "embedding",
        "embed-",
        "text-embedding",
        "gemini-embedding",
        "whisper",
        "transcribe",
        "tts",
        "-audio",
        "realtime",
        "speech",
        "dall-e",
        "gpt-image",
        "image-generation",
        "imagen",
        "sora",
        "veo",
        "moderation",
        "rerank",
        "aqa",
        "davinci-002",
        "babbage-002",
    ];
    !DENY.iter().any(|d| id.contains(d))
}

impl AnyClient {
    /// Built-in providers: rig's per-client `list_models`.
    pub async fn list_models(&self) -> anyhow::Result<Vec<ModelEntry>> {
        let list: ModelList = match self {
            AnyClient::OpenAI(c) => c.list_models().await?,
            AnyClient::Anthropic(c) => c.list_models().await?,
            AnyClient::OpenRouter(c) => c.list_models().await?,
            AnyClient::Gemini(c) => c.list_models().await?,
            AnyClient::Ollama(c) => c.list_models().await?,
        };
        Ok(list.iter().map(ModelEntry::from_rig).collect())
    }
}

/// Response shape of `GET {base}/models` on OpenAI/OpenRouter-compatible
/// gateways. Only `id` and `context_length` are read; everything else is
/// ignored, so plain OpenAI-style endpoints (no `context_length`) parse fine.
#[derive(serde::Deserialize)]
struct ManualModelsResp {
    data: Vec<ManualModelsItem>,
}

#[derive(serde::Deserialize)]
struct ManualModelsItem {
    id: String,
    context_length: Option<u64>,
}

/// Custom / OpenAI-compatible gateway: best-effort GET {base}/models.
#[allow(dead_code)]
pub async fn list_models_manual(
    provider_name: &str,
    cli_key: Option<&str>,
    custom_providers: &std::collections::HashMap<String, CustomProviderConfig>,
    config_api_keys: Option<&std::collections::HashMap<String, String>>,
) -> anyhow::Result<Vec<ModelEntry>> {
    let (models, _) =
        fetch_custom_models_raw(provider_name, cli_key, custom_providers, config_api_keys).await?;
    Ok(models)
}

/// Single GET for custom gateways that returns both model list and pricing.
/// Avoids the duplicate `GET /models` that `fetch_models_cached` previously
/// did (one for listing, one for `fetch_live_model_info`).
pub(crate) async fn fetch_custom_models_raw(
    provider_name: &str,
    cli_key: Option<&str>,
    custom_providers: &std::collections::HashMap<String, CustomProviderConfig>,
    config_api_keys: Option<&std::collections::HashMap<String, String>>,
) -> anyhow::Result<(Vec<ModelEntry>, HashMap<String, OpenRouterModelInfo>)> {
    let config = resolve_provider_config(provider_name, custom_providers)?;
    let base = config
        .base_url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no base_url"))?;
    let key = AuthResolver::new(config.kind)
        .with_cli_key(cli_key)
        .with_env_override(config.api_key_env.as_deref())
        .with_config_keys(config_api_keys)
        .with_custom_provider_name(Some(provider_name))
        .resolve()
        .ok();
    let custom = custom_providers.get(provider_name);
    let http = build_http_client(
        provider_name,
        config.danger_accept_invalid_certs,
        custom,
        Some(&base),
        HttpPurpose::Metadata,
    )?;
    let url = format!("{}/models", base.trim_end_matches('/'));
    tracing::debug!("list_models_manual: GET {}", url);
    let bearer = key
        .as_deref()
        .filter(|k| !k.is_empty())
        .map(|k| k.to_string());
    let body = get_bytes_with_retry(http, url, bearer).await?;
    let models = parse_manual_models(&body)?;
    // Best-effort pricing from same payload — ignore parse errors for pricing,
    // keep models even if pricing shape mismatches.
    let pricing = parse_model_infos(&body).unwrap_or_default();
    Ok((models, pricing))
}

/// Parse a `GET {base}/models` payload from a custom / OpenAI-compatible
/// gateway into picker entries. `context_length` is picked up when the
/// gateway reports it (OpenRouter-style), `None` otherwise.
pub(crate) fn parse_manual_models(body: &[u8]) -> anyhow::Result<Vec<ModelEntry>> {
    let resp: ManualModelsResp = serde_json::from_slice(body)?;
    Ok(resp
        .data
        .into_iter()
        .map(|i| ModelEntry {
            display: i.id.clone(),
            id: i.id,
            context_length: i.context_length.map(|cl| cl as u32),
            kind: None,
            input_price: None,
            output_price: None,
        })
        .collect())
}

#[derive(Clone, Copy, Default)]
pub struct OpenRouterModelInfo {
    pub input_cost: f64,
    pub output_cost: f64,
    pub context_length: Option<u64>,
}

/// Fetch per-model pricing and context length live from
/// `GET {base_url}/models` for the built-in `openrouter` or any custom
/// provider exposing an OpenRouter-style model listing (e.g. laroute).
/// Returns an empty-map error only on transport/parse failure; providers
/// whose listing carries no pricing/`context_length` simply yield no entries.
pub async fn fetch_live_model_info(
    provider_name: &str,
    api_key: Option<&str>,
    custom_providers: &HashMap<String, CustomProviderConfig>,
    config_api_keys: Option<&HashMap<String, String>>,
) -> anyhow::Result<HashMap<String, OpenRouterModelInfo>> {
    let config = resolve_provider_config(provider_name, custom_providers)?;
    let key = AuthResolver::new(config.kind)
        .with_cli_key(api_key)
        .with_env_override(config.api_key_env.as_deref())
        .with_config_keys(config_api_keys)
        .with_custom_provider_name(Some(provider_name))
        .resolve()
        .ok();
    let custom = custom_providers.get(provider_name);
    let base = config
        .base_url
        .clone()
        .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
    let http = build_http_client(
        provider_name,
        config.danger_accept_invalid_certs,
        custom,
        Some(&base),
        HttpPurpose::Metadata,
    )?;
    let url = format!("{}/models", base.trim_end_matches('/'));
    let bearer = key
        .as_deref()
        .filter(|k| !k.is_empty())
        .map(|k| k.to_string());
    let body = get_bytes_with_retry(http, url, bearer).await?;
    parse_model_infos(&body)
}

/// A pricing value: OpenRouter serializes prices as strings, some compatible
/// gateways (e.g. laroute) as plain numbers — accept both.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StringOrNumber {
    S(String),
    N(f64),
}

impl StringOrNumber {
    fn as_f64(&self) -> f64 {
        match self {
            Self::S(s) => s.parse().unwrap_or(0.0),
            Self::N(n) => *n,
        }
    }
}

/// Parse an OpenRouter-style `GET /models` payload into per-model pricing
/// (USD per token in the payload, returned per million tokens) and context
/// length. Entries with no pricing and no `context_length` are skipped.
pub(crate) fn parse_model_infos(
    body: &[u8],
) -> anyhow::Result<HashMap<String, OpenRouterModelInfo>> {
    #[derive(serde::Deserialize)]
    struct PricingResp {
        prompt: StringOrNumber,
        completion: StringOrNumber,
    }
    #[derive(serde::Deserialize)]
    struct PricingEntry {
        id: String,
        pricing: Option<PricingResp>,
        context_length: Option<u64>,
    }
    #[derive(serde::Deserialize)]
    struct PricingList {
        data: Vec<PricingEntry>,
    }
    let resp: PricingList = serde_json::from_slice(body)?;
    let mut map = HashMap::new();
    for entry in resp.data {
        let (input, output) = match entry.pricing.as_ref() {
            Some(p) => (p.prompt.as_f64(), p.completion.as_f64()),
            None => (0.0, 0.0),
        };
        if input > 0.0 || output > 0.0 || entry.context_length.is_some() {
            map.insert(
                entry.id,
                OpenRouterModelInfo {
                    input_cost: input * 1_000_000.0,
                    output_cost: output * 1_000_000.0,
                    context_length: entry.context_length,
                },
            );
        }
    }
    Ok(map)
}

async fn summarize_with_model(model: AnyModel, prompt: String) -> anyhow::Result<String> {
    match model {
        AnyModel::OpenRouter(m) => run_summarizer(m.model, prompt).await,
        AnyModel::OpenAI(m) => run_summarizer(m.as_dyn(), prompt).await,
        AnyModel::Anthropic(m) => run_summarizer(m, prompt).await,
        AnyModel::Gemini(m) => run_summarizer(m, prompt).await,
        AnyModel::Ollama(m) => run_summarizer(m, prompt).await,
    }
}

async fn run_summarizer(model: DynModel<Completion>, prompt: String) -> anyhow::Result<String> {
    let mut preamble = "You are a conversation summarizer.".to_string();
    if let Some(s) = crate::session::storage::load_suffix() {
        preamble.push_str("\n\n---\n\n");
        preamble.push_str(&s);
    }

    let agent = rig::agent::AgentBuilder::new(model)
        .preamble(&preamble)
        .build();

    let agent_ref = &agent;
    let mut stream = retry::retry_stream_chat(&RetryConfig::default(), move || {
        let p = prompt.clone();
        async move {
            agent_ref
                .prompt(p)
                .history(Vec::<Message>::new())
                .max_turns(1)
                .stream()
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("Compression failed: {}", e))?;

    let mut response = String::new();
    use futures::StreamExt;
    use rig::streaming::{Item, StreamEvent};
    while let Some(item) = stream.next().await {
        match item {
            Ok(rig::agent::MultiTurnStreamItem::StreamAssistantItem(Item::Event(
                StreamEvent::Text { text, .. },
            ))) => response.push_str(&text),
            Ok(rig::agent::MultiTurnStreamItem::FinalResponse(res)) => {
                response = res.output.to_string();
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

pub(crate) fn serialize_conversation(messages: &[SessionMessage]) -> String {
    let mut result = String::new();
    for msg in messages {
        let role_tag = match msg.role {
            crate::session::MessageRole::User => "User",
            crate::session::MessageRole::Assistant => "Assistant",
            crate::session::MessageRole::System => "System",
            crate::session::MessageRole::ToolCall => "ToolCall",
            crate::session::MessageRole::ToolResult => "ToolResult",
            crate::session::MessageRole::SubagentToolCall => "SubagentToolCall",
        };
        result.push_str(&format!("[{}]: {}\n\n", role_tag, msg.content));
    }
    result
}

pub enum AnyModel {
    /// The model plus provider-specific extra body params. For
    /// `anthropic/*` models routed via OpenRouter `extra` pins
    /// `provider.order` to the Anthropic direct route, the only route that
    /// honors `cache_control` breakpoints (Bedrock/Vertex silently drop
    /// them). `None` for every other OpenRouter model, which caches
    /// automatically and needs no routing.
    OpenRouter(OpenRouterModel),
    OpenAI(OpenAiModel),
    Anthropic(DynModel<Completion>),
    Gemini(DynModel<Completion>),
    Ollama(DynModel<Completion>),
}

impl AnyModel {
    /// The erased completion model, whatever the provider.
    pub fn as_dyn(&self) -> DynModel<Completion> {
        match self {
            AnyModel::OpenRouter(m) => m.model.clone(),
            AnyModel::OpenAI(m) => m.as_dyn(),
            AnyModel::Anthropic(m) => m.clone(),
            AnyModel::Gemini(m) => m.clone(),
            AnyModel::Ollama(m) => m.clone(),
        }
    }
}

#[derive(Clone)]
pub enum AnyAgent {
    OpenRouter(Agent),
    OpenAI(OpenAiAgent),
    Anthropic(Agent),
    Gemini(Agent),
    Ollama(Agent),
    /// Scripted test double (`rig::test_utils::MockCompletionModel`), used by
    /// headless main-loop integration tests; never constructed in production.
    #[cfg(test)]
    Mock(Agent),
}

/// Synthesizes an `AgentRunner` for a prompt blocked by a `UserPromptSubmit`
/// hook: no model call happens, the block feedback surfaces through the same
/// `AgentEvent::Error` path a real run-time error would use.
#[cfg(feature = "hooks")]
fn spawn_blocked_runner(feedback: String) -> AgentRunner {
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(1);
    let join = tokio::spawn(async move {
        let _ = event_tx
            .send(AgentEvent::Error(CompactString::from(feedback)))
            .await;
    });
    AgentRunner {
        event_rx,
        abort_handle: join.abort_handle(),
    }
}

impl AnyAgent {
    pub async fn run_print(
        &self,
        prompt: &str,
        pure_stdout: bool,
        retry_config: &RetryConfig,
        // Prior turns from a resumed session; see `runner::run_print`. Empty
        // for a fresh session.
        history: Vec<Message>,
        // `--loop` iteration/active state; see `runner::run_print`. `None`
        // for plain `-p` one-shot runs.
        #[cfg(feature = "hooks")] loop_info: Option<LoopInfo>,
    ) -> anyhow::Result<runner::PrintOutcome> {
        match self {
            AnyAgent::OpenRouter(a) => {
                runner::run_print(
                    a,
                    prompt,
                    pure_stdout,
                    retry_config,
                    history,
                    #[cfg(feature = "hooks")]
                    loop_info,
                )
                .await
            }
            AnyAgent::OpenAI(a) => match a {
                OpenAiAgent::Responses(a) => {
                    runner::run_print(
                        a,
                        prompt,
                        pure_stdout,
                        retry_config,
                        history,
                        #[cfg(feature = "hooks")]
                        loop_info,
                    )
                    .await
                }
                OpenAiAgent::Completions(a) => {
                    runner::run_print(
                        a,
                        prompt,
                        pure_stdout,
                        retry_config,
                        history,
                        #[cfg(feature = "hooks")]
                        loop_info,
                    )
                    .await
                }
            },
            AnyAgent::Anthropic(a) => {
                runner::run_print(
                    a,
                    prompt,
                    pure_stdout,
                    retry_config,
                    history,
                    #[cfg(feature = "hooks")]
                    loop_info,
                )
                .await
            }
            AnyAgent::Gemini(a) => {
                runner::run_print(
                    a,
                    prompt,
                    pure_stdout,
                    retry_config,
                    history,
                    #[cfg(feature = "hooks")]
                    loop_info,
                )
                .await
            }
            AnyAgent::Ollama(a) => {
                runner::run_print(
                    a,
                    prompt,
                    pure_stdout,
                    retry_config,
                    history,
                    #[cfg(feature = "hooks")]
                    loop_info,
                )
                .await
            }
            #[cfg(test)]
            AnyAgent::Mock(a) => {
                runner::run_print(
                    a,
                    prompt,
                    pure_stdout,
                    retry_config,
                    history,
                    #[cfg(feature = "hooks")]
                    loop_info,
                )
                .await
            }
        }
    }

    #[cfg(feature = "subagents")]
    pub async fn run_subagent(
        &self,
        prompt: &str,
        max_turns: usize,
        event_tx: Option<&mpsc::Sender<AgentEvent>>,
        retry_config: &RetryConfig,
    ) -> anyhow::Result<String> {
        match self {
            AnyAgent::OpenRouter(a) => {
                runner::run_subagent(a, prompt, max_turns, event_tx, retry_config).await
            }
            AnyAgent::OpenAI(a) => match a {
                OpenAiAgent::Responses(a) => {
                    runner::run_subagent(a, prompt, max_turns, event_tx, retry_config).await
                }
                OpenAiAgent::Completions(a) => {
                    runner::run_subagent(a, prompt, max_turns, event_tx, retry_config).await
                }
            },
            AnyAgent::Anthropic(a) => {
                runner::run_subagent(a, prompt, max_turns, event_tx, retry_config).await
            }
            AnyAgent::Gemini(a) => {
                runner::run_subagent(a, prompt, max_turns, event_tx, retry_config).await
            }
            AnyAgent::Ollama(a) => {
                runner::run_subagent(a, prompt, max_turns, event_tx, retry_config).await
            }
            #[cfg(test)]
            AnyAgent::Mock(a) => {
                runner::run_subagent(a, prompt, max_turns, event_tx, retry_config).await
            }
        }
    }

    /// Async because, under `hooks`, the `UserPromptSubmit` gate must resolve
    /// before spawning: its outcome decides whether the runner spawns at all
    /// (a hook can block the prompt outright) and, if so, with what prompt
    /// (a hook can rewrite it).
    pub async fn spawn_runner(
        self,
        prompt: String,
        history: Vec<Message>,
        retry_config: RetryConfig,
        // `--loop` iteration/active state; see `runner::spawn_agent`. `None`
        // outside loop mode.
        #[cfg(feature = "hooks")] loop_info: Option<LoopInfo>,
    ) -> AgentRunner {
        #[cfg(feature = "hooks")]
        let prompt = match crate::extras::hooks::dispatch_user_prompt_submit(prompt).await {
            crate::extras::hooks::PromptGate::Blocked(feedback) => {
                return spawn_blocked_runner(feedback);
            }
            crate::extras::hooks::PromptGate::Proceed(prompt) => prompt,
        };
        match self {
            AnyAgent::OpenRouter(a) => runner::spawn_agent(
                a,
                prompt,
                history,
                retry_config,
                #[cfg(feature = "hooks")]
                loop_info,
            ),
            AnyAgent::OpenAI(a) => match a {
                OpenAiAgent::Responses(a) => runner::spawn_agent(
                    a,
                    prompt,
                    history,
                    retry_config,
                    #[cfg(feature = "hooks")]
                    loop_info,
                ),
                OpenAiAgent::Completions(a) => runner::spawn_agent(
                    a,
                    prompt,
                    history,
                    retry_config,
                    #[cfg(feature = "hooks")]
                    loop_info,
                ),
            },
            AnyAgent::Anthropic(a) => runner::spawn_agent(
                a,
                prompt,
                history,
                retry_config,
                #[cfg(feature = "hooks")]
                loop_info,
            ),
            AnyAgent::Gemini(a) => runner::spawn_agent(
                a,
                prompt,
                history,
                retry_config,
                #[cfg(feature = "hooks")]
                loop_info,
            ),
            AnyAgent::Ollama(a) => runner::spawn_agent(
                a,
                prompt,
                history,
                retry_config,
                #[cfg(feature = "hooks")]
                loop_info,
            ),
            #[cfg(test)]
            AnyAgent::Mock(a) => runner::spawn_agent(
                a,
                prompt,
                history,
                retry_config,
                #[cfg(feature = "hooks")]
                loop_info,
            ),
        }
    }

    pub fn spawn_btw(
        self,
        prompt: String,
        history: Vec<Message>,
        event_tx: mpsc::Sender<crate::event::BtwEvent>,
        id: u32,
        retry_config: RetryConfig,
    ) -> crate::agent::runner::BtwRunner {
        match self {
            AnyAgent::OpenRouter(a) => {
                runner::spawn_btw(a, prompt, history, event_tx, id, retry_config)
            }
            AnyAgent::OpenAI(a) => match a {
                OpenAiAgent::Responses(a) => {
                    runner::spawn_btw(a, prompt, history, event_tx, id, retry_config)
                }
                OpenAiAgent::Completions(a) => {
                    runner::spawn_btw(a, prompt, history, event_tx, id, retry_config)
                }
            },
            AnyAgent::Anthropic(a) => {
                runner::spawn_btw(a, prompt, history, event_tx, id, retry_config)
            }
            AnyAgent::Gemini(a) => {
                runner::spawn_btw(a, prompt, history, event_tx, id, retry_config)
            }
            AnyAgent::Ollama(a) => {
                runner::spawn_btw(a, prompt, history, event_tx, id, retry_config)
            }
            #[cfg(test)]
            AnyAgent::Mock(a) => runner::spawn_btw(a, prompt, history, event_tx, id, retry_config),
        }
    }
}

/// Expands a value that is exactly "${VAR}" to the environment variable's value;
/// any other format is returned as-is. Only whole-string `${VAR}` is supported
/// (the common, safe case) rather than arbitrary interpolation.
pub(crate) fn expand_env(value: &str) -> anyhow::Result<String> {
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

/// What an HTTP client is for. The two kinds of traffic need opposite
/// deadlines, so the purpose picks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpPurpose {
    /// Completion traffic handed to rig. A streamed reply legitimately runs
    /// for minutes, so it gets no whole-request deadline; a total deadline
    /// here cuts every reply longer than it mid-stream (reqwest reports that
    /// as "error decoding response body"). Only silence is bounded: no bytes
    /// for [`COMPLETION_IDLE_TIMEOUT`] fails the request as a timeout, which
    /// the runner's retry recognises.
    Completion,
    /// Metadata requests zerostack issues itself (`GET /models` at startup
    /// and from `/provider`), capped per attempt by
    /// [`DEFAULT_METADATA_TIMEOUT`] so a stalled DNS, TLS handshake, or
    /// server cannot hold the caller.
    Metadata,
}

/// Connect-phase cap shared by every client.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Whole-request deadline for [`HttpPurpose::Metadata`] when the provider
/// sets no `timeout_secs`.
pub(crate) const DEFAULT_METADATA_TIMEOUT: Duration = Duration::from_secs(8);
/// Idle cap for [`HttpPurpose::Completion`]: a stream that sends nothing for
/// this long is dead. Long reasoning turns are silent for a while, so this is
/// generous; it matches the stream idle timeout the Codex CLI uses.
const COMPLETION_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Whole-request deadline to apply, if any: an explicit `timeout_secs` wins
/// for both purposes; otherwise metadata requests get
/// [`DEFAULT_METADATA_TIMEOUT`] and completions get none (their connect
/// phase is still capped by `connect_timeout`).
pub(crate) fn http_total_timeout(
    purpose: HttpPurpose,
    custom: Option<&CustomProviderConfig>,
) -> Option<Duration> {
    match (purpose, custom.and_then(|c| c.timeout_secs)) {
        (_, Some(secs)) => Some(Duration::from_secs(secs)),
        (HttpPurpose::Metadata, None) => Some(DEFAULT_METADATA_TIMEOUT),
        (HttpPurpose::Completion, None) => None,
    }
}

/// Builder with the defaults every provider client shares: user agent,
/// keepalive, pool sizing, and the connect cap. Reused by the MCP, OAuth, and
/// gist-upload clients so those timeouts/headers stay in one place.
pub(crate) fn base_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(format!(
            "zerostack/{} (https://github.com/gi-dellav/zerostack)",
            env!("CARGO_PKG_VERSION")
        ))
        .tcp_keepalive(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .connect_timeout(CONNECT_TIMEOUT)
}

/// Apply the settings that differ by purpose: the whole-request deadline
/// from [`http_total_timeout`], and the idle cap on completion streams.
fn apply_purpose(
    builder: reqwest::ClientBuilder,
    purpose: HttpPurpose,
    custom: Option<&CustomProviderConfig>,
) -> reqwest::ClientBuilder {
    let builder = match http_total_timeout(purpose, custom) {
        Some(deadline) => builder.timeout(deadline),
        None => builder,
    };
    match purpose {
        HttpPurpose::Completion => builder.read_timeout(COMPLETION_IDLE_TIMEOUT),
        HttpPurpose::Metadata => builder,
    }
}

/// Shared pooled clients for built-in providers with default settings, one
/// per purpose so the metadata deadline never reaches completion streams.
static BUILTIN_COMPLETION_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        apply_purpose(base_client_builder(), HttpPurpose::Completion, None)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    });

static BUILTIN_METADATA_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        apply_purpose(base_client_builder(), HttpPurpose::Metadata, None)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    });

/// Builds a reqwest client for `purpose`, combining:
/// - `danger_accept_invalid_certs` (from #62; the TLS toggle shared by all providers)
/// - a custom provider's `headers` (values support `${ENV_VAR}` expansion) and `timeout_secs`
/// - the per-purpose deadline from [`http_total_timeout`]
///
/// Built-in providers with default settings share one pooled client per
/// purpose instead of opening a new connection pool per request.
pub(crate) fn build_http_client(
    provider_name: &str,
    danger_accept_invalid_certs: bool,
    custom: Option<&CustomProviderConfig>,
    base_url: Option<&str>,
    purpose: HttpPurpose,
) -> anyhow::Result<reqwest::Client> {
    if custom.is_none() && !danger_accept_invalid_certs && !is_localhost(base_url) {
        return Ok(match purpose {
            HttpPurpose::Completion => BUILTIN_COMPLETION_CLIENT.clone(),
            HttpPurpose::Metadata => BUILTIN_METADATA_CLIENT.clone(),
        });
    }
    let mut builder = base_client_builder();
    if is_localhost(base_url) {
        // Disable connection pooling for local LLM servers (notably
        // llama.cpp's cpp-httplib) which close idle keep-alive
        // connections far faster than reqwest's default 90s
        // pool_idle_timeout, leaving stale half-closed sockets.
        builder = builder.pool_max_idle_per_host(0);
    }

    if let Some(cfg) = custom.filter(|cfg| !cfg.headers.is_empty()) {
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
    builder = apply_purpose(builder, purpose, custom);

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

fn is_localhost(url: Option<&str>) -> bool {
    url.is_some_and(|u| {
        u.starts_with("http://localhost")
            || u.starts_with("http://127.")
            || u.starts_with("http://[::1]")
    })
}

/// GET with retry, handling 429/5xx + `Retry-After` and pooling reuse.
/// Clones the request for each attempt (GET has no body, so clone is cheap).
async fn get_bytes_with_retry(
    client: reqwest::Client,
    url: String,
    bearer: Option<String>,
) -> anyhow::Result<Vec<u8>> {
    let cfg = RetryConfig {
        max_attempts: 3,
        initial_backoff_ms: 500,
        max_backoff_ms: 5_000,
    };
    let bytes = retry::with_retry(&cfg, || {
        let client = client.clone();
        let url = url.clone();
        let bearer = bearer.clone();
        async move {
            let mut req = client.get(&url);
            if let Some(k) = bearer.as_deref().filter(|k| !k.is_empty()) {
                req = req.bearer_auth(k);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                let retry_after = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let msg = if retry_after.is_empty() {
                    format!(
                        "HTTP {} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("")
                    )
                } else {
                    format!("HTTP {} retry-after: {}", status.as_u16(), retry_after)
                };
                return Err(std::io::Error::other(msg));
            }
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                // 4xx except 429 is not retryable — use NotFound kind so
                // `is_retryable` returns false.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("HTTP {}: {}", status, text.trim()),
                ));
            }
            let b = resp
                .bytes()
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            Ok(b.to_vec())
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(bytes)
}

/// Determines which API style the OpenAI family should use:
/// if `api_style` is set explicitly, honor it; otherwise default to Completions
/// when a base_url is present (i.e. a compatible gateway) and Responses when it
/// is absent (i.e. real api.openai.com).
pub(crate) fn resolve_api_style(
    base_url: Option<&str>,
    custom: Option<&CustomProviderConfig>,
) -> ApiStyle {
    custom.and_then(|c| c.api_style).unwrap_or({
        if base_url.is_some() {
            ApiStyle::Completions
        } else {
            ApiStyle::Responses
        }
    })
}

/// Wraps a configured `reqwest::Client` as rig's erased HTTP transport so a
/// provider client sends through zerostack's pooled, per-purpose client.
fn rig_http(http_client: reqwest::Client) -> rig::http_client::DynHttpClient {
    rig::http_client::DynHttpClient::new(rig::http_client::ReqwestClient::from(http_client))
}

/// Builds an OpenAI-family client (Responses or Chat Completions) using the
/// already-constructed shared http_client. The completion endpoint is chosen
/// via [`Route`] on the config; [`AnyClient::completion_model`] reads that
/// route back when it builds each model.
fn build_openai_client(
    key: &str,
    base_url: Option<&str>,
    custom: Option<&CustomProviderConfig>,
    http_client: reqwest::Client,
) -> anyhow::Result<OpenAiClient> {
    let style = resolve_api_style(base_url, custom);
    let route = match style {
        ApiStyle::Responses => Route::Responses,
        ApiStyle::Completions => Route::Chat,
    };
    let config = match base_url {
        Some(u) => OpenAIConfig::new(key).with_base_url(u),
        None => OpenAIConfig::new(key),
    };
    Ok(config.with_route(route).connect(rig_http(http_client)))
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
            let http_client = build_http_client(
                provider_name,
                config.danger_accept_invalid_certs,
                custom,
                base_url.as_deref(),
                HttpPurpose::Completion,
            )?;
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
    let config = match base_url {
        Some(u) => anthropic::wire::AnthropicConfig::new(key).with_base_url(u),
        None => anthropic::wire::AnthropicConfig::new(key),
    };
    Ok(AnyClient::Anthropic(config.client()))
}

fn build_gemini_client(key: &str, base_url: Option<&str>) -> anyhow::Result<AnyClient> {
    let config = match base_url {
        Some(u) => gemini::GeminiConfig::new(key).with_base_url(u),
        None => gemini::GeminiConfig::new(key),
    };
    Ok(AnyClient::Gemini(config.client()))
}

fn build_ollama_client(key: &str, base_url: Option<&str>) -> anyhow::Result<AnyClient> {
    let config = ollama::wire::OllamaConfig::new().with_api_key(key);
    let config = match base_url {
        Some(u) => config.with_base_url(u),
        None => config,
    };
    Ok(AnyClient::Ollama(config.client()))
}

fn build_openrouter_client(key: &str, base_url: Option<&str>) -> anyhow::Result<AnyClient> {
    // OpenRouter is an OpenAI-shaped dialect. `with_key` pins the dialect so
    // its base URL, auth header, and quirks apply; a custom `base_url`
    // overrides the dialect default.
    let config = OpenAIConfig::with_key(&OPENROUTER, key);
    let config = match base_url {
        Some(u) => config.with_base_url(u),
        None => config,
    };
    Ok(AnyClient::OpenRouter(config.client()))
}

/// Builds an OpenAiModel (Responses / Chat Completions) into the matching
/// OpenAiAgent.
async fn build_openai_agent(model: OpenAiModel, build: AgentBuild<'_>) -> OpenAiAgent {
    match model {
        OpenAiModel::Responses(m) => {
            OpenAiAgent::Responses(builder::build_agent_inner(m, build).await)
        }
        OpenAiModel::Completions(m) => {
            OpenAiAgent::Completions(builder::build_agent_inner(m, build).await)
        }
    }
}

pub async fn build_agent(model: AnyModel, build: AgentBuild<'_>) -> AnyAgent {
    match model {
        AnyModel::OpenRouter(m) => {
            let mut build = build;
            build.extra_body = merge_extra_body(m.extra, build.extra_body);
            AnyAgent::OpenRouter(builder::build_agent_inner(m.model, build).await)
        }
        AnyModel::OpenAI(m) => AnyAgent::OpenAI(build_openai_agent(m, build).await),
        AnyModel::Anthropic(m) => AnyAgent::Anthropic(builder::build_agent_inner(m, build).await),
        AnyModel::Gemini(m) => AnyAgent::Gemini(builder::build_agent_inner(m, build).await),
        AnyModel::Ollama(m) => AnyAgent::Ollama(builder::build_agent_inner(m, build).await),
    }
}

/// Builds the isolated, tool-less `/btw` agent for the active provider.
pub fn build_btw_agent(model: AnyModel, build: AgentBuild<'_>) -> AnyAgent {
    match model {
        AnyModel::OpenRouter(m) => {
            let mut build = build;
            build.extra_body = merge_extra_body(m.extra, build.extra_body);
            AnyAgent::OpenRouter(builder::build_btw_agent_inner(m.model, build))
        }
        AnyModel::OpenAI(m) => AnyAgent::OpenAI(match m {
            OpenAiModel::Responses(m) => {
                OpenAiAgent::Responses(builder::build_btw_agent_inner(m, build))
            }
            OpenAiModel::Completions(m) => {
                OpenAiAgent::Completions(builder::build_btw_agent_inner(m, build))
            }
        }),
        AnyModel::Anthropic(m) => AnyAgent::Anthropic(builder::build_btw_agent_inner(m, build)),
        AnyModel::Gemini(m) => AnyAgent::Gemini(builder::build_btw_agent_inner(m, build)),
        AnyModel::Ollama(m) => AnyAgent::Ollama(builder::build_btw_agent_inner(m, build)),
    }
}
