use crate::auth::{AuthResolver, ProviderKind};
use std::sync::Mutex;

// Ensure test isolation for environment variables
static ENV_LOCK: Mutex<()> = Mutex::new(());

// --- AuthResolver Tests ---

#[test]
fn auth_resolver_returns_cli_key_when_provided() {
    let resolver = AuthResolver::new(ProviderKind::OpenAI).with_cli_key(Some("sk-test-key-123"));
    let result = resolver.resolve().unwrap();
    assert_eq!(result, "sk-test-key-123");
}

#[test]
fn auth_resolver_returns_env_var_when_no_cli_key() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("OPENAI_API_KEY", "env-key-456") };
    let resolver = AuthResolver::new(ProviderKind::OpenAI)
        .with_cli_key(None)
        .with_config_keys(None);
    let result = resolver.resolve().unwrap();
    assert_eq!(result, "env-key-456");
    unsafe { std::env::remove_var("OPENAI_API_KEY") };
}

#[test]
fn auth_resolver_returns_config_key_when_no_env() {
    use std::collections::HashMap;
    let mut keys = HashMap::new();
    keys.insert("openai".to_string(), "config-key-789".to_string());

    let resolver = AuthResolver::new(ProviderKind::OpenAI)
        .with_cli_key(None)
        .with_config_keys(Some(&keys));
    let result = resolver.resolve().unwrap();
    assert_eq!(result, "config-key-789");
}

#[test]
fn auth_resolver_cli_key_takes_priority_over_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("OPENAI_API_KEY", "env-key") };
    let resolver = AuthResolver::new(ProviderKind::OpenAI)
        .with_cli_key(Some("cli-key"))
        .with_config_keys(None);
    let result = resolver.resolve().unwrap();
    assert_eq!(result, "cli-key");
    unsafe { std::env::remove_var("OPENAI_API_KEY") };
}

#[test]
fn auth_resolver_env_takes_priority_over_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("OPENAI_API_KEY", "env-key") };
    use std::collections::HashMap;
    let mut keys = HashMap::new();
    keys.insert("openai".to_string(), "config-key".to_string());

    let resolver = AuthResolver::new(ProviderKind::OpenAI)
        .with_cli_key(None)
        .with_config_keys(Some(&keys));
    let result = resolver.resolve().unwrap();
    assert_eq!(result, "env-key");
    unsafe { std::env::remove_var("OPENAI_API_KEY") };
}

#[test]
fn auth_resolver_falls_back_for_empty_cli_key() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("OPENAI_API_KEY", "env-key") };
    let resolver = AuthResolver::new(ProviderKind::OpenAI)
        .with_cli_key(Some("")) // Empty CLI key should be ignored
        .with_config_keys(None);
    let result = resolver.resolve().unwrap();
    assert_eq!(result, "env-key");
    unsafe { std::env::remove_var("OPENAI_API_KEY") };
}

#[test]
fn auth_resolver_ollama_returns_empty_key() {
    let resolver = AuthResolver::new(ProviderKind::Ollama)
        .with_cli_key(None)
        .with_env_override(None)
        .with_config_keys(None);
    let result = resolver.resolve().unwrap();
    assert!(result.is_empty());
}

#[test]
fn auth_resolver_errors_when_no_key_available() {
    let _guard = ENV_LOCK.lock().unwrap();
    // Clear any existing env var that might be set
    unsafe { std::env::remove_var("OPENAI_API_KEY") };
    let resolver = AuthResolver::new(ProviderKind::OpenAI)
        .with_cli_key(None)
        .with_config_keys(None);
    let result = resolver.resolve();
    assert!(result.is_err());
}

#[test]
fn provider_kind_from_name_recognizes_all() {
    assert_eq!(
        ProviderKind::from_name("openrouter"),
        Some(ProviderKind::OpenRouter)
    );
    assert_eq!(
        ProviderKind::from_name("openai"),
        Some(ProviderKind::OpenAI)
    );
    assert_eq!(
        ProviderKind::from_name("anthropic"),
        Some(ProviderKind::Anthropic)
    );
    assert_eq!(
        ProviderKind::from_name("gemini"),
        Some(ProviderKind::Gemini)
    );
    assert_eq!(
        ProviderKind::from_name("google"),
        Some(ProviderKind::Gemini)
    );
    assert_eq!(
        ProviderKind::from_name("ollama"),
        Some(ProviderKind::Ollama)
    );
    // "custom" is an alias for OpenAI
    assert_eq!(
        ProviderKind::from_name("custom"),
        Some(ProviderKind::OpenAI)
    );
}

#[test]
fn provider_kind_from_name_case_insensitive() {
    assert_eq!(
        ProviderKind::from_name("OPENAI"),
        Some(ProviderKind::OpenAI)
    );
    assert_eq!(
        ProviderKind::from_name("OpenAi"),
        Some(ProviderKind::OpenAI)
    );
}

#[test]
fn provider_kind_from_name_returns_none_for_unknown() {
    assert_eq!(ProviderKind::from_name("unknown"), None);
}
