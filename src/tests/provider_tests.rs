use crate::auth::ProviderKind;
use crate::config::{ApiStyle, CustomProviderConfig};
use crate::provider::ModelEntry;
use crate::provider::{
    expand_env, is_agent_model, resolve_api_style, resolve_provider_config, serialize_conversation,
};
use crate::session::{MessageRole, SessionMessage};
use compact_str::CompactString;
use std::collections::HashMap;

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
    unsafe { std::env::set_var("ZS_TEST_HDR", "secret-value") };
    assert_eq!(expand_env("${ZS_TEST_HDR}").unwrap(), "secret-value");
    unsafe { std::env::remove_var("ZS_TEST_HDR") };
}

#[test]
fn expand_env_missing_var_errors() {
    assert!(expand_env("${ZS_DEFINITELY_NOT_SET_98237}").is_err());
}

// --- is_agent_model tests ---

fn model(id: &str, kind: Option<&str>) -> ModelEntry {
    ModelEntry {
        id: id.to_string(),
        display: id.to_string(),
        context_length: None,
        kind: kind.map(|s| s.to_string()),
    }
}

#[test]
fn agent_model_plain_chat() {
    assert!(is_agent_model(&model("gpt-4", None)));
    assert!(is_agent_model(&model("claude-sonnet", None)));
}

#[test]
fn non_agent_embedding_kind() {
    assert!(!is_agent_model(&model("text-embedding-3", Some("embed"))));
}

#[test]
fn non_agent_image_kind() {
    assert!(!is_agent_model(&model("dall-e-3", Some("image"))));
}

#[test]
fn non_agent_audio_kind() {
    assert!(!is_agent_model(&model("whisper-1", Some("audio"))));
}

#[test]
fn non_agent_speech_kind() {
    assert!(!is_agent_model(&model("tts-1", Some("speech"))));
}

#[test]
fn non_agent_by_id_deny_list() {
    assert!(!is_agent_model(&model("text-embedding-ada-002", None)));
    assert!(!is_agent_model(&model("whisper-large", None)));
    assert!(!is_agent_model(&model("dall-e-3", None)));
    assert!(!is_agent_model(&model("imagen-3", None)));
}

#[test]
fn non_agent_by_id_deny_list_partial_match() {
    assert!(!is_agent_model(&model("some-embed-model", None)));
    assert!(!is_agent_model(&model("tts-model-v2", None)));
    assert!(!is_agent_model(&model("veo-video-gen", None)));
}

// --- serialize_conversation tests ---

#[test]
fn serialize_empty() {
    let result = serialize_conversation(&[]);
    assert!(result.is_empty());
}

#[test]
fn serialize_single_user_message() {
    let msgs = vec![SessionMessage {
        role: MessageRole::User,
        content: CompactString::new("hello"),
        estimated_tokens: 1,
    }];
    let result = serialize_conversation(&msgs);
    assert!(result.contains("[User]: hello"));
}

#[test]
fn serialize_multiple_roles() {
    let msgs = vec![
        SessionMessage {
            role: MessageRole::User,
            content: CompactString::new("hi"),
            estimated_tokens: 1,
        },
        SessionMessage {
            role: MessageRole::Assistant,
            content: CompactString::new("hey"),
            estimated_tokens: 1,
        },
        SessionMessage {
            role: MessageRole::System,
            content: CompactString::new("note"),
            estimated_tokens: 1,
        },
    ];
    let result = serialize_conversation(&msgs);
    assert!(result.contains("[User]: hi"));
    assert!(result.contains("[Assistant]: hey"));
    assert!(result.contains("[System]: note"));
}

// --- resolve_provider_config tests ---

#[test]
fn resolve_builtin_openai() {
    let cfg = resolve_provider_config("openai", &HashMap::new()).unwrap();
    assert_eq!(cfg.kind, ProviderKind::OpenAI);
    assert!(cfg.base_url.is_none());
}

#[test]
fn resolve_builtin_anthropic() {
    let cfg = resolve_provider_config("anthropic", &HashMap::new()).unwrap();
    assert_eq!(cfg.kind, ProviderKind::Anthropic);
}

#[test]
fn resolve_builtin_gemini() {
    let cfg = resolve_provider_config("gemini", &HashMap::new()).unwrap();
    assert_eq!(cfg.kind, ProviderKind::Gemini);
}

#[test]
fn resolve_builtin_google_alias() {
    let cfg = resolve_provider_config("google", &HashMap::new()).unwrap();
    assert_eq!(cfg.kind, ProviderKind::Gemini);
}

#[test]
fn resolve_builtin_ollama() {
    let cfg = resolve_provider_config("ollama", &HashMap::new()).unwrap();
    assert_eq!(cfg.kind, ProviderKind::Ollama);
}

#[test]
fn resolve_builtin_openrouter() {
    let cfg = resolve_provider_config("openrouter", &HashMap::new()).unwrap();
    assert_eq!(cfg.kind, ProviderKind::OpenRouter);
}

#[test]
fn resolve_unknown_provider_errors() {
    let result = resolve_provider_config("nonexistent_provider_xyz", &HashMap::new());
    assert!(result.is_err());
}

#[test]
fn resolve_custom_provider() {
    let mut custom = HashMap::new();
    custom.insert(
        "my-gw".to_string(),
        CustomProviderConfig {
            provider_type: "openai".into(),
            base_url: "https://mygw.example/v1".to_string(),
            api_key_env: None,
            danger_accept_invalid_certs: None,
            api_style: None,
            headers: HashMap::new(),
            timeout_secs: None,
            model: None,
        },
    );
    let cfg = resolve_provider_config("my-gw", &custom).unwrap();
    assert_eq!(cfg.kind, ProviderKind::OpenAI);
    assert_eq!(cfg.base_url.as_deref(), Some("https://mygw.example/v1"));
}
