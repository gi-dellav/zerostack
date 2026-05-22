#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{SecondsFormat, Utc};
use reqwest::header::{self, HeaderMap, HeaderValue};
use rig::providers::copilot as rig_copilot;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

const CREDENTIAL_KEY: &str = "github-copilot";
const PROVIDER: &str = "github-copilot";
pub(crate) const DEFAULT_BASE_URL: &str = "https://api.individual.githubcopilot.com";
const TOKEN_EXPIRY_SKEW_MS: i64 = 5 * 60 * 1000;

pub(crate) fn base_url_from_token(token: &str) -> Option<String> {
    let proxy_ep = token
        .split(';')
        .find_map(|part| part.trim().strip_prefix("proxy-ep="))?
        .trim();

    if proxy_ep.is_empty() {
        return None;
    }

    let api_ep = proxy_ep.replacen("proxy.", "api.", 1);
    if api_ep.starts_with("http://") || api_ep.starts_with("https://") {
        Some(api_ep)
    } else {
        Some(format!("https://{api_ep}"))
    }
}

pub(crate) fn copilot_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "User-Agent",
        HeaderValue::from_static("GitHubCopilotChat/0.35.0"),
    );
    headers.insert("Editor-Version", HeaderValue::from_static("vscode/1.107.0"));
    headers.insert(
        "Editor-Plugin-Version",
        HeaderValue::from_static("copilot-chat/0.35.0"),
    );
    headers.insert(
        "Copilot-Integration-Id",
        HeaderValue::from_static("vscode-chat"),
    );
    headers.insert("X-Initiator", HeaderValue::from_static("user"));
    headers.insert(
        "Openai-Intent",
        HeaderValue::from_static("conversation-edits"),
    );
    headers
}

pub fn subscription_auth_path(cli_path: Option<&Path>, config_path: Option<&str>) -> PathBuf {
    if let Some(path) = cli_path {
        return path.to_path_buf();
    }
    if let Some(path) = config_path.filter(|path| !path.trim().is_empty()) {
        return PathBuf::from(path);
    }

    crate::session::storage::config_path().join("copilot-auth.json")
}

pub async fn subscription_access_token(auth_path: &Path) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let mut credentials_file = read_or_new_credentials_file(auth_path)?;

    if let Ok(credentials) = read_copilot_credentials(&credentials_file) {
        match ensure_access_token(&client, auth_path, &mut credentials_file, credentials).await {
            Ok(token) => return Ok(token),
            Err(err) => {
                tracing::warn!(
                    "failed to refresh cached GitHub Copilot subscription token; starting device login: {err}"
                );
            }
        }
    }

    login_with_rig_and_import_credentials(auth_path).await
}

pub fn subscription_access_token_blocking(auth_path: &Path) -> anyhow::Result<String> {
    let auth_path = auth_path.to_path_buf();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(subscription_access_token(&auth_path))
    })
    .join()
    .map_err(|_| anyhow::anyhow!("GitHub Copilot subscription login task panicked"))?
}

#[derive(Debug, Clone, Deserialize)]
pub struct CopilotModelSyncConfig {
    pub credentials_path: PathBuf,
    pub output_path: PathBuf,
    #[serde(default)]
    pub enable_policy: bool,
}

