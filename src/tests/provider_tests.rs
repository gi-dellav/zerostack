use crate::auth::ProviderKind;
use crate::config::{ApiStyle, CustomProviderConfig};
use crate::provider::ModelEntry;
use crate::provider::{
    AnyClient, AnyModel, DEFAULT_METADATA_TIMEOUT, HttpPurpose, OpenAiModel, build_http_client,
    create_client, expand_env, http_total_timeout, is_agent_model, merge_extra_body,
    openrouter_anthropic_routing, resolve_api_style, resolve_provider_config,
    serialize_conversation,
};
use crate::session::{MessageRole, SessionMessage};
use compact_str::CompactString;
use rig::agent::AgentBuilder;
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
fn resolve_builtin_opencode_zen() {
    let cfg = resolve_provider_config("opencode-zen", &HashMap::new()).unwrap();
    assert_eq!(cfg.kind, ProviderKind::OpencodeZen);
    assert_eq!(cfg.base_url.as_deref(), Some("https://opencode.ai/zen/v1"));
}

#[test]
fn resolve_builtin_opencode_go() {
    let cfg = resolve_provider_config("opencode-go", &HashMap::new()).unwrap();
    assert_eq!(cfg.kind, ProviderKind::OpencodeGo);
    assert_eq!(
        cfg.base_url.as_deref(),
        Some("https://opencode.ai/zen/go/v1")
    );
}

// --- opencode transport tests ---

#[test]
fn opencode_transport_matches_gateway_endpoints() {
    use crate::provider::{OpencodeTransport, opencode_transport};
    assert_eq!(
        opencode_transport("opencode-zen", "kimi-k2.6"),
        Some(OpencodeTransport::Chat)
    );
    assert_eq!(
        opencode_transport("opencode-zen", "gpt-5.5"),
        Some(OpencodeTransport::Responses)
    );
    assert_eq!(
        opencode_transport("opencode-zen", "claude-opus-4-6"),
        Some(OpencodeTransport::Messages)
    );
    assert_eq!(
        opencode_transport("opencode-go", "kimi-k2.6"),
        Some(OpencodeTransport::Chat)
    );
    assert_eq!(
        opencode_transport("opencode-go", "gpt-5.6-luna"),
        Some(OpencodeTransport::Responses)
    );
    assert_eq!(
        opencode_transport("opencode-go", "minimax-m2.7"),
        Some(OpencodeTransport::Messages)
    );
    // Free models ride the same transports as paid ones.
    assert_eq!(
        opencode_transport("opencode-zen", "mimo-v2.5-free"),
        Some(OpencodeTransport::Chat)
    );
    assert_eq!(
        opencode_transport("opencode-zen", "muse-spark-1.3-contributor-free"),
        Some(OpencodeTransport::Responses)
    );
    // Unknown models and non-OpenCode providers have no baked transport.
    assert_eq!(opencode_transport("opencode-zen", "no-such-model"), None);
    assert_eq!(opencode_transport("openai", "gpt-5.5"), None);
}

#[test]
fn opencode_transport_falls_back_to_chat() {
    use crate::provider::{OpencodeTransport, resolve_opencode_transport};
    assert_eq!(
        resolve_opencode_transport("opencode-zen", "no-such-model"),
        OpencodeTransport::Chat
    );
}

#[test]
fn opencode_session_header_is_stable_and_present() {
    use crate::provider::opencode_session_header_value;
    // Exact bytes for a given id (the gateway gates on this header).
    let value = opencode_session_header_value("test-conversation-id").unwrap();
    assert_eq!(value.to_str().unwrap(), "test-conversation-id");
    // The live id is process-global (OnceLock: first set wins, tests share
    // it), so only stability — not the exact value — is asserted for it.
    let live = crate::provider::opencode_session_header().unwrap();
    assert!(!live.to_str().unwrap().is_empty());
    assert_eq!(crate::provider::opencode_session_header().unwrap(), live);
}

