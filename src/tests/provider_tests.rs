use crate::auth::ProviderKind;
use crate::config::{ApiStyle, CustomProviderConfig};
use crate::provider::ModelEntry;
use crate::provider::{
    AnyClient, create_client, expand_env, is_agent_model, merge_extra_body,
    openrouter_anthropic_routing, resolve_api_style, resolve_provider_config,
    serialize_conversation,
};
use crate::session::{MessageRole, SessionMessage};
use compact_str::CompactString;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

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
        input_price: None,
        output_price: None,
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
        tool: None,
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
            tool: None,
        },
        SessionMessage {
            role: MessageRole::Assistant,
            content: CompactString::new("hey"),
            estimated_tokens: 1,
            tool: None,
        },
        SessionMessage {
            role: MessageRole::System,
            content: CompactString::new("note"),
            estimated_tokens: 1,
            tool: None,
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

#[tokio::test]
async fn anthropic_custom_base_appends_v1_messages() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let request_line = String::from_utf8_lossy(&request)
            .lines()
            .next()
            .unwrap()
            .to_string();
        request_tx.send(request_line).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
    });

    let mut custom = HashMap::new();
    custom.insert(
        "anthropic-capture".to_string(),
        CustomProviderConfig {
            provider_type: "anthropic".into(),
            base_url: format!("http://{address}/anthropic"),
            api_key_env: None,
            danger_accept_invalid_certs: None,
            api_style: None,
            headers: HashMap::new(),
            timeout_secs: None,
            model: None,
        },
    );

    let client = create_client("anthropic-capture", Some("test-key"), &custom, None).unwrap();
    let AnyClient::Anthropic(client) = client else {
        panic!("expected an Anthropic client");
    };
    let agent = client.agent("MiniMax-M3").max_tokens(16).build();
    assert!(agent.prompt("hello").await.is_err());

    server.join().unwrap();
    assert_eq!(
        request_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        "POST /anthropic/v1/messages HTTP/1.1"
    );
}

#[test]
fn merge_extra_body_combines_routing_and_user_keys() {
    // OpenRouter routing (provider.order) plus a user `plugins` preset must both
    // survive in the request body.
    let routing = serde_json::json!({
        "provider": { "order": ["Anthropic"], "allow_fallbacks": true }
    });
    let user = serde_json::json!({ "plugins": { "preset": "general-budget" } });
    let merged = merge_extra_body(Some(routing), Some(user)).unwrap();
    assert_eq!(merged["provider"]["order"][0], "Anthropic");
    assert_eq!(merged["plugins"]["preset"], "general-budget");
}

#[test]
fn merge_extra_body_user_key_overrides_base() {
    let base = serde_json::json!({ "provider": { "order": ["Anthropic"] } });
    let user = serde_json::json!({ "provider": { "order": ["OpenAI"] } });
    let merged = merge_extra_body(Some(base), Some(user)).unwrap();
    assert_eq!(merged["provider"]["order"][0], "OpenAI");
}

#[test]
fn merge_extra_body_handles_absent_sides() {
    let val = serde_json::json!({ "plugins": { "preset": "quality" } });
    assert_eq!(merge_extra_body(None, Some(val.clone())), Some(val.clone()));
    assert_eq!(merge_extra_body(Some(val.clone()), None), Some(val));
    assert_eq!(merge_extra_body(None, None), None);
}

// --- openrouter_anthropic_routing tests ---

#[test]
fn pins_anthropic_namespaced_openrouter_models() {
    for id in [
        "anthropic/claude-sonnet-4.6",
        "anthropic/claude-opus-4.8",
        "anthropic/claude-3.5-haiku",
    ] {
        let extra = openrouter_anthropic_routing(id).expect("should pin {id}");
        assert_eq!(extra["provider"]["order"][0], "Anthropic");
        assert_eq!(extra["provider"]["allow_fallbacks"], true);
    }
}

#[test]
fn pins_tilde_prefixed_latest_aliases() {
    // OpenRouter floating aliases carry a leading `~` that is part of the
    // real slug; they must still be pinned to the Anthropic route.
    for id in [
        "~anthropic/claude-sonnet-latest",
        "~anthropic/claude-opus-latest",
        "~anthropic/claude-haiku-latest",
    ] {
        assert!(
            openrouter_anthropic_routing(id).is_some(),
            "{id} should be pinned"
        );
    }
}

#[test]
fn leaves_non_anthropic_openrouter_models_untouched() {
    for id in [
        "openai/gpt-4o",
        "deepseek/deepseek-chat",
        "google/gemini-2.5-pro",
        "openrouter/auto",
        // A non-Anthropic model that merely mentions claude in its path
        // is not in the anthropic namespace and must not be pinned.
        "somegateway/not-claude",
    ] {
        assert!(
            openrouter_anthropic_routing(id).is_none(),
            "{id} should not be pinned"
        );
    }
}

// --- parse_model_infos tests ---

#[test]
fn parses_openrouter_string_pricing() {
    let body = br#"{
        "data": [
            {
                "id": "openai/gpt-4o",
                "pricing": { "prompt": "0.0000025", "completion": "0.00001" },
                "context_length": 128000
            }
        ]
    }"#;
    let infos = crate::provider::parse_model_infos(body).unwrap();
    let info = infos.get("openai/gpt-4o").expect("model should be present");
    assert_eq!(info.input_cost, 2.5);
    assert_eq!(info.output_cost, 10.0);
    assert_eq!(info.context_length, Some(128_000));
}

#[test]
fn parses_numeric_pricing_from_compatible_gateways() {
    // laroute serializes pricing as numbers, not strings.
    let body = br#"{
        "data": [
            {
                "id": "moonshotai/kimi-k2.6",
                "pricing": { "prompt": 0.000000646, "completion": 0.00000272, "request": 0 },
                "context_length": 262144
            }
        ]
    }"#;
    let infos = crate::provider::parse_model_infos(body).unwrap();
    let info = infos
        .get("moonshotai/kimi-k2.6")
        .expect("model should be present");
    assert!((info.input_cost - 0.646).abs() < 1e-9);
    assert!((info.output_cost - 2.72).abs() < 1e-9);
    assert_eq!(info.context_length, Some(262_144));
}

#[test]
fn keeps_context_length_only_entries_and_skips_empty_ones() {
    let body = br#"{
        "data": [
            { "id": "ctx-only", "context_length": 1048576 },
            { "id": "free", "pricing": { "prompt": 0, "completion": 0 } },
            { "id": "empty" }
        ]
    }"#;
    let infos = crate::provider::parse_model_infos(body).unwrap();
    assert_eq!(
        infos.get("ctx-only").and_then(|i| i.context_length),
        Some(1_048_576)
    );
    assert!(
        !infos.contains_key("free"),
        "zero pricing without context_length is skipped"
    );
    assert!(!infos.contains_key("empty"));
}

// --- parse_manual_models tests ---

#[test]
fn manual_models_pick_up_context_length_when_reported() {
    let body = br#"{
        "object": "list",
        "data": [
            { "id": "deepseek/deepseek-v4-flash", "object": "model", "context_length": 1048576 },
            { "id": "plain-model", "object": "model" }
        ]
    }"#;
    let models = crate::provider::parse_manual_models(body).unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "deepseek/deepseek-v4-flash");
    assert_eq!(models[0].context_length, Some(1_048_576));
    assert_eq!(models[1].id, "plain-model");
    assert_eq!(models[1].context_length, None);
}