impl CopilotModelSyncConfig {
    pub fn load(
        config_path: Option<&Path>,
        credentials_path: Option<&Path>,
        output_path: Option<&Path>,
        enable_policy: bool,
    ) -> anyhow::Result<Self> {
        let mut builder = config_rs::Config::builder()
            .set_default("credentials_path", "copilot-credentials.json")?
            .set_default("output_path", "copilot-models.json")?
            .set_default("enable_policy", false)?;

        builder = if let Some(path) = config_path {
            builder.add_source(config_rs::File::with_name(&path.to_string_lossy()).required(true))
        } else {
            builder.add_source(config_rs::File::with_name("copilot-model-sync").required(false))
        };

        builder = builder.add_source(
            config_rs::Environment::with_prefix("ZEROSTACK_COPILOT_MODELS")
                .separator("__")
                .try_parsing(true)
                .ignore_empty(true),
        );

        if let Some(path) = credentials_path {
            builder =
                builder.set_override("credentials_path", path.to_string_lossy().to_string())?;
        }
        if let Some(path) = output_path {
            builder = builder.set_override("output_path", path.to_string_lossy().to_string())?;
        }
        if enable_policy {
            builder = builder.set_override("enable_policy", true)?;
        }

        Ok(builder.build()?.try_deserialize()?)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CopilotModelsFile {
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    pub provider: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub models: Vec<NormalizedCopilotModel>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NormalizedCopilotModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub api: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub reasoning: bool,
    pub input: Vec<String>,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
    #[serde(rename = "maxTokens")]
    pub max_tokens: u64,
    pub headers: BTreeMap<String, String>,
    pub cost: ModelCost,
    pub compat: BTreeMap<String, Value>,
    #[serde(rename = "thinkingLevelMap", skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModelCost {
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: u64,
}

#[derive(Debug, Clone)]
struct CopilotCredentials {
    refresh: String,
    access: Option<String>,
    expires: Option<i64>,
    enterprise_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: i64,
}

pub async fn sync_copilot_models(
    config: CopilotModelSyncConfig,
) -> anyhow::Result<CopilotModelsFile> {
    let client = reqwest::Client::new();
    let mut credentials_file = read_credentials_file(&config.credentials_path)?;
    let credentials = read_copilot_credentials(&credentials_file)?;
    let enterprise_domain = credentials
        .enterprise_url
        .as_deref()
        .map(normalize_enterprise_domain);
    let access_token = ensure_access_token(
        &client,
        &config.credentials_path,
        &mut credentials_file,
        credentials,
    )
    .await?;
    let base_url = copilot_base_url(&access_token, enterprise_domain.as_deref());
    let models =
        fetch_copilot_models(&client, &base_url, &access_token, config.enable_policy).await?;
    let output = copilot_models_file(base_url, models);
    write_json_file(&config.output_path, &output)?;
    Ok(output)
}

pub async fn fetch_copilot_models(
    client: &reqwest::Client,
    base_url: &str,
    access_token: &str,
    enable_policy: bool,
) -> anyhow::Result<Vec<NormalizedCopilotModel>> {
    let raw_models = fetch_copilot_model_rows(client, base_url, access_token).await?;
    let mut models = Vec::new();

    for mut row in raw_models {
        if policy_disabled(&row) && enable_policy {
            if enable_model_policy(
                client,
                base_url,
                access_token,
                model_id(&row).unwrap_or_default(),
            )
            .await
            {
                set_policy_state(&mut row, "enabled");
            }
        }
        if let Some(model) = normalize_model(&row, base_url) {
            models.push(model);
        }
    }

    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

pub fn copilot_models_file(
    base_url: String,
    models: Vec<NormalizedCopilotModel>,
) -> CopilotModelsFile {
    CopilotModelsFile {
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        provider: PROVIDER.to_string(),
        base_url,
        models,
    }
}

pub fn preferred_gpt_model_id(models: &[NormalizedCopilotModel]) -> Option<&str> {
    models
        .iter()
        .filter(|model| model.id.starts_with("gpt-"))
        .max_by(|a, b| {
            gpt_version_key(&a.id)
                .cmp(&gpt_version_key(&b.id))
                .then_with(|| a.id.cmp(&b.id))
        })
        .map(|model| model.id.as_str())
}

pub fn preferred_gpt_model_id_from_ids<'a>(
    ids: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    ids.into_iter()
        .filter(|id| id.starts_with("gpt-"))
        .max_by(|a, b| {
            gpt_version_key(a)
                .cmp(&gpt_version_key(b))
                .then_with(|| a.cmp(b))
        })
        .map(ToString::to_string)
}

pub(crate) fn api_for_model_id(id: &str) -> &'static str {
    classify_api(id)
}

pub async fn enable_model_policy(
    client: &reqwest::Client,
    base_url: &str,
    access_token: &str,
    model_id: &str,
) -> bool {
    if model_id.is_empty() {
        return false;
    }

    let url = format!(
        "{}/models/{}/policy",
        base_url.trim_end_matches('/'),
        encode_path_segment(model_id)
    );
    let mut request = client
        .post(url)
        .bearer_auth(access_token)
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/json")
        .header("openai-intent", "chat-policy")
        .header("x-interaction-type", "chat-policy")
        .json(&json!({ "state": "enabled" }));
    request = apply_copilot_headers(request);

    match request.send().await {
        Ok(response) if response.status().is_success() => true,
        Ok(response) => {
            tracing::warn!(
                "failed to enable Copilot model policy for {}: HTTP {}",
                model_id,
                response.status()
            );
            false
        }
        Err(err) => {
            tracing::warn!(
                "failed to enable Copilot model policy for {}: {}",
                model_id,
                err
            );
            false
        }
    }
}

fn read_credentials_file(path: &Path) -> anyhow::Result<Value> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Copilot credentials file {}", path.display()))?;
    Ok(serde_json::from_str(&text).with_context(|| {
        format!(
            "failed to parse Copilot credentials file {}",
            path.display()
        )
    })?)
}