#[test]
fn opencode_client_dispatches_per_model_transport() {
    // Zen needs no key: without one it resolves the public credential, so
    // client construction stays offline and deterministic in tests.
    let client = create_client("opencode-zen", None, &HashMap::new(), None).unwrap();
    assert_eq!(client.provider_name(), "opencode-zen");
    assert!(matches!(
        client.completion_model("kimi-k2.6"),
        AnyModel::OpenAI(OpenAiModel::Completions(_))
    ));
    assert!(matches!(
        client.completion_model("gpt-5.5"),
        AnyModel::OpenAI(OpenAiModel::Responses(_))
    ));
    assert!(matches!(
        client.completion_model("claude-opus-4-6"),
        AnyModel::Anthropic(_)
    ));

    let client = create_client("opencode-go", Some("sk-test"), &HashMap::new(), None).unwrap();
    assert_eq!(client.provider_name(), "opencode-go");
    assert!(matches!(
        client.completion_model("kimi-k2.6"),
        AnyModel::OpenAI(OpenAiModel::Completions(_))
    ));
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

/// Read one HTTP request head (through the blank line) off `stream`.
/// Returns `None` on a read error or EOF before the head is complete.
fn read_request_head(stream: &mut std::net::TcpStream) -> Option<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).ok()?;
        if count == 0 {
            return None;
        }
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Some(request);
        }
    }
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

        let request = read_request_head(&mut stream).expect("request head");

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
    let agent = AgentBuilder::new(client.completion("MiniMax-M3"))
        .max_tokens(16)
        .build();
    assert!(agent.prompt("hello").await.is_err());

    server.join().unwrap();
    assert_eq!(
        request_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        "POST /anthropic/v1/messages HTTP/1.1"
    );
}

fn minimax_recipe() -> crate::config::Config {
    let recipe = include_str!("../../docs/providers/Minimax.md");
    let config = recipe
        .split("```toml\n")
        .skip(1)
        .filter_map(|block| block.split("```").next())
        .filter(|block| {
            block.starts_with("[custom_providers.") || block.starts_with("[quick_models.")
        })
        .collect::<Vec<_>>()
        .join("\n");
    toml::from_str(&config).unwrap()
}

#[test]
fn minimax_picker_filters_only_known_routes_and_preserves_metadata() {
    use crate::provider::filter_minimax_choices;
    for (provider, old, flash, m3) in [
        (
            "openrouter",
            "minimax/minimax-m2.7",
            "minimax/minimax-m3.1-flash:free",
            "minimax/minimax-m3",
        ),
        (
            "opencode-zen",
            "minimax-m2.5",
            "minimax-m3.1-flash-free",
            "minimax-m3",
        ),
        (
            "opencode-go",
            "minimax-m2.7",
            "minimax-m3.1-flash-free",
            "minimax-m3",
        ),
    ] {
        let mut models = vec![
            model("other-model", None),
            model(old, None),
            model(m3, None),
            model(flash, None),
        ];
        models[2].context_length = Some(512_000);
        models[2].input_price = Some(0.3);
        filter_minimax_choices(provider, None, &mut models);
        let expected = if provider == "openrouter"
            || crate::provider::opencode_transport(provider, flash).is_some()
        {
            vec!["other-model", flash, m3]
        } else {
            vec!["other-model", m3]
        };
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            expected
        );
        let current = models.iter().find(|m| m.id == m3).unwrap();
        assert_eq!(current.context_length, Some(512_000));
        assert_eq!(current.input_price, Some(0.3));
        // No invented Flash entry when the gateway only lists M3.
        let mut models = vec![model(old, None), model(m3, None)];
        filter_minimax_choices(provider, None, &mut models);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, m3);
    }
    for (_, custom) in minimax_recipe().custom_providers_map() {
        let mut models = vec![
            model("MiniMax-M2.7", None),
            model("MiniMax-Text-01", None),
            model("abab6.5-chat", None),
            model("MiniMax-M3", None),
            model("MiniMax-M3.1-Flash", None),
            model("speech-2.8-hd", Some("audio")),
        ];
        filter_minimax_choices("renamed-route", Some(&custom), &mut models);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["MiniMax-M3.1-Flash", "MiniMax-M3", "speech-2.8-hd"]
        );
    }
    let custom = cfg(None);
    let mut models = vec![model("MiniMax-M2.7", None)];
    filter_minimax_choices("openrouter", Some(&custom), &mut models);
    assert_eq!(models[0].id, "MiniMax-M2.7");
    filter_minimax_choices("ollama", None, &mut models);
    assert_eq!(models[0].id, "MiniMax-M2.7");
}

#[tokio::test]
async fn minimax_baked_picker_hides_legacy_without_changing_saved_selection() {
    for provider in ["opencode-zen", "opencode-go"] {
        let client = create_client(provider, Some("test-key"), &HashMap::new(), None).unwrap();
        let cli = crate::cli::Cli {
            provider: Some(provider.into()),
            ..Default::default()
        };
        let config = crate::config::Config {
            model: Some("minimax-m2.7".into()),
            ..Default::default()
        };
        let choices =
            crate::ui::slash::warm_model_cache(provider, false, &client, &cli, &config).await;
        assert!(choices.iter().any(|id| id == "minimax-m3"));
        assert!(
            !choices
                .iter()
                .any(|id| id == "minimax-m2.7" || id == "minimax-m2.5")
        );
        assert_eq!(cli.resolve_model(&config), "minimax-m2.7");
        let explicit = crate::cli::Cli {
            model: Some("my-custom-model".into()),
            ..cli
        };
        assert_eq!(explicit.resolve_model(&config), "my-custom-model");
    }
}

