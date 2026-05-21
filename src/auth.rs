use std::collections::HashMap;

/// Kind of AI provider
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenRouter,
    OpenAI,
    Anthropic,
    Gemini,
    Ollama,
}

impl ProviderKind {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "openrouter" => Some(Self::OpenRouter),
            "openai" | "custom" => Some(Self::OpenAI), // "custom" is an alias for OpenAI client
            "anthropic" => Some(Self::Anthropic),
            "gemini" | "google" => Some(Self::Gemini),
            "ollama" => Some(Self::Ollama),
            _ => None,
        }
    }
}

/// Resolver for API keys with priority: CLI arg > env var > config file
#[derive(Debug, Clone)]
pub struct AuthResolver {
    pub provider_kind: ProviderKind,
    pub api_key_env_override: Option<String>,
    pub cli_key: Option<String>,
    pub config_api_keys: Option<HashMap<String, String>>,
}

impl AuthResolver {
    pub fn new(kind: ProviderKind) -> Self {
        Self {
            provider_kind: kind,
            api_key_env_override: None,
            cli_key: None,
            config_api_keys: None,
        }
    }

    pub fn with_cli_key(mut self, key: Option<&str>) -> Self {
        self.cli_key = key.filter(|k| !k.is_empty()).map(String::from);
        self
    }

    pub fn with_env_override(mut self, env_var: Option<&str>) -> Self {
        self.api_key_env_override = env_var.filter(|s| !s.is_empty()).map(String::from);
        self
    }

    pub fn with_config_keys(mut self, keys: Option<&HashMap<String, String>>) -> Self {
        self.config_api_keys = keys.cloned();
        self
    }

    pub fn resolve(&self) -> anyhow::Result<String> {
        // Priority 1: CLI argument
        if let Some(ref key) = self.cli_key {
            tracing::warn!(
                "API key provided via --api-key is visible in process listings. \
                 Use the {} environment variable instead.",
                self.env_var_name()
            );
            return Ok(key.clone());
        }

        // Priority 2: Environment variable
        let env_var = self
            .api_key_env_override
            .as_deref()
            .unwrap_or_else(|| self.env_var_name());

        if let Ok(key) = std::env::var(env_var)
            && !key.is_empty()
        {
            return Ok(key);
        }

        // Priority 3: Config file
        let slug = self.provider_slug();
        if let Some(key) = self
            .config_api_keys
            .as_ref()
            .and_then(|m| m.get(slug))
            .filter(|k| !k.is_empty())
        {
            return Ok(key.clone());
        }

        // Ollama doesn't require an API key
        if self.provider_kind == ProviderKind::Ollama {
            return Ok(String::new());
        }

        anyhow::bail!(
            "No API key found. Set the {} environment variable, add it to config.api_keys, or pass --api-key.",
            env_var
        )
    }

    fn env_var_name(&self) -> &'static str {
        match self.provider_kind {
            ProviderKind::OpenAI => "OPENAI_API_KEY",
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::Gemini => "GEMINI_API_KEY",
            ProviderKind::Ollama => "OLLAMA_API_KEY",
            ProviderKind::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    fn provider_slug(&self) -> &'static str {
        match self.provider_kind {
            ProviderKind::OpenRouter => "openrouter",
            ProviderKind::OpenAI => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Gemini => "gemini",
            ProviderKind::Ollama => "ollama",
        }
    }
}