fn read_or_new_credentials_file(path: &Path) -> anyhow::Result<Value> {
    match read_credentials_file(path) {
        Ok(value) => Ok(value),
        Err(err) if path.exists() => Err(err),
        Err(_) => Ok(Value::Object(Map::new())),
    }
}

fn read_copilot_credentials(root: &Value) -> anyhow::Result<CopilotCredentials> {
    let entry = root
        .get(CREDENTIAL_KEY)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            anyhow::anyhow!("credentials file must contain a '{CREDENTIAL_KEY}' object")
        })?;

    let refresh = entry
        .get("refresh")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("'{CREDENTIAL_KEY}.refresh' is required"))?
        .to_string();

    Ok(CopilotCredentials {
        refresh,
        access: entry
            .get("access")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        expires: entry.get("expires").and_then(value_i64),
        enterprise_url: entry
            .get("enterpriseUrl")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
    })
}

async fn login_with_rig_and_import_credentials(auth_path: &Path) -> anyhow::Result<String> {
    let token_dir = rig_token_dir(auth_path);
    let rig_client = rig_copilot::Client::builder()
        .oauth()
        .token_dir(&token_dir)
        .on_device_code(|prompt| {
            eprintln!(
                "Sign in with GitHub Copilot:\n1) Visit {}\n2) Enter code: {}",
                prompt.verification_uri, prompt.user_code
            );
        })
        .build()?;

    rig_client.authorize().await?;
    import_rig_credentials(auth_path, &token_dir)
}

fn rig_token_dir(auth_path: &Path) -> PathBuf {
    auth_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("rig-copilot")
}

fn import_rig_credentials(auth_path: &Path, token_dir: &Path) -> anyhow::Result<String> {
    let refresh = std::fs::read_to_string(token_dir.join("access-token"))
        .with_context(|| {
            format!(
                "failed to read {}",
                token_dir.join("access-token").display()
            )
        })?
        .trim()
        .to_string();
    anyhow::ensure!(
        !refresh.is_empty(),
        "rig Copilot login did not write a GitHub OAuth token"
    );

    let api_key_file = token_dir.join("api-key.json");
    let api_key_json: Value = serde_json::from_slice(
        &std::fs::read(&api_key_file)
            .with_context(|| format!("failed to read {}", api_key_file.display()))?,
    )
    .with_context(|| format!("failed to parse {}", api_key_file.display()))?;
    let access = api_key_json
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| anyhow::anyhow!("rig Copilot API key file did not contain token"))?
        .to_string();
    let expires = api_key_json
        .get("expires_at")
        .and_then(value_i64)
        .map(|expires| expires.saturating_mul(1000) - TOKEN_EXPIRY_SKEW_MS)
        .unwrap_or_else(|| Utc::now().timestamp_millis() + TOKEN_EXPIRY_SKEW_MS);

    let mut credentials_file = read_or_new_credentials_file(auth_path)?;
    let mut entry = credentials_file
        .get(CREDENTIAL_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    entry.insert("type".to_string(), Value::String("oauth".to_string()));
    entry.insert("refresh".to_string(), Value::String(refresh));
    entry.insert("access".to_string(), Value::String(access.clone()));
    entry.insert(
        "expires".to_string(),
        Value::Number(serde_json::Number::from(expires)),
    );

    let root = credentials_file
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Copilot credentials root must be a JSON object"))?;
    root.insert(CREDENTIAL_KEY.to_string(), Value::Object(entry));
    write_json_file(auth_path, &credentials_file)?;

    Ok(access)
}

