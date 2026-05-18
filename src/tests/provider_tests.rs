use std::collections::HashMap;

use crate::config::CustomProviderConfig;
use crate::provider::{parse_provider, resolve_provider_info, ProviderKind};

#[test]
fn parse_minimax_by_name() {
    assert_eq!(parse_provider("minimax"), Some(ProviderKind::MiniMax));
    assert_eq!(parse_provider("MiniMax"), Some(ProviderKind::MiniMax));
}

#[test]
fn parse_minimax_case_insensitive() {
    assert_eq!(parse_provider("MiniMax"), Some(ProviderKind::MiniMax));
    assert_eq!(parse_provider("minimax"), Some(ProviderKind::MiniMax));
}

#[test]
fn parse_provider_all_known() {
    assert_eq!(parse_provider("openrouter"), Some(ProviderKind::OpenRouter));
    assert_eq!(parse_provider("openai"), Some(ProviderKind::OpenAI));
    assert_eq!(parse_provider("anthropic"), Some(ProviderKind::Anthropic));
    assert_eq!(parse_provider("gemini"), Some(ProviderKind::Gemini));
    assert_eq!(parse_provider("google"), Some(ProviderKind::Gemini));
    assert_eq!(parse_provider("ollama"), Some(ProviderKind::Ollama));
    assert_eq!(parse_provider("custom"), Some(ProviderKind::Custom));
    assert_eq!(parse_provider("unknown"), None);
}

#[test]
fn resolve_minimax_provider_info() {
    let custom_providers = HashMap::new();
    let info = resolve_provider_info("minimax", &custom_providers).unwrap();
    assert_eq!(info.kind, ProviderKind::MiniMax);
    assert!(info.base_url.is_none());
    assert!(info.api_key_env.is_none());
}

#[test]
fn resolve_minimax_custom_override() {
    let mut custom_providers = HashMap::new();
    custom_providers.insert(
        "minimax".to_string(),
        CustomProviderConfig {
            provider_type: "minimax".to_string(),
            base_url: "https://api.minimaxi.com/v1".to_string(),
            api_key_env: Some("MY_MINIMAX_KEY".to_string()),
        },
    );
    let info = resolve_provider_info("minimax", &custom_providers).unwrap();
    assert_eq!(info.kind, ProviderKind::MiniMax);
    assert_eq!(info.base_url.as_deref(), Some("https://api.minimaxi.com/v1"));
    assert_eq!(info.api_key_env.as_deref(), Some("MY_MINIMAX_KEY"));
}

#[test]
fn minimax_create_client_with_explicit_key() {
    let custom_providers = HashMap::new();
    let result = crate::provider::create_client("minimax", Some("explicit-key"), &custom_providers);
    assert!(result.is_ok(), "Failed with explicit key: {:?}", result.err());
}

#[test]
fn minimax_create_client_missing_key_fails() {
    // Use an override env var that definitely doesn't exist
    let mut custom_providers = HashMap::new();
    custom_providers.insert(
        "minimax".to_string(),
        CustomProviderConfig {
            provider_type: "minimax".to_string(),
            base_url: "https://api.minimax.io/v1".to_string(),
            api_key_env: Some("MINIMAX_TEST_NONEXISTENT_KEY_XYZ".to_string()),
        },
    );
    let result = crate::provider::create_client("minimax", None, &custom_providers);
    match result {
        Ok(_) => panic!("Expected error when API key is missing"),
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("MINIMAX_TEST_NONEXISTENT_KEY_XYZ"),
                "Error should mention the env var, got: {}",
                err_msg
            );
        }
    }
}

#[test]
fn minimax_completion_model_loads() {
    let custom_providers = HashMap::new();
    let client =
        crate::provider::create_client("minimax", Some("test-key"), &custom_providers).unwrap();
    let _model = client.completion_model("MiniMax-M2.7");
    let _model_hs = client.completion_model("MiniMax-M2.7-highspeed");
}