/// Capture the complete serialized SDK request, including bodies split across reads.
async fn capture_minimax_request(
    route: &str,
    saved_model: Option<&str>,
    extra: Option<serde_json::Value>,
) -> (String, serde_json::Value) {
    use crate::agent::builder::AgentBuild;
    use crate::provider::{AnyAgent, OpenAiAgent, build_agent};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut config = minimax_recipe();
    let mut custom = config.custom_providers_map();
    let route_config = custom.get_mut(route).unwrap();
    let original = reqwest::Url::parse(&route_config.base_url).unwrap();
    let region = if route.contains("-cn-") {
        "api.minimaxi.com"
    } else {
        "api.minimax.io"
    };
    assert_eq!(original.host_str(), Some(region));
    if let Some(saved) = saved_model {
        route_config.model = Some(saved.into());
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    route_config.base_url = format!(
        "http://{}{}",
        listener.local_addr().unwrap(),
        original.path()
    );
    config.custom_providers = Some(custom.clone());
    config.extra_body = extra.clone();
    let cli = crate::cli::Cli {
        provider: Some(route.into()),
        no_tools: true,
        ..Default::default()
    };
    let model_id = cli.resolve_model(&config);
    assert_eq!(model_id, saved_model.unwrap_or("MiniMax-M3.1-Flash"));

    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            let (head_end, content_length) = loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0, "incomplete HTTP headers");
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..end]);
                    let length = head
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .expect("SDK content length");
                    break (end + 4, length);
                }
            };
            while request.len() < head_end + content_length {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0, "incomplete HTTP body");
                request.extend_from_slice(&buffer[..count]);
            }
            let line = String::from_utf8_lossy(&request[..head_end])
                .lines()
                .next()
                .unwrap()
                .to_owned();
            let body: serde_json::Value =
                serde_json::from_slice(&request[head_end..head_end + content_length]).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            (line, body)
        })
        .await
        .expect("HTTP request timeout")
    });
    let client = create_client(route, Some("test-key"), &custom, None).unwrap();
    let context = crate::context::load_with_prompts_dirs(true, &[]);
    let agent = build_agent(
        client.completion_model(model_id.to_string()),
        AgentBuild {
            cli: &cli,
            cfg: &config,
            context: &context,
            permission: None,
            ask_tx: None,
            sandbox: crate::sandbox::Sandbox::new(false, "bwrap"),
            reasoning_enabled: false,
            temperature: None,
            extra_body: crate::config::resolve_extra_body(&config, &model_id),
            #[cfg(feature = "mcp")]
            mcp_manager: None,
        },
    )
    .await;
    let agent = match agent {
        AnyAgent::OpenAI(OpenAiAgent::Completions(a)) | AnyAgent::Anthropic(a) => a,
        _ => panic!("unexpected MiniMax transport"),
    };
    let prompt = rig::completion::Message::user("hello");
    #[cfg(feature = "multimodal")]
    let prompt = {
        use rig::completion::message::{ImageMediaType, UserContent};
        let rig::completion::Message::User { mut content } = prompt else {
            unreachable!()
        };
        content.push(UserContent::image_base64(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=".to_string(),
            Some(ImageMediaType::PNG),
            None,
        ));
        rig::completion::Message::User { content }
    };
    let history = vec![
        rig::completion::Message::user("saved question"),
        rig::completion::Message::assistant("saved answer"),
    ];
    let error = agent.prompt(prompt).history(history).await.unwrap_err();
    let (line, body) = server.await.unwrap_or_else(|failure| {
        panic!("{route} {model_id}: capture failed: {failure}; SDK error: {error}")
    });
    let serialized = body["messages"].to_string();
    assert!(serialized.contains("saved question"));
    assert!(serialized.contains("saved answer"));
    #[cfg(feature = "multimodal")]
    assert!(serialized.contains("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII="));
    assert_eq!(body["model"], model_id.as_str());
    let max_tokens = body
        .get("max_tokens")
        .or_else(|| body.get("max_completion_tokens"));
    assert_eq!(
        max_tokens.and_then(serde_json::Value::as_u64),
        Some(cli.resolve_max_tokens(&config))
    );
    assert!(body["messages"].as_array().is_some_and(|m| !m.is_empty()));
    for field in [
        "thinking",
        "service_tier",
        "reasoning_split",
        "reasoning_effort",
        "output_config",
    ] {
        assert_eq!(
            body.get(field),
            extra.as_ref().and_then(|e| e.get(field)),
            "unexpected {field}"
        );
    }
    (line, body)
}