async fn ensure_access_token(
    client: &reqwest::Client,
    credentials_path: &Path,
    credentials_file: &mut Value,
    credentials: CopilotCredentials,
) -> anyhow::Result<String> {
    if let Some(access) = credentials.access.as_deref()
        && credentials
            .expires
            .is_some_and(|expires| expires > Utc::now().timestamp_millis())
    {
        return Ok(access.to_string());
    }

    let enterprise_domain = credentials
        .enterprise_url
        .as_deref()
        .map(normalize_enterprise_domain);
    let token =
        refresh_copilot_token(client, &credentials.refresh, enterprise_domain.as_deref()).await?;
    let expires = token.expires_at.saturating_mul(1000) - TOKEN_EXPIRY_SKEW_MS;

    let entry = credentials_file
        .get_mut(CREDENTIAL_KEY)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            anyhow::anyhow!("credentials file must contain a '{CREDENTIAL_KEY}' object")
        })?;
    entry.insert("access".to_string(), Value::String(token.token.clone()));
    entry.insert(
        "expires".to_string(),
        Value::Number(serde_json::Number::from(expires)),
    );
    write_json_file(credentials_path, credentials_file)?;

    Ok(token.token)
}

async fn refresh_copilot_token(
    client: &reqwest::Client,
    refresh_token: &str,
    enterprise_domain: Option<&str>,
) -> anyhow::Result<CopilotTokenResponse> {
    let url = match enterprise_domain {
        Some(domain) => format!("https://api.{domain}/copilot_internal/v2/token"),
        None => "https://api.github.com/copilot_internal/v2/token".to_string(),
    };
    let mut request = client
        .get(url)
        .header(header::ACCEPT, "application/json")
        .bearer_auth(refresh_token);
    request = apply_copilot_headers(request);

    Ok(request
        .send()
        .await?
        .error_for_status()?
        .json::<CopilotTokenResponse>()
        .await?)
}

fn copilot_base_url(access_token: &str, enterprise_domain: Option<&str>) -> String {
    if let Some(base_url) = base_url_from_token(access_token) {
        return base_url;
    }

    match enterprise_domain {
        Some(domain) => format!("https://copilot-api.{domain}"),
        None => DEFAULT_BASE_URL.to_string(),
    }
}

async fn fetch_copilot_model_rows(
    client: &reqwest::Client,
    base_url: &str,
    access_token: &str,
) -> anyhow::Result<Vec<Value>> {
    let mut request = client
        .get(format!("{}/models", base_url.trim_end_matches('/')))
        .header(header::ACCEPT, "application/json")
        .bearer_auth(access_token);
    request = apply_copilot_headers(request);
    let raw = request
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    model_rows(&raw)
}

fn model_rows(raw: &Value) -> anyhow::Result<Vec<Value>> {
    let rows = if let Some(rows) = raw.as_array() {
        rows
    } else if let Some(rows) = raw.get("data").and_then(Value::as_array) {
        rows
    } else if let Some(rows) = raw.get("models").and_then(Value::as_array) {
        rows
    } else {
        anyhow::bail!("Copilot models response must be an array or contain data/models array");
    };

    Ok(rows.clone())
}

fn normalize_model(row: &Value, base_url: &str) -> Option<NormalizedCopilotModel> {
    if !supports_tool_calls(row) || excluded(row) {
        return None;
    }

    let id = model_id(row)?;
    let api = classify_api(id);
    let context_window = first_u64(
        row,
        &[
            &["capabilities", "limits", "max_context_window_tokens"],
            &["capabilities", "limits", "max_prompt_tokens"],
            &["limit", "context"],
            &["limits", "max_context_window_tokens"],
            &["limits", "max_prompt_tokens"],
            &["max_context_window_tokens"],
            &["max_prompt_tokens"],
            &["contextWindow"],
            &["context_window"],
        ],
    )
    .unwrap_or(128_000);
    let max_tokens = first_u64(
        row,
        &[
            &["capabilities", "limits", "max_output_tokens"],
            &["capabilities", "limits", "max_non_streaming_output_tokens"],
            &["limit", "output"],
            &["limits", "max_output_tokens"],
            &["limits", "max_non_streaming_output_tokens"],
            &["max_output_tokens"],
            &["max_non_streaming_output_tokens"],
            &["maxOutputTokens"],
            &["maxTokens"],
            &["max_tokens"],
        ],
    )
    .unwrap_or(8192);
    let compat = compat_metadata(id, api);
    let thinking_level_map = thinking_level_map(id, api);

    Some(NormalizedCopilotModel {
        id: id.to_string(),
        name: model_name(row, id).to_string(),
        provider: PROVIDER.to_string(),
        api: api.to_string(),
        base_url: base_url.to_string(),
        reasoning: reasoning_supported(row, id),
        input: input_modalities(row),
        context_window,
        max_tokens,
        headers: normalized_headers(),
        cost: ModelCost {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        },
        compat,
        thinking_level_map,
    })
}

fn model_id(row: &Value) -> Option<&str> {
    row.get("id")
        .and_then(Value::as_str)
        .or_else(|| row.get("model").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
}

fn model_name<'a>(row: &'a Value, id: &'a str) -> &'a str {
    row.get("name")
        .and_then(Value::as_str)
        .or_else(|| row.get("display_name").and_then(Value::as_str))
        .or_else(|| row.get("displayName").and_then(Value::as_str))
        .or_else(|| row.get("model_picker_label").and_then(Value::as_str))
        .or_else(|| row.get("modelPickerLabel").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .unwrap_or(id)
}

fn supports_tool_calls(row: &Value) -> bool {
    bool_at(row, &["tool_call"])
        || bool_at(row, &["capabilities", "supports", "tool_calls"])
        || bool_at(row, &["capabilities", "supports", "toolCalls"])
}

fn excluded(row: &Value) -> bool {
    row.get("status").and_then(Value::as_str) == Some("deprecated")
        || is_false_at(row, &["model_picker_enabled"])
        || is_false_at(row, &["modelPickerEnabled"])
        || policy_disabled(row)
}

fn policy_disabled(row: &Value) -> bool {
    row.get("policy")
        .and_then(|policy| policy.get("state"))
        .and_then(Value::as_str)
        == Some("disabled")
}

fn set_policy_state(row: &mut Value, state: &str) {
    if !row.get("policy").is_some_and(Value::is_object) {
        row.as_object_mut()
            .map(|object| object.insert("policy".to_string(), Value::Object(Map::new())));
    }
    if let Some(policy) = row.get_mut("policy").and_then(Value::as_object_mut) {
        policy.insert("state".to_string(), Value::String(state.to_string()));
    }
}

fn classify_api(id: &str) -> &'static str {
    if is_anthropic_4(id) {
        "anthropic-messages"
    } else if id.starts_with("gpt-5") || id.starts_with("oswe") {
        "openai-responses"
    } else {
        "openai-completions"
    }
}

fn reasoning_supported(row: &Value, id: &str) -> bool {
    bool_at(row, &["reasoning"])
        || bool_at(row, &["capabilities", "supports", "adaptive_thinking"])
        || bool_at(row, &["capabilities", "supports", "reasoning"])
        || non_empty_array_at(row, &["capabilities", "supports", "reasoning_effort"])
        || id.starts_with("gpt-5")
        || is_anthropic_4(id)
        || id.starts_with("gemini-3")
        || id.starts_with("grok-code-fast")
}

fn input_modalities(row: &Value) -> Vec<String> {
    let mut input = vec!["text".to_string()];
    if bool_at(row, &["capabilities", "supports", "vision"])
        || value_at(row, &["capabilities", "limits", "vision"]).is_some()
        || bool_at(row, &["vision"])
        || bool_at(row, &["supports_vision"])
        || bool_at(row, &["supportsVision"])
        || value_at(row, &["modalities", "input"])
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("image")))
    {
        input.push("image".to_string());
    }
    input
}