#[tokio::test]
async fn minimax_recipe_defaults_and_saved_models_reach_http_boundary() {
    for route in [
        "minimax-global-openai",
        "minimax-cn-openai",
        "minimax-global-anthropic",
        "minimax-cn-anthropic",
    ] {
        for saved in [
            None,
            Some("MiniMax-M3"),
            Some("MiniMax-M2.7"),
            Some("my-custom-model"),
        ] {
            let (line, _) = capture_minimax_request(route, saved, None).await;
            let path = if route.ends_with("anthropic") {
                "/anthropic/v1/messages"
            } else {
                "/v1/chat/completions"
            };
            assert_eq!(line, format!("POST {path} HTTP/1.1"));
        }
        for effort in ["low", "medium", "high", "xhigh", "max"] {
            let extra = if route.ends_with("anthropic") {
                serde_json::json!({"thinking": {"type": "adaptive"}, "output_config": {"effort": effort}})
            } else {
                serde_json::json!({"thinking": {"type": "adaptive"}, "reasoning_effort": effort})
            };
            capture_minimax_request(route, None, Some(extra)).await;
        }
        for thinking in ["adaptive", "disabled"] {
            capture_minimax_request(
                route,
                Some("MiniMax-M3"),
                Some(serde_json::json!({"thinking": {"type": thinking}})),
            )
            .await;
        }
    }
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

// ── HTTP client deadlines ────────────────────────────────────────────────────

#[test]
fn completion_client_has_no_default_deadline() {
    assert_eq!(http_total_timeout(HttpPurpose::Completion, None), None);
    let custom = cfg(None);
    assert_eq!(
        http_total_timeout(HttpPurpose::Completion, Some(&custom)),
        None
    );
}

#[test]
fn metadata_client_defaults_to_the_metadata_deadline() {
    assert_eq!(
        http_total_timeout(HttpPurpose::Metadata, None),
        Some(DEFAULT_METADATA_TIMEOUT)
    );
    let custom = cfg(None);
    assert_eq!(
        http_total_timeout(HttpPurpose::Metadata, Some(&custom)),
        Some(DEFAULT_METADATA_TIMEOUT)
    );
}

#[test]
fn explicit_timeout_secs_applies_to_both_purposes() {
    let mut custom = cfg(None);
    custom.timeout_secs = Some(42);
    for purpose in [HttpPurpose::Completion, HttpPurpose::Metadata] {
        assert_eq!(
            http_total_timeout(purpose, Some(&custom)),
            Some(Duration::from_secs(42))
        );
    }
}

/// Serve `chunks` chunked-body frames one second apart on a fresh port, for
/// as many connections as arrive. Returns the base URL. The accept loop lives
/// as long as the test binary; that is fine for a loopback stub.
fn spawn_slow_stream_server(chunks: u64) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                if read_request_head(&mut stream).is_none() {
                    return;
                }
                if stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                          Transfer-Encoding: chunked\r\n\r\n",
                    )
                    .is_err()
                {
                    return;
                }
                for index in 0..chunks {
                    let frame = format!("data: tok{index}\n\n");
                    let chunk = format!("{:x}\r\n{frame}\r\n", frame.len());
                    // The metadata client hangs up at its deadline; a write
                    // error there is the expected outcome, not a failure.
                    if stream.write_all(chunk.as_bytes()).is_err() || stream.flush().is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
                let _ = stream.write_all(b"0\r\n\r\n");
            });
        }
    });
    format!("http://{address}")
}

/// Regression for the whole-request deadline that used to sit on the
/// completion client: a reply streaming longer than it died mid-stream as
/// "error decoding response body". Both clients come from the pooled
/// built-in path (`custom == None`, no base URL), the path the bug was on.
/// The stream outlasts the metadata deadline by one second, so bumping the
/// constant keeps the boundary. The sleeps overlap with the rest of the suite.
#[tokio::test]
async fn completion_client_survives_stream_longer_than_metadata_deadline() {
    let base = spawn_slow_stream_server(DEFAULT_METADATA_TIMEOUT.as_secs() + 1);
    let completion = build_http_client("slow", false, None, None, HttpPurpose::Completion).unwrap();
    let metadata = build_http_client("slow", false, None, None, HttpPurpose::Metadata).unwrap();

    let (streamed, fetched) = tokio::join!(
        async { completion.get(&base).send().await.unwrap().text().await },
        async { metadata.get(&base).send().await.unwrap().text().await },
    );

    let body = streamed.expect("completion client must read the whole slow stream");
    assert_eq!(
        body.matches("data: tok").count() as u64,
        DEFAULT_METADATA_TIMEOUT.as_secs() + 1
    );

    // Users saw reqwest render this as "error decoding response body"; the
    // wording is reqwest's, so only the timeout classification is asserted.
    let err = fetched.expect_err("metadata client must give up at its deadline");
    assert!(
        err.is_timeout(),
        "metadata error should be a timeout: {err}"
    );
}