fn compat_metadata(id: &str, api: &str) -> BTreeMap<String, Value> {
    let mut compat = BTreeMap::new();
    if api == "openai-completions" {
        compat.insert("supportsStore".to_string(), Value::Bool(false));
        compat.insert("supportsDeveloperRole".to_string(), Value::Bool(false));
        compat.insert("supportsReasoningEffort".to_string(), Value::Bool(false));
    }
    if api == "anthropic-messages" {
        if matches!(
            id,
            "claude-haiku-4.5" | "claude-sonnet-4" | "claude-sonnet-4.5"
        ) {
            compat.insert(
                "supportsEagerToolInputStreaming".to_string(),
                Value::Bool(false),
            );
        }
        if contains_any(
            id,
            &[
                "opus-4-6",
                "opus-4.6",
                "opus-4-7",
                "opus-4.7",
                "opus-4-8",
                "opus-4.8",
                "sonnet-4-6",
                "sonnet-4.6",
            ],
        ) {
            compat.insert("forceAdaptiveThinking".to_string(), Value::Bool(true));
        }
        if contains_any(id, &["opus-4-7", "opus-4.7", "opus-4-8", "opus-4.8"]) {
            compat.insert("supportsTemperature".to_string(), Value::Bool(false));
        }
    }
    compat
}

fn thinking_level_map(id: &str, api: &str) -> Option<BTreeMap<String, Value>> {
    let mut map = BTreeMap::new();
    if api == "openai-responses" && id.starts_with("gpt-5") {
        map.insert("off".to_string(), Value::Null);
        map.insert("minimal".to_string(), Value::String("low".to_string()));
        if contains_any(id, &["gpt-5.2", "gpt-5.3", "gpt-5.4", "gpt-5.5"]) {
            map.insert("xhigh".to_string(), Value::String("xhigh".to_string()));
        }
    }
    if api == "anthropic-messages" {
        if contains_any(id, &["opus-4-6", "opus-4.6"]) {
            map.insert("xhigh".to_string(), Value::String("max".to_string()));
        }
        if contains_any(id, &["opus-4-7", "opus-4.7", "opus-4-8", "opus-4.8"]) {
            map.insert("xhigh".to_string(), Value::String("xhigh".to_string()));
        }
    }

    (!map.is_empty()).then_some(map)
}

fn is_anthropic_4(id: &str) -> bool {
    ["claude-haiku-4", "claude-sonnet-4", "claude-opus-4"]
        .iter()
        .any(|prefix| {
            id == *prefix
                || id
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('-'))
        })
}

fn first_u64(row: &Value, paths: &[&[&str]]) -> Option<u64> {
    paths
        .iter()
        .find_map(|path| value_at(row, path).and_then(value_u64))
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn bool_at(value: &Value, path: &[&str]) -> bool {
    value_at(value, path).and_then(Value::as_bool) == Some(true)
}

fn is_false_at(value: &Value, path: &[&str]) -> bool {
    value_at(value, path).and_then(Value::as_bool) == Some(false)
}

fn non_empty_array_at(value: &Value, path: &[&str]) -> bool {
    value_at(value, path)
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
}

fn value_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn value_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn gpt_version_key(id: &str) -> Vec<u32> {
    let Some(rest) = id.strip_prefix("gpt-") else {
        return Vec::new();
    };
    let version = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect::<String>();
    version
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn normalized_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "User-Agent".to_string(),
            "GitHubCopilotChat/0.35.0".to_string(),
        ),
        ("Editor-Version".to_string(), "vscode/1.107.0".to_string()),
        (
            "Editor-Plugin-Version".to_string(),
            "copilot-chat/0.35.0".to_string(),
        ),
        (
            "Copilot-Integration-Id".to_string(),
            "vscode-chat".to_string(),
        ),
    ])
}

fn apply_copilot_headers(mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    for (name, value) in normalized_headers() {
        request = request.header(name, value);
    }
    request
}

fn normalize_enterprise_domain(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_matches('/')
        .to_string()
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec_pretty(value)?;
    let tmp = path.with_extension(format!(
        "{}tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!("{ext}."))
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, bytes)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE_MARKER: &str = "zerostack-copilot-api-ok";

    fn live_copilot_token() -> anyhow::Result<String> {
        std::env::var("COPILOT_API_KEY")
            .map(|token| token.trim().to_string())
            .context("set COPILOT_API_KEY to run live Copilot API tests")
            .and_then(|token| {
                anyhow::ensure!(!token.is_empty(), "COPILOT_API_KEY must not be empty");
                Ok(token)
            })
    }

    fn live_copilot_model(models: &[NormalizedCopilotModel]) -> anyhow::Result<String> {
        if let Ok(model) = std::env::var("COPILOT_TEST_MODEL")
            && !model.trim().is_empty()
        {
            return Ok(model.trim().to_string());
        }

        preferred_gpt_model_id(models)
            .or_else(|| models.first().map(|model| model.id.as_str()))
            .map(ToString::to_string)
            .ok_or_else(|| anyhow::anyhow!("Copilot /models returned no usable models"))
    }

    async fn request_copilot_response(
        client: &reqwest::Client,
        base_url: &str,
        token: &str,
        model_id: &str,
        prompt: &str,
    ) -> anyhow::Result<String> {
        let url = match classify_api(model_id) {
            "openai-responses" => format!("{}/responses", base_url.trim_end_matches('/')),
            _ => format!("{}/chat/completions", base_url.trim_end_matches('/')),
        };
        let body = match classify_api(model_id) {
            "openai-responses" => json!({
                "model": model_id,
                "input": [{
                    "role": "user",
                    "content": [{ "type": "input_text", "text": prompt }]
                }],
                "max_output_tokens": 64
            }),
            _ => json!({
                "model": model_id,
                "messages": [{ "role": "user", "content": prompt }],
                "max_tokens": 64,
                "stream": false
            }),
        };

        Ok(client
            .post(url)
            .bearer_auth(token)
            .headers(copilot_headers())
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }

    #[tokio::test]
    #[ignore = "live Copilot API test; set COPILOT_API_KEY and run with --ignored"]
    async fn live_copilot_models_api_returns_available_models() -> anyhow::Result<()> {
        let token = live_copilot_token()?;
        let base_url = copilot_base_url(&token, None);
        let models =
            fetch_copilot_models(&reqwest::Client::new(), &base_url, &token, false).await?;

        assert!(!models.is_empty(), "Copilot /models returned no models");
        assert!(
            models.iter().all(|model| model.provider == PROVIDER),
            "expected all normalized models to use provider {PROVIDER}"
        );

        Ok(())
    }

    #[tokio::test]
    #[ignore = "live Copilot API test; set COPILOT_API_KEY and run with --ignored"]
    async fn live_copilot_api_returns_response_for_available_model() -> anyhow::Result<()> {
        let token = live_copilot_token()?;
        let base_url = copilot_base_url(&token, None);
        let client = reqwest::Client::new();
        let models = fetch_copilot_models(&client, &base_url, &token, false).await?;
        let model_id = live_copilot_model(&models)?;
        let response = request_copilot_response(
            &client,
            &base_url,
            &token,
            &model_id,
            &format!("Reply with exactly: {LIVE_MARKER}"),
        )
        .await?;

        assert!(
            response.to_lowercase().contains(LIVE_MARKER),
            "Copilot response for {model_id} did not contain marker. Raw response: {response}"
        );

        Ok(())
    }

    #[test]
    fn normalizes_tool_call_models_from_data_shape() {
        let raw = json!({
            "data": [{
                "id": "claude-opus-4.7-1m-internal",
                "name": "Claude Opus 4.7 (1M context)(Internal only)",
                "capabilities": {
                    "limits": {
                        "max_context_window_tokens": 1000000,
                        "max_non_streaming_output_tokens": 16000,
                        "max_output_tokens": 64000,
                        "max_prompt_tokens": 936000
                    },
                    "supports": {
                        "adaptive_thinking": true,
                        "reasoning_effort": ["low", "medium", "high", "xhigh"],
                        "tool_calls": true,
                        "vision": true
                    }
                }
            }]
        });

        let row = model_rows(&raw).unwrap().remove(0);
        let model = normalize_model(&row, DEFAULT_BASE_URL).unwrap();

        assert_eq!(model.id, "claude-opus-4.7-1m-internal");
        assert_eq!(model.api, "anthropic-messages");
        assert!(model.reasoning);
        assert_eq!(model.input, vec!["text", "image"]);
        assert_eq!(model.context_window, 1_000_000);
        assert_eq!(model.max_tokens, 64_000);
        assert_eq!(
            model.compat.get("forceAdaptiveThinking"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            model.compat.get("supportsTemperature"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            model
                .thinking_level_map
                .as_ref()
                .and_then(|map| map.get("xhigh")),
            Some(&Value::String("xhigh".to_string()))
        );
    }

    #[test]
    fn filters_models_without_tool_calls_or_disabled_policy() {
        let no_tools = json!({ "id": "gpt-5.5", "capabilities": { "supports": {} } });
        let disabled = json!({
            "id": "gpt-5.5",
            "capabilities": { "supports": { "tool_calls": true } },
            "policy": { "state": "disabled" }
        });

        assert!(normalize_model(&no_tools, DEFAULT_BASE_URL).is_none());
        assert!(normalize_model(&disabled, DEFAULT_BASE_URL).is_none());
    }

    #[test]
    fn classifies_gpt_5_and_adds_thinking_level_map() {
        let raw = json!({
            "id": "gpt-5.5",
            "tool_call": true,
            "model_picker_enabled": true
        });

        let model = normalize_model(&raw, DEFAULT_BASE_URL).unwrap();

        assert_eq!(model.api, "openai-responses");
        let map = model.thinking_level_map.unwrap();
        assert_eq!(map.get("off"), Some(&Value::Null));
        assert_eq!(map.get("minimal"), Some(&Value::String("low".to_string())));
        assert_eq!(map.get("xhigh"), Some(&Value::String("xhigh".to_string())));
    }

    #[test]
    fn completions_models_get_compat_defaults() {
        let raw = json!({ "id": "gpt-4.1", "tool_call": true });
        let model = normalize_model(&raw, DEFAULT_BASE_URL).unwrap();

        assert_eq!(model.api, "openai-completions");
        assert_eq!(model.compat.get("supportsStore"), Some(&Value::Bool(false)));
        assert_eq!(
            model.compat.get("supportsDeveloperRole"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            model.compat.get("supportsReasoningEffort"),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn derives_base_url_from_proxy_endpoint() {
        assert_eq!(
            copilot_base_url(
                "tid=1;proxy-ep=proxy.individual.githubcopilot.com;exp=2",
                None
            ),
            "https://api.individual.githubcopilot.com"
        );
    }

    #[test]
    fn uses_enterprise_base_url_when_token_has_no_proxy() {
        assert_eq!(
            copilot_base_url("tid=1;exp=2", Some("example.ghe.com")),
            "https://copilot-api.example.ghe.com"
        );
    }

    #[test]
    fn model_rows_accepts_all_supported_shapes() {
        assert_eq!(model_rows(&json!([{ "id": "a" }])).unwrap().len(), 1);
        assert_eq!(
            model_rows(&json!({ "data": [{ "id": "a" }] }))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            model_rows(&json!({ "models": [{ "id": "a" }] }))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn preferred_gpt_model_uses_highest_numeric_version() {
        let ids = ["gpt-4o", "gpt-5.3-codex", "gpt-5.5", "claude-opus-4.7"];

        assert_eq!(
            preferred_gpt_model_id_from_ids(ids),
            Some("gpt-5.5".to_string())
        );
    }

    #[test]
    fn reads_credential_shape_without_dropping_enterprise_url() {
        let credentials = read_copilot_credentials(&json!({
            "github-copilot": {
                "type": "oauth",
                "refresh": "github-token",
                "access": "copilot-token",
                "expires": 1770000000000_i64,
                "enterpriseUrl": "enterprise.example"
            },
            "other": { "kept": true }
        }))
        .unwrap();

        assert_eq!(credentials.refresh, "github-token");
        assert_eq!(credentials.access.as_deref(), Some("copilot-token"));
        assert_eq!(credentials.expires, Some(1_770_000_000_000));
        assert_eq!(
            credentials.enterprise_url.as_deref(),
            Some("enterprise.example")
        );
    }
}
